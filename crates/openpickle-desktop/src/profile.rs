//! Small, cache-safe data for the Profile tab.
//!
//! `SessionResponse` contains a LiveKit token. Convert it immediately with
//! [`ProfileData::apply_session`] and discard it; only the fields below are suitable for the
//! desktop cache and UI protocol.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use openpickle_api::{CreditUsage, Profile, SessionResponse, events::UsageEvent};
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileData {
    pub display_name: String,
    pub avatar_url: Option<String>,
    /// `None` means that usage has not been loaded. Zero values inside a snapshot are real.
    pub usage: Option<UsageSnapshot>,
    /// Catalog id to reset percentage. This lets later SSE credit events update ticket counts
    /// without retaining the session response or assuming server catalog ids.
    #[serde(default)]
    pub ticket_fractions: BTreeMap<String, u8>,
    /// `None` means unavailable; `Some([])` means the server returned an empty history.
    pub ticket_history: Option<Vec<TicketHistoryItem>>,
    /// `None` means the connection catalog has not been loaded.
    pub connections: Option<ConnectionSummary>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub allowance: i64,
    pub remaining: i64,
    pub reset_at: Option<String>,
    pub period: Option<String>,
    pub ticket_auto_use: bool,
    pub ticket_order: Option<String>,
    pub tickets: TicketCounts,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketCounts {
    /// `None` means the consumables catalog did not identify a full-reset ticket.
    pub full: Option<i64>,
    /// `None` means the consumables catalog did not identify a half-reset ticket.
    pub half: Option<i64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TicketHistoryItem {
    pub id: String,
    pub label: String,
    pub granted_at_epoch_ms: Option<i64>,
    pub granted_text: Option<String>,
    pub count: i64,
    pub ticket_fraction_pct: Option<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionSummary {
    /// The server count excludes device permissions that only the mobile client knows about.
    pub enabled_count: Option<usize>,
    #[serde(default)]
    pub enabled: Vec<ConnectionPreview>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionPreview {
    pub source: String,
    pub label: String,
    pub initial: Option<String>,
}

impl ProfileData {
    pub fn from_profile(profile: &Profile) -> Self {
        Self {
            display_name: profile.display_name.clone(),
            avatar_url: nonempty(&profile.avatar_url),
            ..Self::default()
        }
    }

    /// Extracts the safe subset needed by Profile. The caller must not cache `session` itself.
    pub fn apply_session(&mut self, session: &SessionResponse) {
        self.ticket_fractions = ticket_fractions(session.rest.get("consumables"));
        self.apply_credits(&session.credits.usage);
    }

    pub fn apply_credits(&mut self, credits: &CreditUsage) {
        self.usage = Some(UsageSnapshot::from_credits(credits, &self.ticket_fractions));
    }

    /// Applies the two SSE usage events that carry the complete credit snapshot.
    pub fn apply_usage_event(&mut self, event: &UsageEvent) -> bool {
        let credits = match event {
            UsageEvent::Credits(credits) | UsageEvent::Plan { credits, .. } => credits,
            UsageEvent::Energy { .. } | UsageEvent::Jelly { .. } => return false,
        };
        self.apply_credits(credits);
        true
    }

    pub fn set_ticket_history_json(&mut self, value: &Value) -> serde_json::Result<()> {
        let history: TicketHistoryResponse = serde_json::from_value(value.clone())?;
        self.ticket_history = Some(history.items);
        Ok(())
    }

    pub fn set_connections_json(&mut self, value: &Value) -> serde_json::Result<()> {
        let catalog: ConnectionCatalogResponse = serde_json::from_value(value.clone())?;
        self.connections = Some(ConnectionSummary {
            enabled_count: catalog.enabled_count,
            enabled: catalog
                .items
                .into_iter()
                .filter(|item| item.connected)
                .map(|item| ConnectionPreview {
                    source: item.source,
                    label: item.label,
                    initial: item.initial.filter(|initial| !initial.trim().is_empty()),
                })
                .collect(),
        });
        Ok(())
    }
}

impl UsageSnapshot {
    fn from_credits(credits: &CreditUsage, fractions: &BTreeMap<String, u8>) -> Self {
        Self {
            allowance: credits.allowance,
            remaining: credits.remaining,
            reset_at: nonempty(&credits.reset_at),
            period: nonempty(&credits.period),
            ticket_auto_use: credits.ticket_auto_use,
            ticket_order: nonempty(&credits.ticket_order),
            tickets: ticket_counts(&credits.tickets_by_kind, fractions),
        }
    }

    /// Mirrors the captured UI definition: floor `(allowance - remaining) * 100 / allowance`,
    /// bounded to 0–100. No percentage is shown when the allowance is not positive.
    pub fn used_percent(&self) -> Option<u8> {
        used_percent(self.allowance, self.remaining)
    }

    pub fn reset_countdown(&self, now: DateTime<Utc>) -> Option<String> {
        reset_countdown(self.reset_at.as_deref(), now)
    }
}

pub fn used_percent(allowance: i64, remaining: i64) -> Option<u8> {
    if allowance <= 0 {
        return None;
    }
    let used = (i128::from(allowance) - i128::from(remaining)).clamp(0, i128::from(allowance));
    Some(((used * 100) / i128::from(allowance)) as u8)
}

pub fn reset_countdown(reset_at: Option<&str>, now: DateTime<Utc>) -> Option<String> {
    let reset = DateTime::parse_from_rfc3339(reset_at?)
        .ok()?
        .with_timezone(&Utc);
    let seconds = (reset - now).num_seconds();
    if seconds <= 0 {
        return Some("now".to_owned());
    }
    if seconds < 60 {
        return Some("<1m".to_owned());
    }
    let minutes = seconds / 60;
    let days = minutes / (24 * 60);
    let hours = (minutes % (24 * 60)) / 60;
    let minutes = minutes % 60;
    Some(if days > 0 {
        if hours > 0 {
            format!("{days}d {hours}h")
        } else {
            format!("{days}d")
        }
    } else if hours > 0 {
        if minutes > 0 {
            format!("{hours}h {minutes}m")
        } else {
            format!("{hours}h")
        }
    } else {
        format!("{minutes}m")
    })
}

fn ticket_counts(
    tickets_by_kind: &BTreeMap<String, i64>,
    fractions: &BTreeMap<String, u8>,
) -> TicketCounts {
    fn count(
        wanted: u8,
        tickets_by_kind: &BTreeMap<String, i64>,
        fractions: &BTreeMap<String, u8>,
    ) -> Option<i64> {
        let ids: Vec<&String> = fractions
            .iter()
            .filter_map(|(id, &fraction)| (fraction == wanted).then_some(id))
            .collect();
        (!ids.is_empty()).then(|| {
            ids.into_iter()
                .map(|id| tickets_by_kind.get(id).copied().unwrap_or(0))
                .sum()
        })
    }

    TicketCounts {
        full: count(100, tickets_by_kind, fractions),
        half: count(50, tickets_by_kind, fractions),
    }
}

fn ticket_fractions(value: Option<&Value>) -> BTreeMap<String, u8> {
    #[derive(Deserialize)]
    struct Consumables {
        #[serde(default)]
        items: Vec<Consumable>,
    }
    #[derive(Deserialize)]
    struct Consumable {
        id: String,
        #[serde(default)]
        kind: String,
        ticket_fraction_pct: Option<u8>,
    }

    value
        .cloned()
        .and_then(|value| serde_json::from_value::<Consumables>(value).ok())
        .map(|catalog| {
            catalog
                .items
                .into_iter()
                .filter(|item| item.kind == "reset_tickets")
                .filter_map(|item| item.ticket_fraction_pct.map(|pct| (item.id, pct)))
                .collect()
        })
        .unwrap_or_default()
}

fn nonempty(value: &str) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.to_owned())
}

#[derive(Deserialize)]
struct TicketHistoryResponse {
    #[serde(default)]
    items: Vec<TicketHistoryItem>,
}

#[derive(Deserialize)]
struct ConnectionCatalogResponse {
    enabled_count: Option<usize>,
    #[serde(default)]
    items: Vec<ConnectionCatalogItem>,
}

#[derive(Deserialize)]
struct ConnectionCatalogItem {
    source: String,
    label: String,
    #[serde(default)]
    initial: Option<String>,
    #[serde(default)]
    connected: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use chrono::TimeZone;

    use super::*;

    #[test]
    fn usage_percentage_is_floored_and_bounded() {
        assert_eq!(used_percent(1_200, 862), Some(28));
        assert_eq!(used_percent(100, 150), Some(0));
        assert_eq!(used_percent(100, -20), Some(100));
        assert_eq!(used_percent(0, 0), None);
    }

    #[test]
    fn reset_display_uses_compact_duration_and_preserves_unavailable() {
        let now = Utc.with_ymd_and_hms(2026, 11, 17, 2, 42, 0).unwrap();
        assert_eq!(
            reset_countdown(Some("2026-11-17T03:59:00Z"), now),
            Some("1h 17m".to_owned())
        );
        assert_eq!(reset_countdown(None, now), None);
        assert_eq!(reset_countdown(Some("not-a-date"), now), None);
    }

    #[test]
    fn ticket_counts_follow_catalog_fractions_not_ids() {
        let tickets = BTreeMap::from([("blue".to_owned(), 8), ("pink".to_owned(), 2)]);
        let fractions = BTreeMap::from([("blue".to_owned(), 100), ("pink".to_owned(), 50)]);
        assert_eq!(
            ticket_counts(&tickets, &fractions),
            TicketCounts {
                full: Some(8),
                half: Some(2)
            }
        );
    }

    #[test]
    fn known_ticket_kind_missing_from_holdings_is_zero() {
        let tickets = BTreeMap::from([("blue".to_owned(), 8)]);
        let fractions = BTreeMap::from([("blue".to_owned(), 100), ("pink".to_owned(), 50)]);
        assert_eq!(ticket_counts(&tickets, &fractions).half, Some(0));
    }

    #[test]
    fn absent_ticket_catalog_remains_unknown() {
        let tickets = BTreeMap::from([("server-defined".to_owned(), 8)]);
        assert_eq!(
            ticket_counts(&tickets, &BTreeMap::new()),
            TicketCounts::default()
        );
    }

    #[test]
    fn history_distinguishes_empty_from_unavailable() {
        let mut profile = ProfileData::default();
        assert_eq!(profile.ticket_history, None);
        profile
            .set_ticket_history_json(&serde_json::json!({"kind":"ticket-history","items":[]}))
            .unwrap();
        assert_eq!(profile.ticket_history, Some(Vec::new()));
    }

    #[test]
    fn connection_count_preserves_null_and_zero() {
        let mut profile = ProfileData::default();
        profile
            .set_connections_json(&serde_json::json!({"enabled_count":null,"items":[]}))
            .unwrap();
        assert_eq!(profile.connections.unwrap().enabled_count, None);

        let mut profile = ProfileData::default();
        profile
            .set_connections_json(&serde_json::json!({"enabled_count":0,"items":[]}))
            .unwrap();
        assert_eq!(profile.connections.unwrap().enabled_count, Some(0));
    }
}
