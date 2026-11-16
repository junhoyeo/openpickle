//! Draws one row of the message list: a bubble, a card, a timestamp.

use std::{cell::Cell, ops::Range, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, App, Div, FontWeight, HighlightStyle, MouseButton, Pixels,
    SharedString, StyledText, UnderlineStyle, Window, div, prelude::*, px, relative, svg,
};

use crate::{
    content::{Card, CardRow, RichText, Span, SpanKind},
    model::{Part, Request, RequestStatus, Row, RowKind, Side},
    theme,
    ui::{
        select::{SelectableText, SharedSelection},
        wrap::keep_words,
    },
};

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
    pub on_link: fn(&str, &mut Window, &mut App),
    /// How many of the row's texts have been drawn so far.
    drawn: Cell<usize>,
}

impl<'a> RowContext<'a> {
    pub fn new(
        window: &'a Window,
        row: &'a Row,
        index: usize,
        selection: &'a SharedSelection,
        on_link: fn(&str, &mut Window, &mut App),
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
            let surface = shaped(row, matches!(part, Part::Bubble(_) | Part::Task(_)));
            let content = match part {
                Part::Bubble(rich) => bubble(surface, rich, cx),
                // A note, not speech: a started task, attachments, a failed send.
                Part::Task(title) => surface
                    .px(px(14.))
                    .py(px(11.))
                    .max_w(relative(THEIRS_MAX))
                    .when(row.side == Side::Theirs, |note| {
                        note.text_color(theme::color(theme::TEXT_MUTED))
                    })
                    .when(row.side == Side::Mine, |note| {
                        note.text_color(theme::alpha(theme::ON_MINE, 0.6))
                    })
                    .child(plain(title, cx)),
                // Cards sit 2 pt further from their neighbors than bubbles do.
                Part::Card(card) => div()
                    .py(px(2.))
                    .w(relative(THEIRS_MAX))
                    .child(info_card(surface, card, cx)),
                Part::Request(request) => {
                    div().py(px(2.)).child(request_card(surface, request, cx))
                }
            };
            line.child(content).into_any_element()
        }
    }
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

/// Text with its bold, link and code ranges styled. `wrap` is the width body text may fill;
/// with it, lines break between words only.
fn rich_text(rich: &RichText, wrap: Option<Pixels>, cx: &RowContext) -> AnyElement {
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
        on_link: cx.on_link,
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
