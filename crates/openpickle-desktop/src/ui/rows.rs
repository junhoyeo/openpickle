//! Draws one row of the message list: a bubble, a card, a timestamp.

use std::{ops::Range, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, App, Div, FontWeight, HighlightStyle, InteractiveText,
    SharedString, StyledText, UnderlineStyle, Window, div, prelude::*, px, relative, svg,
};

use crate::{
    content::{Card, CardRow, RichText, SpanKind},
    model::{Part, Request, RequestStatus, Row, RowKind, Side},
    theme,
};

/// Widths from the design column (408 pt of content): bubbles hug their text up to these.
const THEIRS_MAX: f32 = 394. / 408.;
const MINE_MAX: f32 = 304. / 408.;
const REQUEST_WIDTH: f32 = 342.;

pub fn render_row(row: &Row, index: usize, on_link: fn(&str, &mut Window, &mut App)) -> AnyElement {
    let top_gap = if index == 0 {
        px(0.)
    } else if row.joined_above {
        theme::JOINED_GAP
    } else {
        theme::GROUP_GAP
    };
    let line = div().w_full().flex().px(theme::GUTTER).pt(top_gap);
    match &row.kind {
        RowKind::Time(text) => line
            .justify_center()
            .pb(px(-4.))
            .text_size(px(13.))
            .line_height(px(18.))
            .text_color(theme::color(theme::TEXT_FAINT))
            .child(SharedString::from(text.clone()))
            .into_any_element(),
        RowKind::Activity(label) => line.child(activity(label.as_deref())).into_any_element(),
        RowKind::Part(part) => {
            let line = match row.side {
                Side::Mine => line.justify_end(),
                Side::Theirs => line.justify_start(),
            };
            let surface = shaped(row, matches!(part, Part::Bubble(_) | Part::Task(_)));
            let content = match part {
                Part::Bubble(text) => bubble(surface, row.side, text, index, on_link),
                Part::Task(title) => surface
                    .px(px(14.))
                    .py(px(11.))
                    .max_w(relative(THEIRS_MAX))
                    .text_color(theme::color(theme::TEXT_MUTED))
                    .child(SharedString::from(title.to_string())),
                // Cards sit 2 pt further from their neighbors than bubbles do.
                Part::Card(card) => div()
                    .py(px(2.))
                    .w(relative(THEIRS_MAX))
                    .child(info_card(surface, card, index, on_link)),
                Part::Request(request) => div().py(px(2.)).child(request_card(surface, request)),
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

fn bubble(
    surface: Div,
    side: Side,
    text: &RichText,
    index: usize,
    on_link: fn(&str, &mut Window, &mut App),
) -> Div {
    let max = if side == Side::Mine {
        MINE_MAX
    } else {
        THEIRS_MAX
    };
    surface
        .px(px(14.))
        .py(px(11.))
        .max_w(relative(max))
        .child(rich_text(
            text,
            ("bubble", index),
            side == Side::Mine,
            on_link,
        ))
}

/// Text with its bold, link and code ranges styled; links are clickable.
pub fn rich_text(
    text: &RichText,
    id: (&'static str, usize),
    on_accent: bool,
    on_link: fn(&str, &mut Window, &mut App),
) -> AnyElement {
    let link_color = if on_accent {
        theme::color(theme::ON_MINE)
    } else {
        theme::color(theme::LINK)
    };
    let mut highlights: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(text.spans.len());
    let mut links: Vec<(Range<usize>, SharedString)> = Vec::new();
    for span in &text.spans {
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
    let styled =
        StyledText::new(SharedString::from(text.text.clone())).with_highlights(flatten(highlights));
    if links.is_empty() {
        return styled.into_any_element();
    }
    let ranges = links.iter().map(|(range, _)| range.clone()).collect();
    InteractiveText::new(id, styled)
        .on_click(ranges, move |link, window, cx| {
            on_link(&links[link].1, window, cx)
        })
        .into_any_element()
}

/// GPUI wants highlight ranges sorted and non-overlapping. Nested markup (a link inside bold
/// text) is rare, so an inner range simply wins over the part of the outer one it covers.
fn flatten(
    mut highlights: Vec<(Range<usize>, HighlightStyle)>,
) -> Vec<(Range<usize>, HighlightStyle)> {
    highlights.sort_by_key(|(range, _)| (range.start, std::cmp::Reverse(range.end)));
    let mut flat: Vec<(Range<usize>, HighlightStyle)> = Vec::with_capacity(highlights.len());
    for (range, style) in highlights {
        if let Some((last, last_style)) = flat.last_mut()
            && range.start < last.end
        {
            let (outer_end, outer_style) = (last.end, *last_style);
            last.end = range.start;
            let inner_end = range.end.min(outer_end);
            flat.push((range.start..inner_end, style));
            if inner_end < outer_end {
                flat.push((inner_end..outer_end, outer_style));
            }
        } else {
            flat.push((range, style));
        }
    }
    flat.retain(|(range, _)| !range.is_empty());
    flat
}

fn info_card(
    surface: Div,
    card: &Card,
    index: usize,
    on_link: fn(&str, &mut Window, &mut App),
) -> Div {
    let mut body = surface
        .w_full()
        .px(px(18.))
        .pt(px(20.))
        .pb(px(18.))
        .flex()
        .flex_col();
    if let Some(title) = &card.title {
        body = body.child(div().font_weight(FontWeight::SEMIBOLD).child(rich_text(
            title,
            ("card-title", index),
            false,
            on_link,
        )));
    }
    if let Some(subtitle) = &card.subtitle {
        body = body.child(
            div()
                .pt(px(4.))
                .text_size(theme::SMALL)
                .line_height(theme::SMALL_LINE)
                .text_color(theme::color(theme::TEXT_MUTED))
                .child(rich_text(
                    subtitle,
                    ("card-subtitle", index),
                    false,
                    on_link,
                )),
        );
    }
    let mut rows = div().flex().flex_col().gap(px(9.));
    if card.title.is_some() || card.subtitle.is_some() {
        rows = rows.pt(px(12.));
    }
    for (row_index, row) in card.rows.iter().enumerate() {
        // Each text in a card needs its own id for link clicks.
        let id = index * 1000 + row_index;
        rows = rows.child(match row {
            CardRow::Field { label, value } => div()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(theme::SMALL)
                        .line_height(theme::SMALL_LINE)
                        .text_color(theme::color(theme::TEXT_MUTED))
                        .child(rich_text(label, ("card-label", id), false, on_link)),
                )
                .child(rich_text(value, ("card-value", id), false, on_link)),
            CardRow::Heading(text) => div()
                .pt(px(4.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(rich_text(text, ("card-heading", id), false, on_link)),
            CardRow::Text(text) => div().child(rich_text(text, ("card-text", id), false, on_link)),
            CardRow::Table { cells, header } => {
                let mut line = div().flex().gap(px(10.));
                if *header {
                    line = line
                        .text_color(theme::color(theme::TEXT_MUTED))
                        .text_size(theme::SMALL)
                        .line_height(theme::SMALL_LINE);
                }
                for (cell_index, cell) in cells.iter().enumerate() {
                    line = line.child(div().flex_1().min_w_0().child(rich_text(
                        cell,
                        ("card-cell", id * 16 + cell_index),
                        false,
                        on_link,
                    )));
                }
                line
            }
        });
    }
    body.child(rows)
}

fn request_card(surface: Div, request: &Request) -> Div {
    let mut card = surface
        .w(px(REQUEST_WIDTH))
        .max_w_full()
        .p(px(16.))
        .pt(px(20.))
        .flex()
        .flex_col()
        .gap(px(14.));
    card = card.child(
        div()
            .flex()
            .items_center()
            .gap(px(8.))
            .text_color(theme::color(theme::TEXT_FAINT))
            .child(
                svg()
                    .path("icons/shield-keyhole.svg")
                    .size(px(18.))
                    .flex_none()
                    .text_color(theme::color(theme::TEXT_FAINT)),
            )
            .child(SharedString::from(request.caption.clone())),
    );
    if let Some(question) = &request.question {
        card = card.child(SharedString::from(question.clone()));
    }
    for option in &request.options {
        let label = div()
            .flex_1()
            .child(SharedString::from(option.label.clone()));
        card = card.child(
            inset_pill()
                .justify_start()
                .when(option.chosen, |pill| {
                    pill.bg(theme::color(theme::CHROME_SELECTED))
                })
                .when(
                    !option.chosen && request.status != RequestStatus::Pending,
                    |pill| pill.text_color(theme::color(theme::TEXT_FAINT)),
                )
                .child(label),
        );
    }
    let status = match request.status {
        RequestStatus::Pending if request.options.is_empty() => Some("Waiting for your answer"),
        RequestStatus::Expired => Some("Expired"),
        RequestStatus::Answered if request.options.is_empty() => Some("Answered"),
        _ => None,
    };
    if let Some(status) = status {
        card = card.child(
            inset_pill()
                .text_color(theme::color(theme::TEXT_FAINT))
                .child(status),
        );
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
