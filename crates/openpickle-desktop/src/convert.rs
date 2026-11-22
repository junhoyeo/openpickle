//! Turns what the API sends into what the chat screen draws.

use std::{
    collections::{BTreeMap, HashSet},
    sync::Arc,
};

use chrono::{DateTime, Utc};
use openpickle_api::{
    CardState, ExploreSectionsScreen, FileMeta, FriendRequest, HistoryItem, InviteMember,
    MessageSearchItem, Reaction, ReplyTo, SocialSnapshot, UserSummary,
};
use unicode_segmentation::UnicodeSegmentation;

use crate::{
    content::{Block, Card, CardRow, LinkTarget, RichText, SpanKind, parse_message, plain_text},
    model::{
        Attachment, AttachmentKind, Explore, Files, Invite, Message, MessageDecorations, Offer,
        OfferSection, Part, Person, Quote, ReactionBadge, Request, RequestOption, RequestStatus,
        RoomInfo, SearchResult, Side, Social,
    },
};

/// The pictures and files a history item carries: what you sent (`attachments`), and what the
/// character put in its reply (`inline_images`, `inline_files`, whose values are attachment
/// paths). The item's `image_meta` and `file_meta` say which is which.
pub fn attachments(item: &HistoryItem) -> Vec<Attachment> {
    let sent = item.attachments.iter().flatten();
    let images = item.inline_images.iter().flat_map(|images| images.values());
    let files = item.inline_files.iter().flat_map(|files| files.values());
    let mut paths: Vec<&String> = sent.chain(images).chain(files).collect();
    paths.dedup();
    paths
        .into_iter()
        .map(|path| {
            let file = item.file_meta.as_ref().and_then(|meta| meta.get(path));
            let picture = item
                .image_meta
                .as_ref()
                .is_some_and(|meta| meta.contains_key(path));
            let name = match file {
                Some(file) if !file.name.is_empty() => file.name.clone(),
                _ => path.rsplit('/').next().unwrap_or(path).to_owned(),
            };
            let picture = picture || (file.is_none() && is_picture_name(&name));
            Attachment {
                path: path.clone(),
                seq: item.seq.max(0) as u64,
                at: item.at,
                detail: file.map(file_detail).unwrap_or_default(),
                name,
                kind: if picture {
                    AttachmentKind::Picture
                } else {
                    AttachmentKind::File
                },
            }
        })
        .collect()
}

fn is_picture_name(name: &str) -> bool {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase());
    matches!(
        extension.as_deref(),
        Some("jpg" | "jpeg" | "png" | "gif" | "heic" | "webp")
    )
}

/// `PDF · 12 pages · 1.2 MB`.
fn file_detail(file: &FileMeta) -> String {
    let mut detail = Vec::new();
    if let Some((_, extension)) = file.name.rsplit_once('.') {
        detail.push(extension.to_ascii_uppercase());
    }
    match file.pages {
        Some(1) => detail.push("1 page".to_owned()),
        Some(pages) if pages > 1 => detail.push(format!("{pages} pages")),
        _ => {}
    }
    let size = file.size.max(0) as f64;
    detail.push(match size {
        _ if size < 1024. => format!("{size} B"),
        _ if size < 1024. * 1024. => format!("{:.0} KB", size / 1024.),
        _ => format!("{:.1} MB", size / (1024. * 1024.)),
    });
    detail.join(" · ")
}

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

/// The chat rooms other than the one with your character: a friend's Pickle each. Also
/// returns the addresses of their pictures.
pub fn rooms(rooms: &[openpickle_api::Room]) -> (Vec<RoomInfo>, Vec<String>) {
    let mut pictures = Vec::new();
    let infos = rooms
        .iter()
        .filter(|room| room.kind != "character")
        .map(|room| {
            let picture = room.peer.as_ref().map_or("", |peer| {
                if peer.character_avatar_url.is_empty() {
                    &peer.avatar_url
                } else {
                    &peer.character_avatar_url
                }
            });
            if !picture.is_empty() {
                pictures.push(picture.to_owned());
            }
            RoomInfo {
                room_id: room.room_id.clone(),
                title: if !room.title.is_empty() {
                    room.title.clone()
                } else {
                    room.peer
                        .as_ref()
                        .map(|peer| peer.display_name.as_str())
                        .filter(|name| !name.is_empty())
                        .unwrap_or("Conversation")
                        .to_owned()
                },
                picture: picture_key(picture).to_owned(),
                observer: room.observer,
            }
        })
        .collect();
    (infos, pictures)
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
    message_with_reactions(item, card_state, std::iter::empty())
}

