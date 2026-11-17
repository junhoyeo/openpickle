//! Small, cache-safe models for the phone-call list.

use std::collections::HashMap;

use openpickle_api::{ExtraConversationsScreen, HandoffItem, HandoffsScreen};
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
