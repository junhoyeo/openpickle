//! Draws one row of the message list: a bubble, a card, a timestamp.

use std::{cell::Cell, collections::HashMap, ops::Range, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, Div, FontWeight, HighlightStyle, ImageSource, MouseButton,
    ObjectFit, Pixels, SharedString, StyledText, UnderlineStyle, Window, div, img,
    linear_color_stop, linear_gradient, prelude::*, px, relative, svg,
};
use openpickle_api::{HandoffItem, PhoneCallTranscript};

use crate::{
    content::{Card, CardRow, RichText, Span, SpanKind},
    model::{
        AttachmentKind, Part, Quote, ReactionBadge, Request, RequestStatus, Row, RowKind, Side,
    },
    phone,
    protocol::HandoffCard,
    theme,
    ui::{
        Pictures,
        phone::{TRACKED, transcript_line},
        select::{LinkHandler, SelectableText, SharedSelection},
        tasks::elapsed_label,
        wrap::{fit, keep_words, line_width},
    },
};

/// The task or call each handoff card shows, by its handoff path.
pub type HandoffCards = HashMap<String, HandoffCard>;

/// Widths from the design column (408 pt of content): bubbles hug their text up to these.
const THEIRS_MAX: f32 = 394. / 408.;
const MINE_MAX: f32 = 304. / 408.;
const REQUEST_WIDTH: f32 = 342.;

/// What drawing a row needs.
pub struct RowContext<'a> {
    pub window: &'a Window,
    pub row: &'a Row,
    /// The row's place in the list.
    pub index: usize,
    /// Width of the message column without its gutters.
    pub content_width: Pixels,
    pub selection: &'a SharedSelection,
    pub on_link: LinkHandler,
    /// The pictures on disk, for the files a message carries.
    pub pictures: &'a Pictures,
    /// Whether the row belongs to the message a search result has just opened.
    pub marked: bool,
    /// The summaries that have been read for the handoff cards.
    pub handoff_cards: Option<&'a HandoffCards>,
    /// The character's picture, which stands next to its lines of a call.
    pub avatar: Option<ImageSource>,
    /// How many of the row's texts have been drawn so far.
    drawn: Cell<usize>,
}

impl<'a> RowContext<'a> {
    pub fn new(
        window: &'a Window,
        row: &'a Row,
        index: usize,
        selection: &'a SharedSelection,
        on_link: LinkHandler,
        pictures: &'a Pictures,
    ) -> Self {
        let content_width =
            window.viewport_size().width.min(theme::COLUMN_MAX) - theme::GUTTER * 2.;
        Self {
            window,
            row,
            index,
            content_width,
            selection,
            on_link,
            pictures,
            marked: false,
            handoff_cards: None,
            avatar: None,
            drawn: Cell::new(0),
        }
    }
}

