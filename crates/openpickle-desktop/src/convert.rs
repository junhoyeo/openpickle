//! Turns what the API sends into what the chat screen draws.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use openpickle_api::{
    CardState, ExploreSectionsScreen, FriendRequest, HistoryItem, InviteMember, MessageSearchItem,
    SocialSnapshot, UserSummary,
};

use crate::{
    content::{Block, parse_message, plain_text},
    model::{
        Explore, Invite, Message, Offer, OfferSection, Part, Person, Request, RequestOption,
        RequestStatus, SearchResult, Side, Social,
    },
};

/// The name of a picture in the cache: its address without the signature, which changes with
/// every answer of the server.
pub fn picture_key(url: &str) -> &str {
    url.split('?').next().unwrap_or_default()
}

fn person(user: &UserSummary, note: String, pictures: &mut Vec<String>) -> Person {
    // Lists show the character, as the app does, and fall back to the person's own picture.
    let picture = match user.character_avatar_url.is_empty() {
        true => &user.avatar_url,
        false => &user.character_avatar_url,
    };
    if !picture.is_empty() {
        pictures.push(picture.clone());
    }
    Person {
        id: user.user_id.clone(),
        name: user.display_name.clone(),
        picture: picture_key(picture).to_owned(),
        note,
    }
}

/// `Joined today`, `Joined 1 day ago`, `Joined 12 days ago`; empty when the date is not one.
fn joined(at: &str, now: DateTime<Utc>) -> String {
    let Ok(at) = DateTime::parse_from_rfc3339(at) else {
        return String::new();
    };
    match (now - at.with_timezone(&Utc)).num_days() {
        ..=0 => "Joined today".to_owned(),
        1 => "Joined 1 day ago".to_owned(),
        days => format!("Joined {days} days ago"),
    }
}

/// The friends screen and the invite sheet, from the social snapshot. Also returns the
/// addresses of the pictures they show.
pub fn social(snapshot: &SocialSnapshot, now: DateTime<Utc>) -> (Social, Vec<String>) {
    let mut pictures = Vec::new();
    let scopes = &snapshot.scopes;
    let mut members = |members: &[InviteMember]| -> Vec<Person> {
        members
            .iter()
            .map(|member| {
                person(
                    &member.user.user,
                    joined(&member.joined_at, now),
                    &mut pictures,
                )
            })
            .collect()
    };
    let invite = scopes.invite.as_ref().map(|invite| &invite.data);
    let invite = Invite {
        url: invite.map(|invite| invite.url.clone()).unwrap_or_default(),
        left: invite.map_or(0, |invite| {
            (invite.invites_total - invite.invites_used).max(0)
        }),
        joined: invite
            .map(|invite| members(&invite.signups))
            .unwrap_or_default(),
        inviters: invite
            .map(|invite| members(&invite.inviters))
            .unwrap_or_default(),
    };
    let friends = scopes.friends.as_ref().map(|friends| &friends.data.friends);
    let friends = friends
        .into_iter()
        .flatten()
        .map(|friend| person(&friend.user, String::new(), &mut pictures))
        .collect();
    // No request to you was ever captured. One that reads like a request from you is shown.
    let requests = scopes
        .requests
        .as_ref()
        .map(|requests| &requests.data.incoming);
    let requests = requests
        .into_iter()
        .flatten()
        .filter_map(|request| serde_json::from_value::<FriendRequest>(request.clone()).ok())
        .map(|request| person(&request.user, String::new(), &mut pictures))
        .collect();
    let blocked = scopes
        .blocked
        .as_ref()
        .map_or(0, |blocked| blocked.data.blocked.len());
    let social = Social {
        friends,
        requests,
        blocked,
        invite,
    };
    (social, pictures)
}

