//! `GET /api/events`: the server-sent event stream, mirroring `ts/src/events.ts`.

use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::types::{CharacterStatus, CreditUsage, JellyRewards, Room};

/// One parsed SSE frame. `data` is the raw payload, lines joined with `\n`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SseFrame {
    /// `message` when the frame names no event.
    pub event: String,
    pub data: Vec<u8>,
    /// The last `id:` seen on the stream, if any.
    pub id: Option<String>,
    /// The `retry:` value sent in this frame, in milliseconds.
    pub retry: Option<u64>,
}

/// Incremental SSE parser (WHATWG event-stream format) that works on bytes, so a chunk may end
/// anywhere, including inside a multi-byte UTF-8 character or between `\r` and `\n`.
#[derive(Debug, Default)]
pub struct SseParser {
    buffer: Vec<u8>,
    event: String,
    data: Vec<u8>,
    has_data: bool,
    id: Option<String>,
    retry: Option<u64>,
    /// The last chunk ended in `\r`; a `\n` that opens the next one is the rest of that `\r\n`.
    after_carriage_return: bool,
}

impl SseParser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feeds a chunk and appends every completed frame to `frames`.
    pub fn push(&mut self, mut chunk: &[u8], frames: &mut Vec<SseFrame>) {
        // A `\r` ends its line without waiting for what follows, because nothing may follow: a
        // stream that uses bare carriage returns would otherwise lose its last event.
        if self.after_carriage_return && chunk.first() == Some(&b'\n') {
            chunk = &chunk[1..];
            self.after_carriage_return = false;
        }
        if let Some(&last) = chunk.last() {
            self.after_carriage_return = last == b'\r';
        }
        let mut buffer = std::mem::take(&mut self.buffer);
        buffer.extend_from_slice(chunk);
        let mut start = 0;
        while let Some(offset) = buffer[start..]
            .iter()
            .position(|&b| b == b'\n' || b == b'\r')
        {
            let end = start + offset;
            let width = if buffer[end..].starts_with(b"\r\n") {
                2
            } else {
                1
            };
            if let Some(frame) = self.line(&buffer[start..end]) {
                frames.push(frame);
            }
            start = end + width;
        }
        buffer.drain(..start);
        self.buffer = buffer;
    }

    fn line(&mut self, line: &[u8]) -> Option<SseFrame> {
        if line.is_empty() {
            return self.dispatch();
        }
        if line[0] == b':' {
            return None;
        }
        let (field, mut value) = match line.iter().position(|&b| b == b':') {
            Some(colon) => (&line[..colon], &line[colon + 1..]),
            None => (line, &[][..]),
        };
        if value.first() == Some(&b' ') {
            value = &value[1..];
        }
        match field {
            b"event" => self.event = String::from_utf8_lossy(value).into_owned(),
            b"data" => {
                if self.has_data {
                    self.data.push(b'\n');
                }
                self.data.extend_from_slice(value);
                self.has_data = true;
            }
            b"id" => self.id = Some(String::from_utf8_lossy(value).into_owned()),
            b"retry" if !value.is_empty() && value.iter().all(u8::is_ascii_digit) => {
                // All digits, so this only fails on overflow.
                self.retry = std::str::from_utf8(value).ok().and_then(|v| v.parse().ok());
            }
            _ => {}
        }
        None
    }

    fn dispatch(&mut self) -> Option<SseFrame> {
        let empty = !self.has_data && self.retry.is_none();
        let event = std::mem::take(&mut self.event);
        let data = std::mem::take(&mut self.data);
        let retry = self.retry.take();
        self.has_data = false;
        if empty {
            return None;
        }
        Some(SseFrame {
            event: if event.is_empty() {
                "message".to_owned()
            } else {
                event
            },
            data,
            id: self.id.clone(),
            retry,
        })
    }
}