pub fn render_row(cx: &RowContext) -> AnyElement {
    let row = cx.row;
    let top_gap = if cx.index == 0 {
        px(0.)
    } else if row.joined_above {
        theme::JOINED_GAP
    } else {
        theme::GROUP_GAP
    };
    let line = div()
        .w_full()
        .flex()
        .px(theme::GUTTER)
        .pt(top_gap)
        .when(cx.marked, |line| line.bg(theme::alpha(theme::LINK, 0.13)))
        .on_mouse_down(MouseButton::Right, {
            let (selection, index) = (cx.selection.clone(), cx.index);
            move |_, _, _| selection.borrow_mut().set_context_row(index)
        });
    match &row.kind {
        // The design puts a timestamp 8 pt above the next row, not the usual 12.
        RowKind::Time(time) => line
            .justify_center()
            .mb(px(-4.))
            .text_size(px(13.))
            .line_height(px(18.))
            .text_color(theme::color(theme::TEXT_FAINT))
            .child(plain(time, cx))
            .into_any_element(),
        RowKind::Activity(label) => line.child(activity(label.as_deref())).into_any_element(),
        RowKind::Part(part) => {
            let line = match row.side {
                Side::Mine => line.justify_end(),
                Side::Theirs => line.justify_start(),
            };
            // A handoff stays the quiet task note only until the handoff list identifies it.
            let card = match part {
                Part::PhoneTask { path, .. } => cx.handoff_cards.and_then(|cards| cards.get(path)),
                _ => None,
            };
            let is_note = matches!(part, Part::Task(_))
                || (matches!(part, Part::PhoneTask { .. }) && card.is_none());
            let surface = shaped(row, matches!(part, Part::Bubble(_)) || is_note);
            let content: AnyElement = match part {
                Part::PhoneTask { title, path } => match card {
                    Some(HandoffCard::Phone(call)) => phone_card(surface, call, path, cx),
                    Some(HandoffCard::Task(task)) => task_card(surface, task, path, cx),
                    None => task_note(surface, title, cx),
                },
                Part::Bubble(rich) => bubble(surface, rich, cx).into_any_element(),
                Part::Task(title) => task_note(surface, title, cx),
                Part::Files(files) => {
                    let room = cx.content_width
                        * if row.side == Side::Mine {
                            MINE_MAX
                        } else {
                            THEIRS_MAX
                        }
                        - px(28.);
                    surface
                        .px(px(14.))
                        .py(px(11.))
                        .max_w(relative(THEIRS_MAX))
                        .flex()
                        .flex_col()
                        .gap(px(10.))
                        .children(files.items.iter().map(|item| file_card(item, room, cx)))
                        .into_any_element()
                }
                Part::RoomLink {
                    room_id,
                    label,
                    picture,
                } => {
                    let target = format!("pickle://chat?room_id={room_id}");
                    let on_link = cx.on_link.clone();
                    let selection = cx.selection.clone();
                    div()
                        .id(("room-link", cx.index))
                        .max_w(relative(THEIRS_MAX))
                        .px(px(14.))
                        .py(px(12.))
                        .rounded(theme::BUBBLE_RADIUS)
                        .bg(theme::color(theme::SURFACE))
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .cursor_pointer()
                        .hover(|card| card.bg(theme::color(theme::SURFACE_INSET)))
                        .on_click(move |_, window, cx| {
                            if selection.borrow().is_empty() {
                                on_link(&target, window, cx);
                            }
                        })
                        .child(super::face(cx.pictures.get(picture), 38.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .text_size(theme::LABEL)
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(plain(label, cx)),
                                )
                                .child(
                                    div()
                                        .text_size(px(13.))
                                        .text_color(theme::color(theme::TEXT_FAINT))
                                        .child("Open chat room"),
                                ),
                        )
                        .into_any_element()
                }
                // Cards sit 2 pt further from their neighbors than bubbles do.
                Part::Card(card) => div()
                    .py(px(2.))
                    .w(relative(THEIRS_MAX))
                    .child(info_card(surface, card, cx))
                    .into_any_element(),
                Part::Request(request) => div()
                    .py(px(2.))
                    .child(request_card(surface, request, cx))
                    .into_any_element(),
            };
            let decorations = &row.decorations;
            if decorations.quote.is_none() && decorations.reactions.is_none() {
                return line.child(content).into_any_element();
            }
            let mine = row.side == Side::Mine;
            let max = if mine { MINE_MAX } else { THEIRS_MAX };
            line.flex_col()
                .gap(theme::JOINED_GAP)
                .when(mine, |line| line.items_end())
                .when(!mine, |line| line.items_start())
                .children(
                    decorations
                        .quote
                        .as_deref()
                        .map(|quote| quote_preview(quote, max)),
                )
                .child(content)
                .children(
                    decorations
                        .reactions
                        .as_deref()
                        .map(|reactions| reaction_summary(reactions, max, mine)),
                )
                .into_any_element()
        }
    }
}

// These labels are intentionally not SelectableText: they do not change the row's text ordinals.
fn quote_preview(quote: &Quote, max: f32) -> Div {
    let label = quote.author.map_or_else(
        || "Quoted message".into(),
        |author| format!("Reply to {author}"),
    );
    div()
        .max_w(relative(max))
        .px(px(10.))
        .py(px(4.))
        .border_l_2()
        .border_color(theme::color(theme::TEXT_FAINT))
        .rounded_r(theme::JOINED_RADIUS)
        .bg(theme::color(theme::SURFACE_INSET))
        .text_size(theme::SMALL)
        .line_height(theme::SMALL_LINE)
        .child(
            div()
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::color(theme::TEXT))
                .child(label),
        )
        .when(!quote.snippet.is_empty(), |preview| {
            preview.child(
                div()
                    .truncate()
                    .text_color(theme::color(theme::TEXT_MUTED))
                    .child(quote.snippet.clone()),
            )
        })
}

