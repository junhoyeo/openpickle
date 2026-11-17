//! What the chat screen draws: messages, and the rows of the virtual list built from them.

use std::{ops::Range, sync::Arc};

use chrono::{DateTime, Local, TimeZone};

use crate::content::{Card, CardRow, RichText};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Your own messages: right-aligned, green.
    Mine,
    Theirs,
}

/// One result of a message search.
#[derive(Clone, Debug, PartialEq)]
pub struct SearchResult {
    /// The message's sequence number, when the result is a message of the open conversation.
    /// Other results cannot be opened.
    pub seq: Option<u64>,
    /// What the result is: `Chat`.
    pub label: String,
    /// The day of the message: `Nov 16`.
    pub date: String,
    /// A piece of the message on one line, and the parts of it that match the query.
    pub snippet: String,
    pub matches: Vec<Range<usize>>,
}

/// Someone on Pickle, as the friends and invite screens list them.
#[derive(Clone, Debug, PartialEq)]
pub struct Person {
    pub id: String,
    pub name: String,
    /// Their character's picture, named by its key in the picture cache. Empty when there
    /// is none.
    pub picture: String,
    /// A line under the name, where a list has one: `Joined 3 days ago`.
    pub note: String,
}

/// What the friends screen and the invite sheet show.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Social {
    pub friends: Vec<Person>,
    /// People who asked to be friends.
    pub requests: Vec<Person>,
    /// How many people are blocked.
    pub blocked: usize,
    pub invite: Invite,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Invite {
    /// The link to share. Empty when the server gave none.
    pub url: String,
    /// How many invites are left.
    pub left: i64,
    pub joined: Vec<Person>,
    pub inviters: Vec<Person>,
}

/// Something the character offers to do, on the Explore tab.
#[derive(Clone, Debug, PartialEq)]
pub struct Offer {
    pub id: String,
    /// The section the offer belongs to.
    pub section: String,
    /// What the character would do. Empty for some offers, which then show only `detail`.
    pub title: String,
    pub detail: String,
    /// The message that asks for the offer.
    pub ask: String,
    /// The offer's picture, named by its key in the picture cache.
    pub picture: String,
    /// The name of a system symbol, for an offer without a picture.
    pub symbol: String,
    /// `75,776 people asked for this`, or empty.
    pub asked_by: String,
    pub steps: Vec<String>,
    /// The areas the offer is listed under.
    pub areas: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct OfferSection {
    pub id: String,
    pub title: String,
    pub offers: Vec<Arc<Offer>>,
    /// How many offers show before the section is expanded.
    pub preview: usize,
    /// Whether the section has more to show than its preview.
    pub expandable: bool,
}

/// The Explore tab's content.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Explore {
    /// The areas that have a filter chip.
    pub areas: Vec<String>,
    /// The offers of the "popular" section, which an expanded section does not repeat.
    pub popular: Vec<String>,
    pub sections: Vec<OfferSection>,
    /// The offers no section previews, listed after the sections.
    pub more: Vec<Arc<Offer>>,
    /// Every offer in the server's order of rank: what an area's filter lists.
    pub ranked: Vec<Arc<Offer>>,
}

/// A chat room other than the one with your own character: a friend's Pickle to talk to.
#[derive(Clone, Debug, PartialEq)]
pub struct RoomInfo {
    pub room_id: String,
    /// The exact room title supplied by the server.
    pub title: String,
    /// The peer character's picture, by its key in the picture cache. Empty when there is none.
    pub picture: String,
    /// True when this room is visible but messages cannot be sent to it.
    pub observer: bool,
}

/// A picture or a file that a message of the conversation carries, as the Archive tab lists
/// it.
#[derive(Clone, Debug, PartialEq)]
pub struct Attachment {
    /// The server's path of the attachment: `attachments/<day>/<file name>`.
    pub path: String,
    /// The sequence number of the message that carries it.
    pub seq: u64,
    /// Unix seconds of that message.
    pub at: f64,
    pub name: String,
    /// `PDF · 12 pages · 1.2 MB`, for a file; empty for a picture.
    pub detail: String,
    pub kind: AttachmentKind,
}