// ---- Typed events ----
//
// Every payload carries `v: 1`. The event name plus `kind` select the shape. The names and kinds
// below were present in traffic; a payload that does not fit them arrives as
// `PickleEvent::Unknown`.

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChatEvent {
    /// Your message as stored.
    User {
        role: String,
        /// The `client_msg_id` the message was sent with.
        id: String,
        text: String,
        /// Unix seconds
        at: f64,
        seq: i64,
        channel: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        prev_seq: Option<i64>,
    },
    /// A character reply. `text` wraps each chat bubble in `<bubble>…</bubble>`.
    Assistant {
        role: String,
        id: String,
        text: String,
        at: f64,
        seq: i64,
        channel: String,
        parts: Vec<i64>,
        prev_seq: i64,
    },
    /// Whether the character is working on a turn.
    Pending { on: bool },
    /// `chat_turn_started`, `chat_turn_finished`, …
    Responder { state: String },
    /// `thinking` (with `typing`), `tool` (with `label`), …
    Activity {
        state: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        typing: Option<bool>,
    },
    Reaction {
        id: String,
        seq: i64,
        /// The `client_msg_id` of the message reacted to.
        target_id: String,
        part: i64,
        actor: String,
        emoji: String,
        at: f64,
        prev_seq: i64,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JellyBatch {
    pub accepted: i64,
    pub eating: bool,
    pub rev: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UsageEvent {
    Energy {
        pct: i64,
        state: String,
    },
    Credits(CreditUsage),
    Jelly {
        count: i64,
        rewards: JellyRewards,
        batch: JellyBatch,
    },
    Plan {
        plan: String,
        plan_rank: i64,
        /// `null` in every observed event (free plan).
        expires_at: Option<String>,
        credits: CreditUsage,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BgmTrack {
    pub name: String,
    pub title: String,
    pub emoji: String,
    pub sha256: String,
    pub bytes: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum WidgetEvent {
    BgmCatalog {
        widget: String,
        tracks: Vec<BgmTrack>,
    },
    State {
        widget: String,
        revision: i64,
        data: serde_json::Map<String, Value>,
    },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RoomEvent {
    /// A room has a new `last_seq`. `room` lacks `log_epoch` and `projection_version`.
    Advanced {
        room_id: String,
        last_seq: i64,
        room: Room,
    },
}

/// A change to the room list; call `sync()` when `rev` moves past what you hold.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SyncEvent {
    pub rev: i64,
}

/// An item of [`PickleClient::events`](crate::PickleClient::events).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum PickleEvent {
    Chat(ChatEvent),
    Usage(Box<UsageEvent>),
    Status(Box<CharacterStatus>),
    Widget(WidgetEvent),
    Sync(SyncEvent),
    Room(Box<RoomEvent>),
    /// An event name, `kind` or shape this crate does not know, as received.
    Unknown {
        name: String,
        data: Value,
    },
    /// The server's suggested reconnection delay (`retry:`).
    Retry(Duration),
}

impl PickleEvent {
    /// Parses a frame's payload. Frames without data (the leading `retry:` frame) yield `None`.
    pub fn from_frame(frame: &SseFrame) -> Result<Option<Self>, serde_json::Error> {
        if frame.data.is_empty() {
            return Ok(None);
        }
        fn typed<T: serde::de::DeserializeOwned>(
            data: &[u8],
            wrap: fn(T) -> PickleEvent,
        ) -> Option<PickleEvent> {
            serde_json::from_slice(data).ok().map(wrap)
        }
        let data = &frame.data[..];
        let event = match frame.event.as_str() {
            "chat" => typed(data, Self::Chat),
            "usage" => typed(data, |v| Self::Usage(Box::new(v))),
            "status" => typed(data, |v| Self::Status(Box::new(v))),
            "widget" => typed(data, Self::Widget),
            "sync" => typed(data, Self::Sync),
            "room" => typed(data, |v| Self::Room(Box::new(v))),
            _ => None,
        };
        match event {
            Some(event) => Ok(Some(event)),
            None => Ok(Some(Self::Unknown {
                name: frame.event.clone(),
                data: serde_json::from_slice(data)?,
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Korean text, emoji, CRLF and lone CR line ends, a comment, `retry:`, multi-line data, an
    /// `id:`, an unknown event and an unknown `kind`.
    const STREAM: &str = concat!(
        ": keep-alive comment\r\n",
        "retry: 1250\r\n\r\n",
        "event: chat\r\n",
        "data: {\"v\":1,\"kind\":\"pending\",\"on\":true}\r\n\r\n",
        "event: chat\n",
        "id: 7\n",
        "data: {\"v\":1,\"kind\":\"user\",\"role\":\"user\",\"id\":\"m1\",\n",
        "data: \"text\":\"안녕하세요 👋🏽\",\"at\":1.5,\"seq\":3,\"channel\":\"chat\"}\n\n",
        "event: sparkle\r",
        "data: {\"v\":1,\"x\":\"✨\"}\r\r",
        "event: usage\r\n",
        "data:{\"v\":1,\"kind\":\"weather\",\"temp\":3}\r\n",
        "\r\n",
    );

    fn parse_split(at: usize) -> Vec<SseFrame> {
        let bytes = STREAM.as_bytes();
        let mut parser = SseParser::new();
        let mut frames = Vec::new();
        parser.push(&bytes[..at], &mut frames);
        parser.push(&bytes[at..], &mut frames);
        frames
    }

    #[test]
    fn a_bare_carriage_return_stream_keeps_its_last_event() {
        let stream = b"event: sync\rdata: {\"v\":1,\"rev\":1}\r\r";
        for at in 0..=stream.len() {
            let mut parser = SseParser::new();
            let mut frames = Vec::new();
            parser.push(&stream[..at], &mut frames);
            parser.push(&stream[at..], &mut frames);
            assert_eq!(frames.len(), 1, "split at byte {at}");
            assert_eq!(frames[0].event, "sync");
        }

        // `\r`, then `\n` in the next chunk, is one line ending; a second `\n` is a blank line.
        let mut parser = SseParser::new();
        let mut frames = Vec::new();
        parser.push(b"data: a\r", &mut frames);
        parser.push(b"\n", &mut frames);
        assert!(frames.is_empty());
        parser.push(b"\n", &mut frames);
        assert_eq!(frames.len(), 1);
    }

    #[test]
    fn sse_parser_is_independent_of_chunk_boundaries() {
        let whole = parse_split(STREAM.len());
        let names: Vec<&str> = whole.iter().map(|f| f.event.as_str()).collect();
        assert_eq!(names, ["message", "chat", "chat", "sparkle", "usage"]);
        assert_eq!(whole[0].retry, Some(1250));
        assert!(whole[0].data.is_empty());
        assert_eq!(whole[2].id.as_deref(), Some("7"));
        assert_eq!(
            whole[4].id.as_deref(),
            Some("7"),
            "the last id carries over"
        );

        for at in 0..=STREAM.len() {
            assert_eq!(parse_split(at), whole, "split at byte {at}");
        }

        let events: Vec<PickleEvent> = whole
            .iter()
            .filter_map(|f| PickleEvent::from_frame(f).unwrap())
            .collect();
        assert_eq!(events.len(), 4, "the retry-only frame yields no event");
        assert!(matches!(
            events[0],
            PickleEvent::Chat(ChatEvent::Pending { on: true })
        ));
        match &events[1] {
            PickleEvent::Chat(ChatEvent::User { text, seq, .. }) => {
                assert_eq!(text, "안녕하세요 👋🏽");
                assert_eq!(*seq, 3);
            }
            other => panic!("expected a user message, got {other:?}"),
        }
        match &events[2] {
            PickleEvent::Unknown { name, data } => {
                assert_eq!(name, "sparkle");
                assert_eq!(data["x"], "✨");
            }
            other => panic!("expected an unknown event, got {other:?}"),
        }
        match &events[3] {
            PickleEvent::Unknown { name, data } => {
                assert_eq!(name, "usage");
                assert_eq!(data["kind"], "weather");
            }
            other => panic!("expected an unknown usage kind, got {other:?}"),
        }
    }
}