/// The Explore tab, from the server's sections. Also returns the addresses of the pictures.
pub fn explore(screen: ExploreSectionsScreen) -> (Explore, Vec<String>) {
    let mut pictures = Vec::new();
    // The ranked list comes without the steps, which the offer's sheet shows.
    let steps: std::collections::HashMap<String, Vec<String>> = screen
        .sections
        .iter()
        .flat_map(|section| &section.items)
        .chain(&screen.more_items)
        .filter(|offer| !offer.steps.is_empty())
        .map(|offer| (offer.offer_id.clone(), offer.steps.clone()))
        .collect();
    let mut offer = |offer: openpickle_api::Offer, section: &str| {
        if !offer.icon_image.is_empty() {
            pictures.push(offer.icon_image.clone());
        }
        let mut areas = offer.areas;
        if !areas.contains(&offer.area) {
            areas.push(offer.area);
        }
        Arc::new(Offer {
            section: offer.section_id.unwrap_or_else(|| section.to_owned()),
            steps: match offer.steps.is_empty() {
                true => steps.get(&offer.offer_id).cloned().unwrap_or_default(),
                false => offer.steps,
            },
            id: offer.offer_id,
            title: offer.title,
            detail: offer.detail,
            ask: offer.cta_label,
            picture: picture_key(&offer.icon_image).to_owned(),
            symbol: offer.icon_symbol,
            asked_by: offer.accepted_users_label,
            areas,
        })
    };
    let sections = screen
        .sections
        .into_iter()
        .map(|section| {
            let id = section.section_id;
            let count = section.items.len();
            OfferSection {
                offers: section
                    .items
                    .into_iter()
                    .map(|item| offer(item, &id))
                    .collect(),
                preview: usize::try_from(section.preview_count)
                    .unwrap_or(count)
                    .min(count),
                expandable: section.expand_count > 0,
                title: section.title,
                id,
            }
        })
        .collect();
    let explore = Explore {
        areas: screen.areas,
        popular: screen.popular_ids,
        sections,
        more: screen
            .more_items
            .into_iter()
            .map(|item| offer(item, ""))
            .collect(),
        ranked: screen
            .ranked_items
            .into_iter()
            .map(|item| offer(item, ""))
            .collect(),
    };
    (explore, pictures)
}

