//! Turns the text of a chat message into blocks the UI can draw.
//!
//! The character writes Markdown, wraps each chat bubble in `<bubble>…</bubble>` and puts
//! structured summaries in fenced ```` ```card ```` blocks, inside a bubble or between bubbles.
//! Parsing happens once per message, when it is loaded, never while drawing.

use std::ops::Range;

/// One visual unit of a message.
#[derive(Clone, Debug, PartialEq)]
pub enum Block {
    /// A link that opens another chat room, which the app puts in a card fence.
    RoomLink(String),
    Bubble(RichText),
    Card(Card),
}

/// Plain text plus the ranges that carry inline formatting.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RichText {
    pub text: String,
    pub spans: Vec<Span>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Span {
    pub range: Range<usize>,
    pub kind: SpanKind,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SpanKind {
    Bold,
    Italic,
    Code,
    Muted,
    /// A canonical target string. Message conversion resolves file aliases before the UI sees it.
    Link(String),
}

/// A link action the desktop app supports. Other schemes are inert.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    Web(String),
    Room(String),
    /// A task listed by the account's handoff screen; resolve to a call before opening.
    PhoneTask(String),
    /// The server attachment path, without the `attachment:` prefix.
    Attachment(String),
}

impl LinkTarget {
    /// Parses a canonical link target. This deliberately does not turn arbitrary local paths or
    /// unsupported internal schemes into web addresses.
    pub fn parse(target: &str) -> Option<Self> {
        if target.starts_with("https://") || target.starts_with("http://") {
            return Some(Self::Web(target.to_owned()));
        }
        if let Some(path) = target.strip_prefix("handoff:") {
            let name = path.strip_prefix("handoffs/")?;
            if name.is_empty()
                || name.contains('/')
                || name.chars().any(char::is_control)
                || matches!(name, "." | "..")
            {
                return None;
            }
            return Some(Self::PhoneTask(path.to_owned()));
        }
        if let Some(path) = target.strip_prefix("attachment:") {
            return Self::attachment(path);
        }
        let query = target.strip_prefix("pickle://chat?")?;
        let mut room_id = None;
        for parameter in query.split('&') {
            let (name, value) = parameter.split_once('=').unwrap_or((parameter, ""));
            if name == "room_id" {
                if room_id.is_some() || !valid_room_id(value) {
                    return None;
                }
                room_id = Some(value);
            }
        }
        room_id.map(|room_id| Self::Room(room_id.to_owned()))
    }

    /// Builds a canonical target only for a server attachment path.
    pub fn attachment(path: &str) -> Option<Self> {
        valid_attachment_path(path).then(|| Self::Attachment(path.to_owned()))
    }

    /// The canonical string stored in a rich-text link span and passed through the row callback.
    pub fn into_link(self) -> String {
        match self {
            Self::Web(url) => url,
            Self::Room(room_id) => format!("pickle://chat?room_id={room_id}"),
            Self::PhoneTask(path) => format!("handoff:{path}"),
            Self::Attachment(path) => format!("attachment:{path}"),
        }
    }
}

