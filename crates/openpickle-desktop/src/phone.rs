//! Small, cache-safe models for the phone-call list.

use std::collections::HashMap;

use openpickle_api::{ExtraConversationsScreen, HandoffItem, HandoffsScreen, PhoneCallTranscript};
use serde::{Deserialize, Serialize};

/// One phone call as it appears in the phone history.
///
/// All labels are server-owned display text. `state` is intentionally an open string because
/// the service may add call states without requiring a desktop release.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallSummary {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub state: String,
    pub at: String,
}

impl CallSummary {
    /// Combines the call-bearing handoffs with the phone conversation history.
    ///
    /// Handoffs come first because they carry the current server state. Extra conversations fill
    /// missing labels and add older calls. A call id appears at most once in the result.
    pub fn from_screens(handoffs: &HandoffsScreen, extras: &ExtraConversationsScreen) -> Vec<Self> {
        let mut calls = Vec::new();
        let mut positions = HashMap::<String, usize>::new();

        for item in handoffs
            .active_items
            .iter()
            .chain(&handoffs.recent_items)
            .chain(&handoffs.items)
        {
            let Some(call) = Self::from_handoff(item) else {
                continue;
            };
            if positions.contains_key(&call.id) {
                continue;
            }
            positions.insert(call.id.clone(), calls.len());
            calls.push(call);
        }

        for item in extras.items.iter().filter(|item| item.kind == "phone") {
            let id = item.call_id.trim();
            if id.is_empty() {
                continue;
            }
            if let Some(&position) = positions.get(id) {
                let call = &mut calls[position];
                if call.title.trim().is_empty() {
                    call.title.clone_from(&item.title);
                }
                if call.subtitle.trim().is_empty() {
                    call.subtitle.clone_from(&item.subtitle);
                }
                if call.at.trim().is_empty() {
                    call.at.clone_from(&item.date_label);
                }
                continue;
            }
            positions.insert(id.to_owned(), calls.len());
            calls.push(Self {
                id: id.to_owned(),
                title: item.title.clone(),
                subtitle: item.subtitle.clone(),
                state: String::new(),
                at: item.date_label.clone(),
            });
        }

        calls
    }

    fn from_handoff(item: &HandoffItem) -> Option<Self> {
        let id = item.phone_call_id.as_deref()?.trim();
        if id.is_empty() {
            return None;
        }
        let subtitle = item
            .latest_log_text
            .as_deref()
            .and_then(nonempty)
            .or_else(|| item.subtitle.as_deref().and_then(nonempty))
            .or_else(|| item.result_text.as_deref().and_then(nonempty))
            .unwrap_or_default();
        let state = item
            .phone_status_text
            .as_deref()
            .and_then(nonempty)
            .unwrap_or(&item.state);
        Some(Self {
            id: id.to_owned(),
            title: if item.title.trim().is_empty() {
                item.name.clone()
            } else {
                item.title.clone()
            },
            subtitle: subtitle.to_owned(),
            state: state.to_owned(),
            at: item.time_text.clone(),
        })
    }
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

/// The time on this computer, in the unit of the server's call times.
pub fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0., |since| since.as_millis() as f64)
}

/// How long a call lasted, as `1:36`: what the server counted once it ended, the time since
/// the answer while it is live. A call nobody answered has no duration.
pub fn duration_label(call: &PhoneCallTranscript, now_ms: f64) -> Option<String> {
    let seconds = match call.duration_s {
        Some(seconds) => seconds,
        None if call.live => ((now_ms - call.answered_at_epoch_ms?) / 1000.) as i64,
        None => return None,
    }
    .max(0);
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    Some(if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn call(fields: serde_json::Value) -> PhoneCallTranscript {
        let mut call = json!({
            "kind": "phone_call", "call_id": "sample", "title": "A call",
            "subtitle_lead": "", "counterpart_label": "", "subtitle_tail": "",
            "started_label": "", "live": false, "hangup_requested": false,
            "footer": "", "lines": [],
        });
        call.as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        serde_json::from_value(call).unwrap()
    }

    #[test]
    fn an_ended_call_lasted_what_the_server_counted() {
        let ended = call(json!({"duration_s": 96, "answered_at_epoch_ms": 1000.0}));
        assert_eq!(duration_label(&ended, 9_999_000.).as_deref(), Some("1:36"));
        let long = call(json!({"duration_s": 3725}));
        assert_eq!(duration_label(&long, 0.).as_deref(), Some("1:02:05"));
    }

    #[test]
    fn a_live_call_counts_from_its_answer_and_an_unanswered_one_has_no_duration() {
        let live = call(json!({"live": true, "answered_at_epoch_ms": 10_000.0}));
        assert_eq!(duration_label(&live, 75_900.).as_deref(), Some("1:05"));
        let ringing = call(json!({"live": true}));
        assert_eq!(duration_label(&ringing, 75_900.), None);
        let missed = call(json!({"live": false}));
        assert_eq!(duration_label(&missed, 75_900.), None);
    }
}