/// A message's pictures and files, drawn under its text.
#[derive(Clone, Debug, PartialEq)]
pub struct Files {
    pub items: Vec<Attachment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AttachmentKind {
    Picture,
    File,
}

/// Messages typed here that the server has not confirmed yet are numbered above this, which
/// is above every real sequence number. They stay last, in typing order.
pub const UNCONFIRMED_SEQ: u64 = u64::MAX - 1_000_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Message {
    /// The `client_msg_id` for messages you sent, the server's id otherwise.
    pub id: String,
    pub seq: u64,
    /// Unix seconds.
    pub at: f64,
    pub side: Side,
    pub parts: Vec<Part>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Part {
    Bubble(Arc<RichText>),
    Card(Arc<Card>),
    Request(Arc<Request>),
    /// A task the character started (a phone call, a purchase).
    Task(Arc<str>),
    /// A subagent handoff whose call transcript can be resolved from the handoff list.
    PhoneTask {
        title: Arc<str>,
        path: String,
    },
    /// Pictures and files the message carries.
    Files(Arc<Files>),
    /// A link that opens another chat room.
    RoomLink {
        room_id: String,
        label: String,
        /// The friend's character's picture, by its key in the picture cache.
        picture: String,
    },
}

/// A question from the character that is answered through a card.
#[derive(Clone, Debug, PartialEq)]
pub struct Request {
    pub caption: String,
    pub question: Option<String>,
    pub options: Vec<RequestOption>,
    pub status: RequestStatus,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RequestOption {
    pub label: String,
    pub chosen: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestStatus {
    Pending,
    Answered,
    Expired,
}

/// One row of the message list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    pub side: Side,
    /// Whether the row above / below belongs to the same group, which squares the corners
    /// between them and shrinks the gap.
    pub joined_above: bool,
    pub joined_below: bool,
}

impl Row {
    /// The texts this row shows, in reading order. Selecting and copying address a text by its
    /// place in this list, so drawing must show them in the same order.
    pub fn texts(&self) -> Vec<&str> {
        match &self.kind {
            RowKind::Time(text) => vec![text.as_str()],
            RowKind::Activity(_) => Vec::new(),
            RowKind::Part(Part::Bubble(text)) => vec![&text.text],
            RowKind::Part(Part::Task(title)) => vec![&**title],
            RowKind::Part(Part::PhoneTask { title, .. }) => vec![&**title],
            RowKind::Part(Part::Card(card)) => {
                let mut texts: Vec<&str> = card
                    .title
                    .iter()
                    .chain(&card.subtitle)
                    .map(|text| text.text.as_str())
                    .collect();
                for row in &card.rows {
                    match row {
                        CardRow::Field { label, value } => {
                            texts.extend([label.text.as_str(), value.text.as_str()])
                        }
                        CardRow::Heading(text) | CardRow::Text(text) => texts.push(&text.text),
                        CardRow::Table { cells, .. } => {
                            texts.extend(cells.iter().map(|cell| cell.text.as_str()))
                        }
                    }
                }
                texts
            }
            RowKind::Part(Part::Request(request)) => std::iter::once(request.caption.as_str())
                .chain(request.question.as_deref())
                .chain(request.options.iter().map(|option| option.label.as_str()))
                .collect(),
            RowKind::Part(Part::Files(files)) => files
                .items
                .iter()
                .flat_map(|item| [item.name.as_str(), item.detail.as_str()])
                .filter(|text| !text.is_empty())
                .collect(),
            RowKind::Part(Part::RoomLink { label, .. }) => vec![label.as_str()],
        }
    }

    /// Whether two rows draw the same thing. Parts are compared by identity, which is enough
    /// because a message's parts are parsed once and shared from then on.
    pub fn same_as(&self, other: &Row) -> bool {
        let same_kind = match (&self.kind, &other.kind) {
            (RowKind::Part(a), RowKind::Part(b)) => match (a, b) {
                (Part::Bubble(a), Part::Bubble(b)) => Arc::ptr_eq(a, b),
                (Part::Card(a), Part::Card(b)) => Arc::ptr_eq(a, b),
                (Part::Request(a), Part::Request(b)) => Arc::ptr_eq(a, b),
                (Part::Task(a), Part::Task(b)) => Arc::ptr_eq(a, b),
                (Part::Files(a), Part::Files(b)) => Arc::ptr_eq(a, b),
                (Part::RoomLink { .. }, Part::RoomLink { .. }) => a == b,
                _ => false,
            },
            (a, b) => a == b,
        };
        same_kind
            && self.side == other.side
            && self.joined_above == other.joined_above
            && self.joined_below == other.joined_below
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Time(String),
    Part(Part),
    /// The character is working on a reply; the label names what it is doing, when known.
    Activity(Option<String>),
}

/// Messages closer together than this, from the same side, form one group.
const GROUP_WINDOW_S: f64 = 5. * 60.;
/// A pause this long gets a timestamp before the next message.
const TIMESTAMP_GAP_S: f64 = 60. * 60.;

/// Lays the messages out as list rows. Also returns, for each message, the index of its first
/// row, which is where the list scrolls to when a search result is opened.
pub fn build_rows(
    messages: &[Message],
    activity: Option<Option<&str>>,
    now: DateTime<Local>,
) -> (Vec<Row>, Vec<usize>) {
    let mut rows: Vec<Row> = Vec::with_capacity(messages.len() * 2);
    let mut starts: Vec<usize> = Vec::with_capacity(messages.len());
    let mut previous: Option<&Message> = None;
    for message in messages {
        let gap = previous.map_or(f64::INFINITY, |previous| message.at - previous.at);
        let stamped = gap >= TIMESTAMP_GAP_S;
        if stamped {
            rows.push(Row {
                kind: RowKind::Time(format_time(message.at, now)),
                side: Side::Theirs,
                joined_above: false,
                joined_below: false,
            });
        }
        starts.push(rows.len());
        let continues = !stamped
            && gap < GROUP_WINDOW_S
            && previous.is_some_and(|previous| previous.side == message.side);
        for (index, part) in message.parts.iter().enumerate() {
            let joined_above = index > 0 || continues;
            if joined_above && let Some(above) = rows.last_mut() {
                above.joined_below = true;
            }
            rows.push(Row {
                kind: RowKind::Part(part.clone()),
                side: message.side,
                joined_above,
                joined_below: false,
            });
        }
        previous = Some(message);
    }
    if let Some(label) = activity {
        rows.push(Row {
            kind: RowKind::Activity(label.map(str::to_owned)),
            side: Side::Theirs,
            joined_above: false,
            joined_below: false,
        });
    }
    (rows, starts)
}

/// `Today 10:08 PM`, `Yesterday 9:12 AM`, `Nov 12, 9:12 AM`.
fn format_time(at: f64, now: DateTime<Local>) -> String {
    let Some(time) = Local.timestamp_opt(at as i64, 0).single() else {
        return String::new();
    };
    let days = (now.date_naive() - time.date_naive()).num_days();
    let clock = time.format("%-I:%M %p");
    match days {
        0 => format!("Today {clock}"),
        1 => format!("Yesterday {clock}"),
        _ if time.format("%Y").to_string() == now.format("%Y").to_string() => {
            format!("{} {clock}", time.format("%b %-d,"))
        }
        _ => format!("{} {clock}", time.format("%b %-d, %Y,")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(at: f64, side: Side, parts: usize) -> Message {
        let part = Part::Bubble(Arc::new(RichText {
            text: "hi".into(),
            spans: vec![],
        }));
        Message {
            id: at.to_string(),
            seq: at as u64,
            at,
            side,
            parts: vec![part; parts],
        }
    }

    #[test]
    fn file_row_texts_match_only_the_nonempty_labels_the_ui_draws() {
        let row = Row {
            kind: RowKind::Part(Part::Files(Arc::new(Files {
                items: vec![
                    Attachment {
                        path: "attachments/day/photo.png".into(),
                        seq: 1,
                        at: 1.,
                        name: "photo.png".into(),
                        detail: String::new(),
                        kind: AttachmentKind::Picture,
                    },
                    Attachment {
                        path: "attachments/day/report.pdf".into(),
                        seq: 1,
                        at: 1.,
                        name: String::new(),
                        detail: "PDF · 2 pages".into(),
                        kind: AttachmentKind::File,
                    },
                ],
            }))),
            side: Side::Theirs,
            joined_above: false,
            joined_below: false,
        };
        assert_eq!(row.texts(), ["photo.png", "PDF · 2 pages"]);
    }

    #[test]
    fn groups_by_side_and_time_and_stamps_long_pauses() {
        let now = Local.timestamp_opt(10_000, 0).unwrap();
        let (rows, starts) = build_rows(
            &[
                message(0., Side::Theirs, 2),
                message(10., Side::Theirs, 1),
                message(20., Side::Mine, 1),
                message(9_000., Side::Mine, 1),
            ],
            Some(Some("Working")),
            now,
        );
        let shape: Vec<(bool, bool, bool)> = rows
            .iter()
            .map(|row| {
                (
                    matches!(row.kind, RowKind::Part(_)),
                    row.joined_above,
                    row.joined_below,
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (false, false, false), // timestamp before the first message
                (true, false, true),
                (true, true, true),
                (true, true, false),   // same side, 10 s later
                (true, false, false),  // other side
                (false, false, false), // timestamp after a 2.5 h pause
                (true, false, false),
                (false, false, false), // activity
            ]
        );
        assert!(matches!(&rows[7].kind, RowKind::Activity(Some(label)) if label == "Working"));
        // Each message starts at its first part, after its timestamp.
        assert_eq!(starts, [1, 3, 4, 6]);
    }
}