pub(crate) fn message_with_reactions<'a>(
    item: &'a HistoryItem,
    card_state: Option<&CardState>,
    reactions: impl IntoIterator<Item = &'a Reaction>,
) -> Message {
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
                .map(|title| {
                    let task = item
                        .path
                        .as_ref()
                        .and_then(|path| LinkTarget::parse(&format!("handoff:{path}")));
                    match task {
                        Some(LinkTarget::PhoneTask(path)) => Part::PhoneTask {
                            title: title.as_str().into(),
                            path,
                        },
                        _ => Part::Task(title.as_str().into()),
                    }
                })
                .collect(),
        ),
        // In a room between two characters, yours speaks on your side.
        _ if item.mine == Some(true) => (Side::Mine, bubbles(text, true)),
        _ => (Side::Theirs, bubbles(text, true)),
    };
    let mut parts = parts;
    // Only offer replies when desktop rows map to the message's text bubble indices.
    // Cards, files and other mixed-content messages have a different part layout.
    let reply_role = match item.role.as_str() {
        "user" if matches!(parts.as_slice(), [Part::Bubble(_)]) => Some("user"),
        "assistant"
            if !parts.is_empty() && parts.iter().all(|part| matches!(part, Part::Bubble(_))) =>
        {
            let tagged = text.matches("<bubble>").count();
            (tagged == parts.len() || (tagged == 0 && parts.len() == 1)).then_some("assistant")
        }
        _ => None,
    };
    // Only character-authored Markdown is normalized. Literal links in user messages stay as
    // typed; the message's own maps are the sole authority for attachment aliases.
    if item.role != "user" {
        normalize_part_links(&mut parts, item);
    }
    // A chat-room link the character put in a card fence. The app names it in
    // `app_references`; the members of the card say whose Pickle it is.
    for block in std::mem::take(&mut parts) {
        match block {
            // Resolve every card independently. Map iteration order need not match card order.
            Part::RoomLink {
                room_id,
                label,
                picture: _,
            } if label.is_empty() => {
                let member = item
                    .app_reference_cards
                    .iter()
                    .flatten()
                    .find(|(target, _)| room_target(target) == Some(room_id.as_str()))
                    .and_then(|(_, card)| card.members.as_deref()?.first());
                let label = item
                    .app_references
                    .iter()
                    .flatten()
                    .find(|(target, _)| room_target(target) == Some(room_id.as_str()))
                    .map(|(_, label)| label.clone())
                    .unwrap_or_else(|| {
                        member
                            .map(|member| format!("{}'s Pickle", member.profile.display_name))
                            .unwrap_or_else(|| "Open chat room".to_owned())
                    });
                let picture = member
                    .map(|member| member.profile.character_avatar_url.as_str())
                    .filter(|url| !url.is_empty())
                    .unwrap_or("");
                parts.push(Part::RoomLink {
                    room_id,
                    label,
                    picture: picture_key(picture).to_owned(),
                });
            }
            other => parts.push(other),
        }
    }
    // Pictures and files the message carries, as the app shows them.
    let carried = attachments(item);
    // `parts` contains offsets; a reply's `part` is the ordinal into that list.
    // If the server's part count differs from how we split the text, do not guess.
    let reply_role = reply_role.filter(|_| {
        carried.is_empty()
            && item
                .parts
                .as_ref()
                .is_none_or(|offsets| offsets.len() == parts.len())
    });
    if !carried.is_empty() {
        parts.push(Part::Files(Arc::new(Files { items: carried })));
    }
    Message {
        id: item.id.clone(),
        seq: item.seq.max(0) as u64,
        at: item.at,
        side,
        parts,
        reply_role,
        decorations: MessageDecorations {
            reply_target: None,
            quote: item
                .reply_to
                .as_ref()
                .map(|reply| Arc::new(quoted_reply(reply))),
            reactions: reaction_badges(item, reactions),
        },
    }
}