fn reaction_summary(reactions: &[ReactionBadge], max: f32, mine: bool) -> Div {
    div()
        .max_w(relative(max))
        .flex()
        .flex_wrap()
        .items_center()
        .when(mine, |line| line.justify_end())
        .gap(px(4.))
        .text_size(theme::SMALL)
        .line_height(theme::SMALL_LINE)
        .text_color(theme::color(theme::TEXT_MUTED))
        .child("Reactions")
        .children(reactions.iter().map(|reaction| {
            let label = if reaction.count == 1 {
                reaction.emoji.clone()
            } else {
                format!("{} {}", reaction.emoji, reaction.count)
            };
            div()
                .max_w_full()
                .truncate()
                .px(px(6.))
                .rounded(theme::JOINED_RADIUS)
                .bg(theme::color(theme::SURFACE))
                .text_color(theme::color(theme::TEXT))
                .child(label)
        }))
}

/// A filled surface whose corners are squared where it meets a neighbor of the same group.
fn shaped(row: &Row, is_bubble: bool) -> Div {
    let full = if is_bubble {
        theme::BUBBLE_RADIUS
    } else {
        theme::CARD_RADIUS
    };
    let above = if row.joined_above {
        theme::JOINED_RADIUS
    } else {
        full
    };
    let below = if row.joined_below {
        theme::JOINED_RADIUS
    } else {
        full
    };
    let surface = div().rounded(full);
    match row.side {
        Side::Theirs => surface
            .bg(theme::color(theme::SURFACE))
            .rounded_tl(above)
            .rounded_bl(below),
        Side::Mine => surface
            .bg(theme::color(theme::MINE))
            .text_color(theme::color(theme::ON_MINE))
            .rounded_tr(above)
            .rounded_br(below),
    }
}

fn bubble(surface: Div, rich: &RichText, cx: &RowContext) -> Div {
    let max = if cx.row.side == Side::Mine {
        MINE_MAX
    } else {
        THEIRS_MAX
    };
    let padding = px(14.);
    let wrap = cx.content_width * max - padding * 2.;
    surface
        .px(padding)
        .py(px(11.))
        .max_w(relative(max))
        .child(rich_text(rich, Some(wrap), cx))
}

/// A message attachment. The original remains private on Pickle; this card opens a clearly
/// labelled thumbnail preview using the endpoint observed in the captures.
fn file_card(item: &crate::model::Attachment, room: Pixels, cx: &RowContext) -> AnyElement {
    let key = super::archive::thumbnail_key(&item.path);
    let size = if item.kind == AttachmentKind::Picture {
        px(110.)
    } else {
        px(46.)
    };
    let preview = match cx.pictures.get(&key) {
        Some(source) => img(source)
            .size(size)
            .rounded(px(8.))
            .object_fit(ObjectFit::Contain)
            .into_any_element(),
        None => div()
            .size(size)
            .rounded(px(8.))
            .bg(theme::alpha(0xffffff, 0.06))
            .flex()
            .items_center()
            .justify_center()
            .child(
                svg()
                    .path("icons/file-text.svg")
                    .size(px(22.))
                    .text_color(theme::color(theme::TEXT_MUTED)),
            )
            .into_any_element(),
    };
    let on_link = cx.on_link.clone();
    let target = format!("attachment:{}", item.path);
    let selection = cx.selection.clone();
    let mut labels = div().flex_1().min_w_0().flex().flex_col().gap(px(3.));
    if !item.name.is_empty() {
        labels = labels.child(div().text_size(theme::LABEL).child(plain(&item.name, cx)));
    }
    if !item.detail.is_empty() {
        labels = labels.child(
            div()
                .text_size(px(13.))
                .text_color(theme::color(theme::TEXT_FAINT))
                .child(plain(&item.detail, cx)),
        );
    }
    labels = labels.child(
        div()
            .text_size(px(12.))
            .text_color(theme::color(theme::LINK))
            .child("Preview"),
    );
    div()
        .id(SharedString::from(format!(
            "attachment-{}-{}",
            cx.index,
            cx.drawn.get()
        )))
        .w(room)
        .max_w_full()
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            if selection.borrow().is_empty() {
                on_link(&target, window, cx);
            }
        })
        .child(div().flex_none().child(preview))
        .child(labels)
        .into_any_element()
}

