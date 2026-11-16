//! Turns what the API sends into what the chat screen draws.

use std::sync::Arc;

use openpickle_api::{CardState, HistoryItem};

use crate::{
    content::{Block, parse_message, plain_text},
    model::{Message, Part, Request, RequestOption, RequestStatus, Side},
};

/// The message for a history item. `card_state` is the latest state of the item's request card,
/// when it is one and has been answered or has expired.
pub fn message(item: &HistoryItem, card_state: Option<&CardState>) -> Message {
    let text = item.text.as_deref().unwrap_or_default();
    let (side, parts) = match item.role.as_str() {
        "user" => (Side::Mine, bubbles(text, false)),
        "agent_request" => (
            Side::Theirs,
            request(item, card_state).into_iter().collect(),
        ),
        "subagent" => (
            Side::Theirs,
            item.title
                .iter()
                .map(|title| Part::Task(title.as_str().into()))
                .collect(),
        ),
        // In a room between two characters, yours speaks on your side.
        _ if item.mine == Some(true) => (Side::Mine, bubbles(text, true)),
        _ => (Side::Theirs, bubbles(text, true)),
    };
    let mut parts = parts;
    // Pictures and files cannot be downloaded yet, so a message says how many it carries.
    match item.attachments.as_ref().map_or(0, Vec::len) {
        0 => {}
        1 => parts.push(Part::Task("1 attachment".into())),
        count => parts.push(Part::Task(format!("{count} attachments").into())),
    }
    Message {
        id: item.id.clone(),
        seq: item.seq.max(0) as u64,
        at: item.at,
        side,
        parts,
    }
}

/// A message that was typed here and is on its way to the server.
pub fn outgoing(id: String, text: &str, seq: u64, at: f64) -> Message {
    Message {
        id,
        seq,
        at,
        side: Side::Mine,
        parts: bubbles(text, false),
    }
}

fn bubbles(text: &str, markdown: bool) -> Vec<Part> {
    if !markdown {
        return if text.trim().is_empty() {
            Vec::new()
        } else {
            vec![Part::Bubble(Arc::new(plain_text(text)))]
        };
    }
    parse_message(text)
        .into_iter()
        .map(|block| match block {
            Block::Bubble(text) => Part::Bubble(Arc::new(text)),
            Block::Card(card) => Part::Card(Arc::new(card)),
        })
        .collect()
}

fn request(item: &HistoryItem, card_state: Option<&CardState>) -> Option<Part> {
    let request = item.request.as_ref()?;
    let status = match card_state.map(|state| state.state.as_str()) {
        Some("answered") => RequestStatus::Answered,
        // `expired`, `superseded`: either way it can no longer be answered.
        Some(_) => RequestStatus::Expired,
        None => RequestStatus::Pending,
    };
    let chosen = card_state.and_then(|state| state.option_id.as_deref());
    let caption = match (request.kind.as_str(), &request.service) {
        ("otp", Some(service)) => format!("{service} verification code"),
        ("otp", None) => "Verification code".to_owned(),
        ("choice", _) => "Pickle asks".to_owned(),
        _ => "Pickle needs your input".to_owned(),
    };
    let options = request
        .options
        .iter()
        .flatten()
        .map(|option| RequestOption {
            label: option.label.clone(),
            chosen: chosen == Some(option.id.as_str()),
        })
        .collect();
    Some(Part::Request(Arc::new(Request {
        caption,
        question: request.question.clone(),
        options,
        status,
    })))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: serde_json::Value) -> HistoryItem {
        serde_json::from_value(json).unwrap()
    }

    #[test]
    fn roles_map_to_sides_and_parts() {
        let user = message(
            &item(
                serde_json::json!({"role": "user", "id": "u", "text": "**그대로**", "at": 1.0, "seq": 3}),
            ),
            None,
        );
        assert_eq!(user.side, Side::Mine);
        assert!(matches!(&user.parts[..], [Part::Bubble(text)] if text.text == "**그대로**"));

        let reply = message(
            &item(
                serde_json::json!({"role": "assistant", "id": "a", "text": "<bubble>하나</bubble>\n```card\n## 제목\n```", "at": 2.0, "seq": 4}),
            ),
            None,
        );
        assert_eq!(reply.side, Side::Theirs);
        assert!(matches!(&reply.parts[..], [Part::Bubble(_), Part::Card(_)]));

        let mine = message(
            &item(
                serde_json::json!({"role": "assistant", "id": "c", "text": "안녕", "at": 2.0, "seq": 5, "mine": true}),
            ),
            None,
        );
        assert_eq!(mine.side, Side::Mine);

        let task = message(
            &item(
                serde_json::json!({"role": "subagent", "id": "t", "title": "📞 전화", "path": "p", "at": 2.0, "seq": 6}),
            ),
            None,
        );
        assert!(matches!(&task.parts[..], [Part::Task(title)] if &**title == "📞 전화"));
    }

    #[test]
    fn a_picture_without_text_still_shows_up() {
        let photo = message(
            &item(
                serde_json::json!({"role": "user", "id": "p", "at": 1.0, "seq": 7, "attachments": ["attachments/2026-01-01/a.jpg", "attachments/2026-01-01/b.jpg"]}),
            ),
            None,
        );
        assert!(matches!(&photo.parts[..], [Part::Task(note)] if &**note == "2 attachments"));
    }

    #[test]
    fn request_cards_follow_their_card_state() {
        let choice = item(serde_json::json!({
            "role": "agent_request", "id": "r", "at": 1.0, "seq": 9, "state": "pending",
            "request": {"kind": "choice", "question": "어느 쪽?", "options": [{"id": "a", "label": "A"}, {"id": "b", "label": "B"}]}
        }));
        let pending = message(&choice, None);
        assert!(
            matches!(&pending.parts[..], [Part::Request(request)] if request.status == RequestStatus::Pending)
        );

        let state: CardState = serde_json::from_value(
            serde_json::json!({"seq": 10, "id": "r", "state": "answered", "option_id": "b"}),
        )
        .unwrap();
        let answered = message(&choice, Some(&state));
        let [Part::Request(request)] = &answered.parts[..] else {
            panic!()
        };
        assert_eq!(request.status, RequestStatus::Answered);
        assert_eq!(
            request
                .options
                .iter()
                .map(|option| option.chosen)
                .collect::<Vec<_>>(),
            [false, true]
        );

        let otp = item(
            serde_json::json!({"role": "agent_request", "id": "o", "at": 1.0, "seq": 11, "request": {"kind": "otp", "service": "샘플"}}),
        );
        let expired: CardState =
            serde_json::from_value(serde_json::json!({"seq": 12, "id": "o", "state": "expired"}))
                .unwrap();
        let [Part::Request(request)] = &message(&otp, Some(&expired)).parts[..] else {
            panic!()
        };
        assert_eq!(
            (request.caption.as_str(), request.status),
            ("샘플 verification code", RequestStatus::Expired)
        );
    }
}