pub(crate) fn quoted_reply(reply: &ReplyTo) -> Quote {
    const SNIPPET_GRAPHEMES: usize = 120;
    let author = match reply.role.as_str() {
        "user" => Some("User"),
        "assistant" | "agent_request" => Some("Pickle"),
        _ => None,
    };
    let mut snippet = reply
        .text
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if let Some((end, _)) = snippet.grapheme_indices(true).nth(SNIPPET_GRAPHEMES) {
        snippet.truncate(end);
        snippet.push('…');
    }
    Quote { author, snippet }
}

fn reaction_badges<'a>(
    item: &'a HistoryItem,
    reactions: impl IntoIterator<Item = &'a Reaction>,
) -> Option<Arc<[ReactionBadge]>> {
    let inline = item.reactions.iter().flatten().map(|reaction| {
        (
            reaction.part,
            reaction.actor.as_str(),
            reaction.emoji.as_str(),
        )
    });
    let separate = reactions
        .into_iter()
        .filter(|reaction| reaction.target_id == item.id)
        .map(|reaction| {
            (
                reaction.part,
                reaction.actor.as_str(),
                reaction.emoji.as_str(),
            )
        });
    // The reaction names a wire part; Markdown parsing may split one wire part into several
    // desktop rows. Aggregate across the message without guessing a particular row or an
    // add/remove/replace operation.
    let mut seen = HashSet::new();
    let mut counts = BTreeMap::new();
    for (part, actor, emoji) in inline.chain(separate) {
        if !actor.trim().is_empty()
            && !emoji.trim().is_empty()
            && !emoji.chars().any(char::is_control)
            && seen.insert((part, actor, emoji))
        {
            *counts.entry(emoji).or_default() += 1;
        }
    }
    (!counts.is_empty()).then(|| {
        counts
            .into_iter()
            .map(|(emoji, count)| ReactionBadge {
                emoji: emoji.to_owned(),
                count,
            })
            .collect()
    })
}

fn room_target(target: &str) -> Option<&str> {
    match LinkTarget::parse(target) {
        Some(LinkTarget::Room(room_id)) => {
            // The returned id must borrow `target`, not the temporary parsed value.
            let query = target.strip_prefix("pickle://chat?")?;
            query.split('&').find_map(|parameter| {
                let (name, value) = parameter.split_once('=')?;
                (name == "room_id" && value == room_id).then_some(value)
            })
        }
        _ => None,
    }
}

fn normalize_part_links(parts: &mut [Part], item: &HistoryItem) {
    for part in parts {
        match part {
            Part::Bubble(text) => normalize_rich_links(Arc::make_mut(text), item),
            Part::Card(card) => normalize_card_links(Arc::make_mut(card), item),
            _ => {}
        }
    }
}

fn normalize_card_links(card: &mut Card, item: &HistoryItem) {
    card.title
        .iter_mut()
        .chain(&mut card.subtitle)
        .for_each(|text| normalize_rich_links(text, item));
    for row in &mut card.rows {
        match row {
            CardRow::Field { label, value } => {
                normalize_rich_links(label, item);
                normalize_rich_links(value, item);
            }
            CardRow::Heading(text) | CardRow::Text(text) => normalize_rich_links(text, item),
            CardRow::Table { cells, .. } => cells
                .iter_mut()
                .for_each(|text| normalize_rich_links(text, item)),
        }
    }
}

fn normalize_rich_links(text: &mut RichText, item: &HistoryItem) {
    for span in &mut text.spans {
        let SpanKind::Link(target) = &mut span.kind else {
            continue;
        };
        if let Some(normalized) = normalize_link(target, item) {
            *target = normalized;
        }
    }
}