/// The phone app's task card is 280 pt wide. Its texts fill 220 pt, left of the chevron.
const TASK_CARD_WIDTH: f32 = 280.;
const TASK_CARD_TEXT: f32 = 220.;

/// A task Pickle handed off, as the phone app draws it in the chat: its title, what it is
/// doing now and for how long, and the note that the chat stays open meanwhile. A task that
/// has ended keeps its title and the line the handoff list has about it.
fn task_card(surface: Div, task: &HandoffItem, path: &str, cx: &RowContext) -> AnyElement {
    let target = format!("handoff:{path}");
    let on_link = cx.on_link.clone();
    let selection = cx.selection.clone();
    let running = task.state == "running";
    let room = px(TASK_CARD_TEXT);
    let label = px(14. * TRACKED);
    let fit_label =
        |text: &str, room: Pixels| fit(text, label, FontWeight::NORMAL, room, cx.window);
    let status = if running {
        let elapsed = elapsed_label(task.requested_at_epoch_ms, phone::now_ms());
        let line = div()
            .flex()
            .gap(px(3.))
            .text_color(theme::color(theme::LINK));
        Some(match task.latest_log_text.as_deref() {
            Some(log) => {
                let elapsed = format!("· {elapsed}");
                let taken = line_width(&elapsed, label, FontWeight::NORMAL, cx.window);
                line.child(fit_label(log, room - px(3.) - taken))
                    .child(elapsed)
            }
            // Nothing is logged yet: the time alone says that the task runs.
            None => line.child(elapsed),
        })
    } else {
        task.subtitle.as_deref().map(|subtitle| {
            div()
                .text_color(theme::color(theme::TEXT_FAINT))
                .child(fit_label(subtitle, room))
        })
    };
    let texts = div()
        .w(room)
        .flex_none()
        .flex()
        .flex_col()
        .whitespace_nowrap()
        .child(
            div()
                .text_size(px(16.))
                .line_height(px(22.))
                .font_weight(FontWeight::MEDIUM)
                .child(fit(
                    &task.title,
                    px(16.),
                    FontWeight::MEDIUM,
                    room,
                    cx.window,
                )),
        )
        .children(status.map(|status| status.pt(px(4.)).text_size(label).line_height(px(18.))));
    let chevron = div()
        .w(px(20.))
        .flex()
        .justify_center()
        .opacity(0.8)
        .child(super::icon("chevron-right", 14., theme::TEXT_FAINT));
    let footer = running.then(|| {
        div()
            .mt(px(8.))
            .pt(px(6.75))
            .border_t_1()
            .border_color(theme::color(theme::HAIRLINE))
            .flex()
            .items_center()
            .gap(px(2.))
            .text_size(px(12. * TRACKED))
            .line_height(px(16.))
            .text_color(theme::alpha(theme::TEXT_MUTED, 0.9))
            .child(
                div()
                    .w(px(16.))
                    .pb(px(1.5))
                    .flex()
                    .justify_center()
                    .opacity(0.7)
                    .child(super::icon("message-circle-filled", 12., theme::TEXT_FAINT)),
            )
            .child("Feel free to chat about other things")
    });
    surface
        .id(("handoff-task", cx.index))
        .w(px(TASK_CARD_WIDTH))
        .max_w_full()
        .px(px(16.))
        .pt(px(14.5))
        .pb(px(if running { 10.75 } else { 13.5 }))
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            if selection.borrow().is_empty() {
                on_link(&target, window, cx);
            }
        })
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(texts)
                .child(chevron),
        )
        .children(footer)
        .into_any_element()
}

/// The transcript in a phone card is the transcript screen at this scale, with text that is
/// a little larger for it: a line holds fewer words there.
const CARD_TRANSCRIPT_SCALE: f32 = 0.73;
const CARD_TEXT_SCALE: f32 = 0.742;
/// A card shows the end of the call: no more lines than can fill its window.
const CARD_TRANSCRIPT_LINES: usize = 6;

/// A note, not speech: a task the character started, a failed send.
fn task_note(surface: Div, title: &str, cx: &RowContext) -> AnyElement {
    surface
        .px(px(14.))
        .py(px(11.))
        .max_w(relative(THEIRS_MAX))
        .when(cx.row.side == Side::Theirs, |note| {
            note.text_color(theme::color(theme::TEXT_MUTED))
        })
        .when(cx.row.side == Side::Mine, |note| {
            note.text_color(theme::alpha(theme::ON_MINE, 0.6))
        })
        .child(plain(title, cx))
        .into_any_element()
}

