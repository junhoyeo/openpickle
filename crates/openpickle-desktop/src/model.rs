//! What the chat screen draws: messages, and the rows of the virtual list built from them.

use std::sync::Arc;

use chrono::{DateTime, Local, TimeZone};

use crate::content::{Card, CardRow, RichText};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    /// Your own messages: right-aligned, green.
    Mine,
    Theirs,
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

pub fn build_rows(
    messages: &[Message],
    activity: Option<Option<&str>>,
    now: DateTime<Local>,
) -> Vec<Row> {
    let mut rows: Vec<Row> = Vec::with_capacity(messages.len() * 2);
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
    rows
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
    fn groups_by_side_and_time_and_stamps_long_pauses() {
        let now = Local.timestamp_opt(10_000, 0).unwrap();
        let rows = build_rows(
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
    }
}