fn valid_room_id(room_id: &str) -> bool {
    !room_id.is_empty()
        && room_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

fn valid_attachment_path(path: &str) -> bool {
    path.strip_prefix("attachments/").is_some_and(|rest| {
        !rest.is_empty()
            && !rest.contains('\\')
            && !rest.chars().any(char::is_control)
            && rest
                .split('/')
                .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
    })
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Card {
    pub title: Option<RichText>,
    pub subtitle: Option<RichText>,
    pub rows: Vec<CardRow>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CardRow {
    /// `- **Label** value`, drawn as a small label above its value.
    Field {
        label: RichText,
        value: RichText,
    },
    Heading(RichText),
    Text(RichText),
    /// One table row; the first row of a table is its header.
    Table {
        cells: Vec<RichText>,
        header: bool,
    },
}

/// Splits a message into bubbles and cards, in reading order.
pub fn parse_message(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("<bubble>") {
        push_markdown(&rest[..start], &mut blocks);
        let after = &rest[start + "<bubble>".len()..];
        let end = after.find("</bubble>").unwrap_or(after.len());
        push_markdown(&after[..end], &mut blocks);
        rest = after.get(end + "</bubble>".len()..).unwrap_or("");
    }
    push_markdown(rest, &mut blocks);
    blocks
}

/// Text shown as typed, for your own messages: nothing is markup, but web addresses are links.
pub fn plain_text(text: &str) -> RichText {
    let mut spans = Vec::new();
    let mut from = 0;
    while let Some(found) = text[from..].find("http") {
        let start = from + found;
        let rest = &text[start..];
        if rest.starts_with("https://") || rest.starts_with("http://") {
            let url = bare_url(rest);
            spans.push(Span {
                range: start..start + url.len(),
                kind: SpanKind::Link(url.to_owned()),
            });
            from = start + url.len();
        } else {
            from = start + "http".len();
        }
    }
    RichText {
        text: text.to_owned(),
        spans,
    }
}

/// The web address at the start of `text`, without the punctuation that ends a sentence.
fn bare_url(text: &str) -> &str {
    let end = text
        .find(|c: char| c.is_whitespace() || matches!(c, ')' | '>' | '"'))
        .unwrap_or(text.len());
    text[..end].trim_end_matches(['.', ',', '!', '?'])
}

/// Markdown between or inside bubbles: text becomes a bubble, each ```card fence becomes a card.
fn push_markdown(markdown: &str, blocks: &mut Vec<Block>) {
    let mut text = String::new();
    let mut card: Option<String> = None;
    for line in markdown.lines() {
        let fence = line.trim_start().strip_prefix("```");
        match (&mut card, fence) {
            (None, Some(lang)) if lang.trim() == "card" => {
                push_bubble(&text, blocks);
                text.clear();
                card = Some(String::new());
            }
            (Some(body), Some(_)) => {
                // A card whose whole body is one supported chat address opens that room.
                // Parsing it here and inline uses the same scheme and query validation.
                let body = body.trim();
                if let Some(LinkTarget::Room(room_id)) = LinkTarget::parse(body) {
                    blocks.push(Block::RoomLink(room_id));
                } else {
                    blocks.push(Block::Card(parse_card(body)));
                }
                card = None;
            }
            (Some(body), None) => {
                body.push_str(line);
                body.push('\n');
            }
            // Other fences (plain code blocks) are kept as text without the fence lines.
            (None, Some(_)) => {}
            (None, None) => {
                text.push_str(line);
                text.push('\n');
            }
        }
    }
    if let Some(body) = card {
        blocks.push(Block::Card(parse_card(&body)));
    }
    push_bubble(&text, blocks);
}

fn push_bubble(markdown: &str, blocks: &mut Vec<Block>) {
    let mut out = RichText::default();
    for line in markdown.trim().lines() {
        if !out.text.is_empty() {
            out.text.push('\n');
        }
        let line = line.trim_end();
        let (indent, body) = split_indent(line);
        if let Some(heading) = strip_heading(body) {
            let start = out.text.len();
            push_inline(heading, &mut out);
            out.spans.push(Span {
                range: start..out.text.len(),
                kind: SpanKind::Bold,
            });
        } else if let Some(item) = strip_bullet(body) {
            out.text.push_str(&"  ".repeat(indent / 2));
            out.text.push_str("• ");
            push_inline(item, &mut out);
        } else if let Some(quote) = body.strip_prefix('>') {
            let start = out.text.len();
            push_inline(quote.trim_start(), &mut out);
            out.spans.push(Span {
                range: start..out.text.len(),
                kind: SpanKind::Muted,
            });
        } else if is_rule(body) {
            // A horizontal rule only separates paragraphs.
        } else {
            push_inline(line, &mut out);
        }
    }
    if !out.text.trim().is_empty() {
        blocks.push(Block::Bubble(out));
    }
}

fn parse_card(markdown: &str) -> Card {
    let mut card = Card::default();
    let mut table_started = false;
    for line in markdown.lines() {
        let line = line.trim_end();
        let (indent, body) = split_indent(line);
        if body.is_empty() {
            table_started = false;
            continue;
        }
        if body.starts_with('|') {
            let cells: Vec<&str> = body.trim_matches('|').split('|').map(str::trim).collect();
            let divider = cells
                .iter()
                .all(|cell| !cell.is_empty() && cell.chars().all(|c| matches!(c, '-' | ':')));
            if !divider {
                card.rows.push(CardRow::Table {
                    cells: cells.into_iter().map(inline).collect(),
                    header: !table_started,
                });
            }
            table_started = true;
            continue;
        }
        table_started = false;
        if let Some(heading) = strip_heading(body) {
            let level = body.len() - body.trim_start_matches('#').len();
            match level {
                1 | 2 if card.title.is_none() && card.rows.is_empty() => {
                    card.title = Some(inline(heading))
                }
                3 if card.subtitle.is_none() && card.rows.is_empty() => {
                    card.subtitle = Some(inline(heading))
                }
                _ => card.rows.push(CardRow::Heading(inline(heading))),
            }
        } else if let Some(item) = strip_bullet(body) {
            match (indent, split_field(item)) {
                (0, Some((label, value))) => card.rows.push(CardRow::Field {
                    label: inline(label),
                    value: inline(value),
                }),
                // A nested item continues the value of the field above it.
                (2.., _) if matches!(card.rows.last(), Some(CardRow::Field { .. })) => {
                    if let Some(CardRow::Field { value, .. }) = card.rows.last_mut() {
                        if !value.text.is_empty() {
                            value.text.push('\n');
                        }
                        value.text.push_str("• ");
                        push_inline(item, value);
                    }
                }
                _ => card.rows.push(CardRow::Text(inline(&format!(
                    "{}• {item}",
                    "  ".repeat(indent / 2)
                )))),
            }
        } else if let Some(quote) = body.strip_prefix('>') {
            let mut text = inline(quote.trim_start());
            text.spans.push(Span {
                range: 0..text.text.len(),
                kind: SpanKind::Muted,
            });
            card.rows.push(CardRow::Text(text));
        } else if !is_rule(body) {
            card.rows.push(CardRow::Text(inline(line)));
        }
    }
    card
}

fn split_indent(line: &str) -> (usize, &str) {
    let body = line.trim_start();
    (line.len() - body.len(), body)
}

fn strip_heading(line: &str) -> Option<&str> {
    let hashes = line.len() - line.trim_start_matches('#').len();
    (1..=6)
        .contains(&hashes)
        .then(|| line[hashes..].strip_prefix(' '))
        .flatten()
        .map(str::trim)
}

fn strip_bullet(line: &str) -> Option<&str> {
    line.strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .or_else(|| line.strip_prefix("• "))
}

fn is_rule(line: &str) -> bool {
    line.len() >= 3 && line.chars().all(|c| c == '-')
}

/// `**Label** value` or `**Label**: value`.
fn split_field(item: &str) -> Option<(&str, &str)> {
    let rest = item.strip_prefix("**")?;
    let end = rest.find("**")?;
    let value = rest[end + 2..].trim_start_matches([':', ' ']);
    (!value.is_empty()).then_some((&rest[..end], value))
}

fn inline(markdown: &str) -> RichText {
    let mut out = RichText::default();
    push_inline(markdown, &mut out);
    out
}

/// Appends `markdown` to `out`, removing inline markup and recording what it styled.
fn push_inline(markdown: &str, out: &mut RichText) {
    let bytes = markdown.as_bytes();
    let mut i = 0;
    let mut plain_from = 0;
    macro_rules! flush {
        () => {
            out.text.push_str(&markdown[plain_from..i]);
        };
    }
    while i < bytes.len() {
        let rest = &markdown[i..];
        let styled = if let Some(inner) = delimited(rest, "**") {
            Some((inner, 4, Some(SpanKind::Bold), true))
        } else if let Some(inner) = delimited(rest, "`") {
            Some((inner, 2, Some(SpanKind::Code), false))
        } else if rest.starts_with('*') && !rest.starts_with("* ") {
            delimited(rest, "*").map(|inner| (inner, 2, Some(SpanKind::Italic), true))
        } else {
            None
        };
        if let Some((inner, markup, kind, nested)) = styled {
            flush!();
            let start = out.text.len();
            if nested {
                push_inline(inner, out);
            } else {
                out.text.push_str(inner);
            }
            if let Some(kind) = kind {
                out.spans.push(Span {
                    range: start..out.text.len(),
                    kind,
                });
            }
            i += inner.len() + markup;
            plain_from = i;
        } else if let Some((label, url, len)) = markdown_link(rest) {
            flush!();
            let start = out.text.len();
            push_inline(label, out);
            out.spans.push(Span {
                range: start..out.text.len(),
                kind: SpanKind::Link(url.to_string()),
            });
            i += len;
            plain_from = i;
        } else if rest.starts_with("https://") || rest.starts_with("http://") {
            flush!();
            let url = bare_url(rest);
            let start = out.text.len();
            out.text.push_str(url);
            out.spans.push(Span {
                range: start..out.text.len(),
                kind: SpanKind::Link(url.to_string()),
            });
            i += url.len();
            plain_from = i;
        } else if let Some((ch, len)) = html_entity(rest) {
            flush!();
            out.text.push(ch);
            i += len;
            plain_from = i;
        } else {
            i += rest.chars().next().map_or(1, char::len_utf8);
        }
    }
    flush!();
}

/// The text between a leading `mark` and the next `mark` on the same line.
fn delimited<'a>(text: &'a str, mark: &str) -> Option<&'a str> {
    let inner = text.strip_prefix(mark)?;
    let end = inner.find(mark)?;
    let inner = &inner[..end];
    (!inner.is_empty() && !inner.contains('\n') && !inner.starts_with(' ')).then_some(inner)
}

/// `[label](url)`; a leading `!` (image) is treated the same way.
fn markdown_link(text: &str) -> Option<(&str, &str, usize)> {
    let bang = usize::from(text.starts_with("!["));
    let body = text[bang..].strip_prefix('[')?;
    let close = body.find("](")?;
    let label = &body[..close];
    let after = &body[close + 2..];
    let end = after.find(')')?;
    if label.contains('\n') || after[..end].contains(char::is_whitespace) {
        return None;
    }
    Some((label, &after[..end], bang + 1 + close + 2 + end + 1))
}

/// `&#xF8FF;`, `&#63743;`, `&amp;` and the few other named entities that occur.
fn html_entity(text: &str) -> Option<(char, usize)> {
    let body = text.strip_prefix('&')?;
    let end = body.find(';').filter(|end| *end <= 8)?;
    let name = &body[..end];
    let ch = match name {
        "amp" => '&',
        "lt" => '<',
        "gt" => '>',
        "quot" => '"',
        "nbsp" => ' ',
        _ => {
            let code = name.strip_prefix('#')?;
            let value = match code.strip_prefix(['x', 'X']) {
                Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                None => code.parse().ok()?,
            };
            char::from_u32(value)?
        }
    };
    Some((ch, end + 2))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bubble(blocks: &[Block], index: usize) -> &RichText {
        match &blocks[index] {
            Block::Bubble(text) => text,
            other => panic!("block {index} is {other:?}"),
        }
    }

    #[test]
    fn splits_bubbles_and_cards_in_order() {
        let blocks = parse_message(
            "<bubble>\n첫 번째\n</bubble>\n<bubble>둘째\n```card\n## 🏢 제목\n### 부제\n- **상호** 회사\n```\n</bubble>\n\n```card\n## 밖\n```\n<bubble>끝</bubble>",
        );
        assert_eq!(blocks.len(), 5);
        assert_eq!(bubble(&blocks, 0).text, "첫 번째");
        assert_eq!(bubble(&blocks, 1).text, "둘째");
        let Block::Card(card) = &blocks[2] else {
            panic!()
        };
        assert_eq!(card.title.as_ref().unwrap().text, "🏢 제목");
        assert_eq!(card.subtitle.as_ref().unwrap().text, "부제");
        assert_eq!(
            card.rows,
            vec![CardRow::Field {
                label: inline("상호"),
                value: inline("회사")
            }]
        );
        assert!(
            matches!(&blocks[3], Block::Card(card) if card.title.as_ref().unwrap().text == "밖")
        );
        assert_eq!(bubble(&blocks, 4).text, "끝");
    }

    #[test]
    fn text_without_bubble_tags_is_one_bubble() {
        let blocks = parse_message("그냥 **굵게** 쓴 글");
        assert_eq!(bubble(&blocks, 0).text, "그냥 굵게 쓴 글");
        assert_eq!(
            bubble(&blocks, 0).spans,
            vec![Span {
                range: 7..13,
                kind: SpanKind::Bold
            }]
        );
    }

    #[test]
    fn link_target_parses_only_supported_canonical_targets() {
        let cases = [
            (
                "https://example.test/a?next=pickle%3A%2F%2Fchat",
                Some(LinkTarget::Web(
                    "https://example.test/a?next=pickle%3A%2F%2Fchat".into(),
                )),
            ),
            (
                "pickle://chat?source=card&room_id=room-test&view=compact",
                Some(LinkTarget::Room("room-test".into())),
            ),
            (
                "attachment:attachments/2026-11-17/%ED%95%9C%EA%B8%80.pdf",
                Some(LinkTarget::Attachment(
                    "attachments/2026-11-17/%ED%95%9C%EA%B8%80.pdf".into(),
                )),
            ),
            ("pickle://chat?room_id=", None),
            ("pickle://chat?room_id=first&room_id=second", None),
            ("attachment:/tmp/report.pdf", None),
            ("attachment:attachments/../secret", None),
            ("file:///tmp/report.pdf", None),
            ("javascript:alert(1)", None),
            ("data:text/plain,no", None),
        ];
        for (raw, expected) in cases {
            assert_eq!(LinkTarget::parse(raw), expected, "{raw}");
        }
    }

    #[test]
    fn room_link_cards_use_the_same_query_parser_as_inline_links() {
        let blocks = parse_message(
            "```card\npickle://chat?source=reply&room_id=room-first\n```\n\n```card\npickle://chat?room_id=room-second&source=reply\n```",
        );
        assert_eq!(
            blocks,
            vec![
                Block::RoomLink("room-first".into()),
                Block::RoomLink("room-second".into()),
            ]
        );
    }

    #[test]
    fn inline_markup_becomes_spans() {
        let text = inline("a [링크](https://x.io/p) `code` https://y.io/z. &#xF8FF;&amp;");
        assert_eq!(text.text, "a 링크 code https://y.io/z. \u{F8FF}&");
        assert_eq!(
            text.spans,
            vec![
                Span {
                    range: 2..8,
                    kind: SpanKind::Link("https://x.io/p".into())
                },
                Span {
                    range: 9..13,
                    kind: SpanKind::Code
                },
                Span {
                    range: 14..28,
                    kind: SpanKind::Link("https://y.io/z".into())
                },
            ]
        );
    }

    #[test]
    fn plain_text_links_addresses_and_nothing_else() {
        let text = plain_text("**그대로** http 와 https://a.io/b?c=1, 끝");
        assert_eq!(text.text, "**그대로** http 와 https://a.io/b?c=1, 끝");
        assert_eq!(text.spans.len(), 1);
        assert_eq!(
            &text.text[text.spans[0].range.clone()],
            "https://a.io/b?c=1"
        );
    }

    #[test]
    fn bubble_keeps_paragraphs_and_lists() {
        let blocks =
            parse_message("<bubble>첫 줄\n\n- 하나\n  - 둘\n## 제목\n*기울임* 과 2 * 3</bubble>");
        let text = bubble(&blocks, 0);
        assert_eq!(text.text, "첫 줄\n\n• 하나\n  • 둘\n제목\n기울임 과 2 * 3");
        let styled = |kind: SpanKind| {
            let span = text.spans.iter().find(|span| span.kind == kind).unwrap();
            &text.text[span.range.clone()]
        };
        assert_eq!(styled(SpanKind::Bold), "제목");
        assert_eq!(styled(SpanKind::Italic), "기울임");
    }

    #[test]
    fn card_rows_cover_fields_nested_values_and_tables() {
        let card = parse_card(
            "## T\n- **이름** 값\n  - 더\n- 그냥\n1. 순서\n| a | b |\n| --- | --- |\n| 1 | 2 |\n",
        );
        assert_eq!(card.rows.len(), 5);
        assert!(matches!(&card.rows[0], CardRow::Field { value, .. } if value.text == "값\n• 더"));
        assert!(matches!(&card.rows[1], CardRow::Text(text) if text.text == "• 그냥"));
        assert!(matches!(&card.rows[2], CardRow::Text(text) if text.text == "1. 순서"));
        assert!(
            matches!(&card.rows[3], CardRow::Table { header: true, cells } if cells.len() == 2)
        );
        assert!(matches!(
            &card.rows[4],
            CardRow::Table { header: false, .. }
        ));
    }
}