fn normalize_link(target: &str, item: &HistoryItem) -> Option<String> {
    if let Some(path) = attachment_path(target, item) {
        return LinkTarget::attachment(path).map(LinkTarget::into_link);
    }
    match LinkTarget::parse(target) {
        Some(LinkTarget::Web(_)) => Some(target.to_owned()),
        Some(room @ LinkTarget::Room(_)) => Some(room.into_link()),
        Some(task @ LinkTarget::PhoneTask(_)) => Some(task.into_link()),
        // A literal `attachment:` target is accepted only when this message proves its path.
        Some(LinkTarget::Attachment(_)) | None => None,
    }
}

fn attachment_path<'a>(target: &str, item: &'a HistoryItem) -> Option<&'a str> {
    let unprefixed = target.strip_prefix("attachment:").unwrap_or(target);
    if !safe_path_components(unprefixed) {
        return None;
    }
    let decoded = if unprefixed.contains('%') {
        Some(percent_decode_path(unprefixed)?)
    } else {
        None
    };
    let matches = |candidate: &str| {
        candidate == unprefixed
            || decoded
                .as_deref()
                .is_some_and(|decoded| candidate == decoded)
    };
    let mapped = item
        .inline_files
        .iter()
        .flatten()
        .chain(item.inline_images.iter().flatten())
        .find_map(|(alias, path)| matches(alias).then_some(path.as_str()));
    if mapped.is_some() {
        return mapped;
    }
    item.attachments
        .iter()
        .flatten()
        .map(String::as_str)
        .chain(
            item.inline_files
                .iter()
                .flatten()
                .chain(item.inline_images.iter().flatten())
                .map(|(_, path)| path.as_str()),
        )
        .find(|path| matches(path))
}

/// Decodes one round of `%XX` bytes for exact comparison with a message-provided alias. `+`
/// stays a plus. Malformed UTF-8, malformed escapes and traversal components are rejected.
fn percent_decode_path(path: &str) -> Option<String> {
    let bytes = path.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = hex(bytes.get(index + 1).copied()?)?;
            let low = hex(bytes.get(index + 2).copied()?)?;
            decoded.push(high << 4 | low);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    let decoded = String::from_utf8(decoded).ok()?;
    safe_path_components(&decoded).then_some(decoded)
}

fn safe_path_components(path: &str) -> bool {
    path.split(['/', '\\'])
        .all(|component| component != "." && component != "..")
}

fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
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
        reply_role: None,
        decorations: Default::default(),
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
    // The room link is filled in by `message`, which reads the message's references.
    parse_message(text)
        .into_iter()
        .map(|block| match block {
            Block::RoomLink(room_id) => Part::RoomLink {
                room_id,
                label: String::new(),
                picture: String::new(),
            },
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
    fn a_message_lists_what_it_carries_as_pictures_and_files() {
        let sent = item(serde_json::json!({
            "role": "user", "id": "u", "at": 1.0, "seq": 7,
            "attachments": ["attachments/2026-11-05/IMG_0001.jpg", "attachments/2026-11-05/scan.HEIC"],
            "image_meta": {"attachments/2026-11-05/IMG_0001.jpg": {"w": 10, "h": 10, "blurhash": ""}},
        }));
        let listed = attachments(&sent);
        assert_eq!(
            listed.iter().map(|a| a.kind).collect::<Vec<_>>(),
            [AttachmentKind::Picture; 2]
        );
        assert_eq!(
            (listed[0].name.as_str(), listed[0].seq),
            ("IMG_0001.jpg", 7)
        );

        let reply = item(serde_json::json!({
            "role": "assistant", "id": "a", "at": 2.0, "seq": 8, "text": "<bubble>여기요</bubble>",
            "inline_files": {
                "downloads/보고서.pdf": "attachments/2026-11-05/보고서.pdf",
                "downloads/표.xlsx": "attachments/2026-11-05/표.xlsx",
            },
            "file_meta": {
                "attachments/2026-11-05/보고서.pdf": {"name": "보고서.pdf", "size": 1_572_864, "pages": 12},
                "attachments/2026-11-05/표.xlsx": {"name": "표.xlsx", "size": 20_480},
            },
        }));
        let listed = attachments(&reply);
        let files: Vec<(&str, &str)> = listed
            .iter()
            .map(|a| (a.name.as_str(), a.detail.as_str()))
            .collect();
        assert!(files.contains(&("보고서.pdf", "PDF · 12 pages · 1.5 MB")));
        assert!(files.contains(&("표.xlsx", "XLSX · 20 KB")));
        assert!(listed.iter().all(|a| a.kind == AttachmentKind::File));
    }

    #[test]
    fn rooms_keep_server_titles_and_include_observers_without_peers() {
        let base = |id: &str, kind: &str, title: &str, observer: bool| {
            serde_json::json!({
                "room_id": id, "kind": kind, "title": title, "peer": null,
                "member_count": 2, "last_seq": 10, "read_seq": 10, "role": "member",
                "last_event_at": "2026-11-17T00:00:00Z", "observer": observer,
                "unread": 0, "muted": false, "pinned_at": null,
                "peer_read_seq": null, "peer_read_at": null, "peer_joined_at": null,
                "request_id": "", "request_role": "", "extra_conversation_unread": false
            })
        };
        let own = base("room-own", "character", "My Pickle", false);
        let observer = base("room-observer", "character_pair", "", true);
        let mut direct = base("room-direct", "direct", "", false);
        direct["peer"] = serde_json::json!({
            "user_id": "user-mina", "handle": "mina", "display_name": "Mina",
            "avatar_url": "https://img.example/mina-user.png?sig=1",
            "character_avatar_url": "https://img.example/mina-character.png?sig=2",
            "character_name": "Sample"
        });
        let mut titled = direct.clone();
        titled["room_id"] = "room-titled".into();
        titled["title"] = "Server supplied title".into();
        let input: Vec<openpickle_api::Room> =
            serde_json::from_value(serde_json::json!([own, observer, direct, titled])).unwrap();
        let (listed, pictures) = rooms(&input);

        assert_eq!(listed.len(), 3);
        assert_eq!(
            (
                listed[0].room_id.as_str(),
                listed[0].title.as_str(),
                listed[0].observer,
                listed[0].picture.as_str(),
            ),
            ("room-observer", "Conversation", true, "")
        );
        assert_eq!(listed[1].title, "Mina");
        assert_eq!(listed[2].title, "Server supplied title");
        assert_eq!(listed[1].picture, "https://img.example/mina-character.png");
        assert_eq!(pictures.len(), 2);
        assert!(
            pictures
                .iter()
                .all(|picture| picture == "https://img.example/mina-character.png?sig=2")
        );
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
    fn replies_target_confirmed_text_bubbles_without_guessing_mixed_part_indices() {
        let second = "한".repeat(510);
        let source = item(serde_json::json!({
            "id":"assistant-1", "role":"assistant", "seq":9, "at":1.0,
            "parts":[0, 22], "text":format!("<bubble>First</bubble><bubble>{second}</bubble>")
        }));
        let message = message(&source, None);
        assert!(message.reply_target(2).is_none());
        let reply = message.reply_target(1).unwrap();
        assert_eq!(reply.target_id, "assistant-1");
        assert_eq!(reply.part, 1);
        assert_eq!(reply.role, "assistant");
        assert_eq!(reply.text, "한".repeat(500));

        let mixed = item(serde_json::json!({
            "id":"assistant-2", "role":"assistant", "seq":10, "at":1.0,
            "text":"<bubble>Report</bubble>\n```card\n- **Example** value\n```"
        }));
        assert!(super::message(&mixed, None).reply_target(0).is_none());
        assert!(
            outgoing("pending".into(), "Unconfirmed", u64::MAX, 1.0)
                .reply_target(0)
                .is_none()
        );
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
    fn reply_and_reaction_context_does_not_add_rows_or_selectable_text() {
        use crate::model::build_rows;

        let target = item(serde_json::json!({
            "role": "assistant", "id": "m", "at": 1.0, "seq": 7,
            "text": "<bubble>First **bubble**</bubble><bubble>Second bubble</bubble>",
            "reply_to": {
                "target_id": "not-in-local-history", "part": 0, "role": "user",
                "text": " \nQuoted\twords\u{0000} here "
            },
            "reactions": [{"part": 0, "actor": "user", "emoji": "👍", "at": 2.0}]
        }));
        let reaction = |seq, target_id: &str, part, actor: &str| Reaction {
            seq,
            target_id: target_id.into(),
            part,
            actor: actor.into(),
            emoji: "👍".into(),
            at: 2.,
        };
        let inline_only = message(&target, None);
        assert_eq!(
            inline_only.decorations.reactions.as_deref().unwrap()[0].count,
            1
        );
        let updated = message_with_reactions(
            &target,
            None,
            &[
                reaction(8, "m", 0, "user"), // also present inline
                reaction(9, "m", 0, "assistant"),
                reaction(10, "m", 20, "user"), // a different wire part, not a row index
                reaction(11, "different-message", 0, "assistant"),
            ],
        );
        let (rows, starts) = build_rows(&[updated], None, chrono::Local::now());
        assert_eq!(starts, [1]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].texts(), ["First bubble"]);
        assert_eq!(rows[2].texts(), ["Second bubble"]);
        assert!(rows[1].joined_below && rows[2].joined_above);
        let quote = rows[1].decorations.quote.as_deref().unwrap();
        assert_eq!(quote.author, Some("User"));
        assert_eq!(quote.snippet, "Quoted words here");
        assert!(rows[1].decorations.reactions.is_none());
        assert!(rows[2].decorations.quote.is_none());
        assert_eq!(
            rows[2].decorations.reactions.as_deref().unwrap(),
            [ReactionBadge {
                emoji: "👍".into(),
                count: 3
            }]
        );
        assert_eq!(inline_only.parts.len(), 2);
    }

    #[test]
    fn quote_snippets_keep_whole_graphemes_and_do_not_guess_an_unknown_author() {
        let reply = ReplyTo {
            target_id: "target".into(),
            part: 0,
            text: "👩🏽‍💻".repeat(130),
            role: "future-role".into(),
            kind: None,
            path: None,
        };
        let quote = quoted_reply(&reply);
        assert_eq!(quote.author, None);
        assert_eq!(quote.snippet, format!("{}…", "👩🏽‍💻".repeat(120)));
    }

    #[test]
    fn a_picture_without_text_still_shows_up() {
        let photo = message(
            &item(
                serde_json::json!({"role": "user", "id": "p", "at": 1.0, "seq": 7, "attachments": ["attachments/2026-01-01/a.jpg", "attachments/2026-01-01/b.jpg"]}),
            ),
            None,
        );
        assert!(matches!(
            &photo.parts[..],
            [Part::Files(files)] if files.items.len() == 2
        ));
    }

    #[test]
    fn known_message_links_are_normalized_without_changing_visible_text() {
        let reply = item(serde_json::json!({
            "role": "assistant", "id": "links", "at": 1.0, "seq": 11,
            "text": "<bubble>[방](pickle://chat?source=inline&room_id=room-inline) [보고서](downloads/%ED%95%9C%EA%B8%80%20%EB%B3%B4%EA%B3%A0%EC%84%9C.pdf) [웹](https://example.test/a?q=1) [로컬](file:///tmp/private.pdf)</bubble>\n```card\npickle://chat?room_id=room-z\n```\n```card\npickle://chat?source=card&room_id=room-a\n```",
            "inline_files": {
                "downloads/한글 보고서.pdf": "attachments/2026-11-17/한글 보고서.pdf"
            },
            // BTreeMap order is room-a then room-z, the reverse of the cards above.
            "app_references": {
                "pickle://chat?room_id=room-a": "Mina's Pickle",
                "pickle://chat?room_id=room-z": "Zed's Pickle"
            },
            "app_reference_cards": {
                "pickle://chat?room_id=room-a": {
                    "kind": "chat", "room_kind": "character_pair",
                    "members": [{"kind": "character", "user_id": "user-a", "handle": "mina", "display_name": "Mina",
                                 "character_name": "", "avatar_url": "", "character_avatar_url": "https://img.example/mina.png"}]
                },
                "pickle://chat?room_id=room-z": {
                    "kind": "chat", "room_kind": "character_pair",
                    "members": [{"kind": "character", "user_id": "user-z", "handle": "zed", "display_name": "Zed",
                                 "character_name": "", "avatar_url": "", "character_avatar_url": "https://img.example/zed.png"}]
                }
            }
        }));
        let reply = message(&reply, None);
        let Part::Bubble(text) = &reply.parts[0] else {
            panic!()
        };
        assert_eq!(text.text, "방 보고서 웹 로컬");
        let targets: Vec<&str> = text
            .spans
            .iter()
            .filter_map(|span| match &span.kind {
                SpanKind::Link(target) => Some(target.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            targets,
            [
                "pickle://chat?room_id=room-inline",
                "attachment:attachments/2026-11-17/한글 보고서.pdf",
                "https://example.test/a?q=1",
                "file:///tmp/private.pdf",
            ]
        );
        assert!(matches!(
            &reply.parts[1],
            Part::RoomLink { room_id, label, picture }
                if room_id == "room-z" && label == "Zed's Pickle"
                    && picture == "https://img.example/zed.png"
        ));
        assert!(matches!(
            &reply.parts[2],
            Part::RoomLink { room_id, label, picture }
                if room_id == "room-a" && label == "Mina's Pickle"
                    && picture == "https://img.example/mina.png"
        ));
    }

    #[test]
    fn link_normalization_accepts_only_exact_message_attachment_mappings() {
        let reply = item(serde_json::json!({
            "role": "assistant", "id": "mapped", "at": 1.0, "seq": 12,
            "attachments": ["attachments/2026-11-17/photo.png"],
            "inline_files": {
                "downloads/report final.pdf": "attachments/2026-11-17/보고서 최종.pdf",
                "downloads/../secret.pdf": "attachments/2026-11-17/secret.pdf"
            }
        }));
        let cases = [
            (
                "downloads/report%20final.pdf",
                Some("attachment:attachments/2026-11-17/보고서 최종.pdf"),
            ),
            ("downloads/report+final.pdf", None),
            ("downloads/report%ZZfinal.pdf", None),
            ("downloads/report%2520final.pdf", None),
            ("downloads/%2e%2e/secret.pdf", None),
            ("downloads/../secret.pdf", None),
            (
                "attachments/2026-11-17/photo.png",
                Some("attachment:attachments/2026-11-17/photo.png"),
            ),
            (
                "attachment:attachments/2026-11-17/photo.png",
                Some("attachment:attachments/2026-11-17/photo.png"),
            ),
            ("attachment:attachments/2026-11-17/unknown.pdf", None),
            ("/tmp/report.pdf", None),
            ("file:///tmp/report.pdf", None),
            ("javascript:alert(1)", None),
            (
                "https://example.test/file?q=1",
                Some("https://example.test/file?q=1"),
            ),
            (
                "pickle://chat?from=message&room_id=room-test",
                Some("pickle://chat?room_id=room-test"),
            ),
        ];
        for (target, expected) in cases {
            assert_eq!(
                normalize_link(target, &reply).as_deref(),
                expected,
                "{target}"
            );
        }
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

    #[test]
    fn captured_subagent_handoffs_are_clickable_task_links() {
        let task = item(serde_json::json!({
            "role":"subagent","id":"sample-task-message","at":1.0,"seq":8,
            "title":"Library opening hours","path":"handoffs/sample-library-task"
        }));
        let converted = message(&task, None);
        assert!(matches!(&converted.parts[..],[Part::PhoneTask {title,path}]
            if &**title=="Library opening hours" && path=="handoffs/sample-library-task"));
        let unsupported = item(serde_json::json!({
            "role":"subagent","id":"another-task","at":2.0,"seq":9,
            "title":"Other task","path":"documents/sample.txt"
        }));
        assert!(matches!(
            &message(&unsupported, None).parts[..],
            [Part::Task(_)]
        ));
    }
}