/// A search result as the search screen draws it. `room_id` is the open conversation: only a
/// message in it can be opened. The server names that room with an empty id.
pub fn search_result(item: MessageSearchItem, room_id: &str) -> SearchResult {
    let here = item.kind == "chat" && (item.room_id.is_empty() || item.room_id == room_id);
    let mut snippet = String::new();
    let mut matches = Vec::new();
    for segment in &item.segments {
        let start = snippet.len();
        // The snippet is drawn on one line.
        snippet.extend(
            segment
                .text
                .chars()
                .map(|c| if c.is_control() { ' ' } else { c }),
        );
        if segment.is_match {
            matches.push(start..snippet.len());
        }
    }
    SearchResult {
        seq: u64::try_from(item.seq).ok().filter(|seq| here && *seq > 0),
        label: item.label,
        date: item.date_label,
        snippet,
        matches,
    }
}

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
    fn the_social_snapshot_becomes_the_friends_and_invite_lists() {
        let user = |id: &str, name: &str| {
            serde_json::json!({
                "user_id": id, "handle": id, "display_name": name, "character_name": "",
                "avatar_url": format!("https://img.example/{id}.jpg?sig=1"),
                "character_avatar_url": format!("https://img.example/{id}-character.png?sig=2"),
            })
        };
        let friend = |id: &str, name: &str| {
            let mut friend = user(id, name);
            friend["since"] = "2026-11-01T00:00:00Z".into();
            friend["relation_label"] = "".into();
            friend["speech_style"] = "".into();
            friend
        };
        let mut joined_user = user("u3", "Gyu");
        joined_user["character_level"] = 3.into();
        joined_user["joined_at"] = "2026-11-10T09:00:00Z".into();
        let snapshot: SocialSnapshot = serde_json::from_value(serde_json::json!({
            "v": 1,
            "scopes": {
                "friends": {"status": 200, "data": {
                    "friends": [friend("u1", "Mina"), friend("u2", "Jun")],
                    "birthdays_this_week": [], "friends_reward_granted": false,
                }},
                "requests": {"status": 200, "data": {"incoming": [{"unexpected": true}], "outgoing": []}},
                "blocked": {"status": 200, "data": {"blocked": [{}]}},
                "invite": {"status": 200, "data": {
                    "invite_only": true, "url": "https://pickle.example/i/abc", "code": "abc",
                    "invites_used": 4, "invites_total": 3,
                    "signups": [{"user": joined_user, "joined_at": "2026-11-13T09:00:00Z"}],
                    "inviters": [],
                }},
            },
        }))
        .unwrap();
        let now = DateTime::parse_from_rfc3339("2026-11-16T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (social, pictures) = social(&snapshot, now);

        assert_eq!(
            social
                .friends
                .iter()
                .map(|friend| friend.name.as_str())
                .collect::<Vec<_>>(),
            ["Mina", "Jun"]
        );
        // The picture is the character's, named without its signature; the address keeps it.
        assert_eq!(
            social.friends[0].picture,
            "https://img.example/u1-character.png"
        );
        assert!(pictures.contains(&"https://img.example/u1-character.png?sig=2".to_owned()));
        // More invites used than given is none left, not a negative number.
        assert_eq!(
            (social.invite.left, social.invite.joined[0].note.as_str()),
            (0, "Joined 3 days ago")
        );
        // A request of a shape that was never seen is left out; the rest still shows.
        assert_eq!((social.requests.len(), social.blocked), (0, 1));
    }

    #[test]
    fn explore_offers_keep_their_section_and_get_their_steps_back() {
        let offer = |id: &str, steps: &[&str]| {
            serde_json::json!({
                "offer_id": id, "title": "제목", "detail": "설명", "cta_label": "해줘",
                "icon_image": format!("https://img.example/{id}.png"), "icon_symbol": "house",
                "area": "home", "areas": ["money"], "accepted_users_label": "", "steps": steps,
            })
        };
        let mut ranked = offer("a", &[]);
        ranked["section_id"] = "money_back".into();
        ranked.as_object_mut().unwrap().remove("steps");
        let screen: ExploreSectionsScreen = serde_json::from_value(serde_json::json!({
            "kind": "explore-sections", "areas": ["money", "home"], "popular_ids": ["a"],
            "sections": [{
                "section_id": "money_back", "title": "받을 돈", "areas": ["money"],
                "items": [offer("a", &["하나", "둘"]), offer("b", &["셋"])],
                "preview_count": 1, "expand_count": 1,
            }],
            "more_items": [], "ranked_items": [ranked],
        }))
        .unwrap();
        let (explore, pictures) = explore(screen);

        let section = &explore.sections[0];
        assert_eq!(
            (
                section.preview,
                section.expandable,
                section.offers[1].section.as_str()
            ),
            (1, true, "money_back")
        );
        // The ranked copy of an offer has no steps of its own.
        assert_eq!(explore.ranked[0].steps, ["하나", "둘"]);
        // An offer is listed under its own area as well as under the others.
        assert_eq!(explore.ranked[0].areas, ["money", "home"]);
        assert_eq!(pictures.len(), 3);
    }

    #[test]
    fn a_search_result_is_one_line_with_its_matches_and_opens_only_in_this_room() {
        let found =
            |json: serde_json::Value| search_result(serde_json::from_value(json).unwrap(), "room");
        let result = found(serde_json::json!({
            "id": "chat:42", "kind": "chat", "label": "Chat", "date_label": "Nov 16",
            "room_id": "", "call_id": "", "seq": 42, "line_id": "",
            "segments": [
                {"text": "...내일\n", "match": false},
                {"text": "비", "match": true},
                {"text": " 온대요", "match": false},
            ],
        }));
        assert_eq!(result.snippet, "...내일 비 온대요");
        assert_eq!(&result.snippet[result.matches[0].clone()], "비");
        assert_eq!((result.seq, result.date.as_str()), (Some(42), "Nov 16"));

        // Results from another room, or of another kind, are shown but cannot be opened.
        let elsewhere = serde_json::json!({
            "id": "chat:7", "kind": "chat", "label": "Chat", "date_label": "Nov 2",
            "room_id": "other", "call_id": "", "seq": 7, "line_id": "", "segments": [],
        });
        assert_eq!(found(elsewhere).seq, None);
        let call = serde_json::json!({
            "id": "call:7", "kind": "call", "label": "Call", "date_label": "Nov 2", "segments": [],
        });
        assert_eq!(found(call).seq, None);
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