/// A call Pickle placed: its name, the other side and how long it lasted, over the end of its
/// transcript.
fn phone_card(surface: Div, call: &PhoneCallTranscript, path: &str, cx: &RowContext) -> AnyElement {
    let target = format!("handoff:{path}");
    let on_link = cx.on_link.clone();
    let selection = cx.selection.clone();
    let name = call.title.clone();
    let counterpart = Some(call.counterpart_label.clone()).filter(|label| !label.is_empty());
    let duration = phone::duration_label(call, phone::now_ms());

    let header = div()
        .w_full()
        .pt(px(12.))
        .pb(px(10.))
        .pl(px(12.))
        .pr(px(20.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .size(px(38.))
                .flex_none()
                .rounded(px(12.))
                .bg(theme::color(theme::TILE))
                .flex()
                .items_center()
                .justify_center()
                .child(super::icon("phone-filled", 17., theme::TEXT)),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(15.))
                        .line_height(px(20.))
                        .font_weight(FontWeight::MEDIUM)
                        .truncate()
                        .child(name),
                )
                .children(counterpart.map(|label| {
                    div()
                        .text_size(px(12. * TRACKED))
                        .line_height(px(16.))
                        .text_color(theme::alpha(0xffffff, 0.48))
                        .truncate()
                        .child(label)
                })),
        )
        .children(duration.map(|duration| {
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(15.5))
                .line_height(px(20.))
                .text_color(theme::color(theme::TEXT_FAINT))
                .child(super::icon("audio-lines", 16., theme::TEXT_FAINT))
                .child(duration)
        }));

    let lines = &call.lines[call.lines.len().saturating_sub(CARD_TRANSCRIPT_LINES)..];
    // The window on the transcript: its last lines stand at the bottom, the ones before them
    // run out under the top, where the page fades in over them.
    let transcript = (!lines.is_empty()).then(|| {
        div()
            .relative()
            .mx(px(4.))
            .mb(px(4.))
            .h(px(165.))
            .rounded(theme::CARD_RADIUS - px(4.))
            .bg(theme::color(theme::PAGE))
            .overflow_hidden()
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .px(px(14.))
                    .pb(px(13.))
                    .flex()
                    .flex_col()
                    .children(lines.iter().map(|line| {
                        transcript_line(
                            line,
                            cx.avatar.clone(),
                            CARD_TRANSCRIPT_SCALE,
                            CARD_TEXT_SCALE,
                            cx.window,
                        )
                    })),
            )
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(90.))
                    .rounded_t(theme::CARD_RADIUS - px(4.))
                    .bg(linear_gradient(
                        180.,
                        linear_color_stop(theme::alpha(theme::PAGE, 0.82), 0.),
                        linear_color_stop(theme::alpha(theme::PAGE, 0.), 1.),
                    )),
            )
    });

    surface
        .id(("phone-task", cx.index))
        .w(px(380.))
        .max_w_full()
        // The phone app's card has this one corner closed up, whatever stands above it.
        .rounded_tl(theme::JOINED_RADIUS)
        .flex()
        .flex_col()
        .cursor_pointer()
        .on_click(move |_, window, cx| {
            if selection.borrow().is_empty() {
                on_link(&target, window, cx);
            }
        })
        .child(header)
        .children(transcript)
        .into_any_element()
}

/// Text with its bold, link and code ranges styled. `wrap` is the width body text may fill;
/// with it, lines break between words only.
pub(super) fn rich_text(rich: &RichText, wrap: Option<Pixels>, cx: &RowContext) -> AnyElement {
    text(&rich.text, &rich.spans, wrap, cx)
}

fn plain(content: &str, cx: &RowContext) -> AnyElement {
    text(content, &[], None, cx)
}

/// Draws the next of the row's texts. It can be selected, and its links clicked. Calls must
/// come in the order of [`Row::texts`], because that order is how a selection names a text.
fn text(content: &str, spans: &[Span], wrap: Option<Pixels>, cx: &RowContext) -> AnyElement {
    let ordinal = cx.drawn.replace(cx.drawn.get() + 1);
    debug_assert_eq!(
        cx.row.texts().get(ordinal).copied(),
        Some(content),
        "drawing order differs from Row::texts"
    );

    let on_accent = cx.row.side == Side::Mine
        && matches!(cx.row.kind, RowKind::Part(Part::Bubble(_) | Part::Task(_)));
    let link_color = theme::color(if on_accent {
        theme::ON_MINE
    } else {
        theme::LINK
    });
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(spans.len() + 1);
    let mut links: Vec<(Range<usize>, SharedString)> = Vec::new();
    for span in spans {
        let style = match &span.kind {
            SpanKind::Bold => HighlightStyle {
                font_weight: Some(FontWeight::SEMIBOLD),
                ..Default::default()
            },
            SpanKind::Italic => HighlightStyle {
                font_style: Some(gpui::FontStyle::Italic),
                ..Default::default()
            },
            SpanKind::Code => HighlightStyle {
                background_color: Some(theme::alpha(0xffffff, 0.08).into()),
                ..Default::default()
            },
            SpanKind::Muted => HighlightStyle {
                color: Some(theme::color(theme::TEXT_MUTED)),
                ..Default::default()
            },
            SpanKind::Link(url) => {
                links.push((span.range.clone(), url.clone().into()));
                HighlightStyle {
                    color: Some(link_color),
                    underline: Some(UnderlineStyle {
                        thickness: px(1.),
                        color: Some(link_color),
                        wavy: false,
                    }),
                    ..Default::default()
                }
            }
        };
        highlights.push((span.range.clone(), style));
    }
    if let Some(selected) = cx
        .selection
        .borrow()
        .range_in(cx.index, ordinal, content.len())
    {
        let tint = if on_accent {
            theme::alpha(0x000000, 0.22)
        } else {
            theme::alpha(theme::LINK, 0.38)
        };
        highlights.push((
            selected,
            HighlightStyle {
                background_color: Some(tint.into()),
                ..Default::default()
            },
        ));
    }
    let shown = match wrap {
        Some(width) => keep_words(content, width, cx.window),
        None => SharedString::from(content.to_owned()),
    };
    SelectableText {
        text: StyledText::new(shown.clone()).with_highlights(flatten(highlights)),
        content: shown,
        row: cx.index,
        ordinal,
        selection: cx.selection.clone(),
        links,
        on_link: cx.on_link.clone(),
    }
    .into_any_element()
}

/// GPUI wants highlight ranges sorted and non-overlapping, and markup nests (a link inside bold
/// text inside a quote). Every span start and end becomes a cut, and each piece between two
/// cuts gets the styles of all spans that cover it, the innermost applied last.
fn flatten(
    mut highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    // Widest first, so that a narrower span's color or weight wins where they overlap.
    highlights.sort_by_key(|(range, _)| std::cmp::Reverse(range.len()));
    let mut cuts: Vec<usize> = highlights
        .iter()
        .flat_map(|(range, _)| [range.start, range.end])
        .collect();
    cuts.sort_unstable();
    cuts.dedup();
    let mut flat = Vec::with_capacity(cuts.len());
    for piece in cuts.windows(2) {
        let (start, end) = (piece[0], piece[1]);
        let style = highlights
            .iter()
            .filter(|(range, _)| range.start <= start && end <= range.end)
            .map(|(_, style)| *style)
            .reduce(HighlightStyle::highlight);
        if let Some(style) = style {
            flat.push((start..end, style));
        }
    }
    flat
}

fn info_card(surface: Div, card: &Card, cx: &RowContext) -> Div {
    let padding = px(18.);
    let wrap = Some(cx.content_width * THEIRS_MAX - padding * 2.);
    let small = || {
        div()
            .text_size(theme::SMALL)
            .line_height(theme::SMALL_LINE)
            .text_color(theme::color(theme::TEXT_MUTED))
    };
    let mut body = surface
        .w_full()
        .px(padding)
        .pt(px(20.))
        .pb(px(18.))
        .flex()
        .flex_col();
    if let Some(title) = &card.title {
        body = body.child(
            div()
                .font_weight(FontWeight::SEMIBOLD)
                .child(rich_text(title, None, cx)),
        );
    }
    if let Some(subtitle) = &card.subtitle {
        body = body.child(small().pt(px(4.)).child(rich_text(subtitle, None, cx)));
    }
    let mut rows = div().flex().flex_col().gap(px(9.));
    if card.title.is_some() || card.subtitle.is_some() {
        rows = rows.pt(px(12.));
    }
    for row in &card.rows {
        rows = rows.child(match row {
            CardRow::Field { label, value } => div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(small().child(rich_text(label, None, cx)))
                .child(rich_text(value, wrap, cx)),
            CardRow::Heading(heading) => div()
                .pt(px(4.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(rich_text(heading, None, cx)),
            CardRow::Text(text) => div().child(rich_text(text, wrap, cx)),
            CardRow::Table { cells, header } => {
                let line = if *header { small() } else { div() }.flex().gap(px(10.));
                line.children(
                    cells
                        .iter()
                        .map(|cell| div().flex_1().min_w_0().child(rich_text(cell, None, cx))),
                )
            }
        });
    }
    body.child(rows)
}

fn request_card(surface: Div, request: &Request, cx: &RowContext) -> Div {
    let faint = theme::color(theme::TEXT_FAINT);
    let mut card = surface
        .w(px(REQUEST_WIDTH))
        .max_w_full()
        .p(px(16.))
        .pt(px(20.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .text_color(faint)
                .child(
                    svg()
                        .path("icons/shield-keyhole.svg")
                        .size(px(18.))
                        .flex_none()
                        .text_color(faint),
                )
                .child(plain(&request.caption, cx)),
        );
    if let Some(question) = &request.question {
        card = card.child(plain(question, cx));
    }
    let settled = request.status != RequestStatus::Pending;
    for option in &request.options {
        card = card.child(
            inset_pill()
                .justify_start()
                .when(option.chosen, |pill| {
                    pill.bg(theme::color(theme::CHROME_SELECTED))
                })
                .when(settled && !option.chosen, |pill| pill.text_color(faint))
                .child(plain(&option.label, cx)),
        );
    }
    let status = match request.status {
        RequestStatus::Expired => Some("Expired"),
        RequestStatus::Pending if request.options.is_empty() => Some("Waiting for your answer"),
        RequestStatus::Answered if request.options.is_empty() => Some("Answered"),
        _ => None,
    };
    if let Some(status) = status {
        card = card.child(inset_pill().text_color(faint).child(status));
    }
    card
}

/// The darker pill inside a request card.
fn inset_pill() -> Div {
    div()
        .min_h(px(50.))
        .px(px(18.))
        .py(px(10.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(25.))
        .bg(theme::color(theme::SURFACE_INSET))
}

/// The bubble shown while the character works on a reply.
fn activity(label: Option<&str>) -> impl IntoElement {
    div()
        .px(px(14.))
        .py(px(11.))
        .rounded(theme::BUBBLE_RADIUS)
        .bg(theme::color(theme::SURFACE))
        .text_color(theme::color(theme::TEXT_MUTED))
        .child(SharedString::from(
            label.map_or_else(|| "•••".to_owned(), |label| format!("{label}…")),
        ))
        .with_animation(
            "activity",
            Animation::new(Duration::from_millis(1400))
                .repeat()
                .with_easing(gpui::pulsating_between(0.45, 1.)),
            |bubble, delta| bubble.opacity(delta),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_spans_flatten_into_ordered_pieces_with_combined_styles() {
        let bold = HighlightStyle {
            font_weight: Some(FontWeight::SEMIBOLD),
            ..Default::default()
        };
        let muted = HighlightStyle {
            color: Some(theme::color(theme::TEXT_MUTED)),
            ..Default::default()
        };
        let link = HighlightStyle {
            color: Some(theme::color(theme::LINK)),
            ..Default::default()
        };
        // A quote (0..30) holding bold text (2..20) holding a link (5..12), given innermost first
        // as the parser produces them, plus a separate span after a gap.
        let flat = flatten(vec![
            (5..12, link),
            (2..20, bold),
            (0..30, muted),
            (40..44, bold),
        ]);

        let ranges: Vec<Range<usize>> = flat.iter().map(|(range, _)| range.clone()).collect();
        assert_eq!(ranges, [0..2, 2..5, 5..12, 12..20, 20..30, 40..44]);
        // Inside the link: the link's color, still bold. Outside it: muted, bold only where bold.
        assert_eq!(flat[2].1.color, link.color);
        assert_eq!(flat[2].1.font_weight, bold.font_weight);
        assert_eq!(
            (flat[1].1.color, flat[1].1.font_weight),
            (muted.color, bold.font_weight)
        );
        assert_eq!(
            (flat[4].1.color, flat[4].1.font_weight),
            (muted.color, None)
        );
    }
}
