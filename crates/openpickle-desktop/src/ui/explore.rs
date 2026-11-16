//! The Explore tab: what the character offers to do, in sections, with a filter by area and
//! a sheet for one offer.

use std::{collections::HashSet, sync::Arc, time::Duration};

use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    ListAlignment, ListState, MouseButton, ObjectFit, SharedString, Subscription, Task, Window,
    div, img, list, prelude::*, px, relative,
};

use crate::{
    model::{Explore, Offer},
    theme,
    ui::{
        Cancel, Identity, Pictures, Tab, header_cover, icon, tab_bar, tab_header,
        wrap::{keep_words, keep_words_at},
    },
};

pub enum ExploreEvent {
    Menu,
    Invite,
    Open(Tab),
    /// Ask the character for this offer, by sending its text to the chat.
    Ask {
        offer: String,
        text: String,
    },
    /// These offers were on screen for the first time.
    Shown(Vec<String>),
    /// Read the offers again.
    Reload,
}

/// The filter chips, in the app's order: area, label, icon. The empty area is "For you".
const AREAS: [(&str, &str, &str); 9] = [
    ("", "For you", "sparkles"),
    ("money", "Benefits", "banknote"),
    ("shopping", "Shopping", "shopping-bag"),
    ("travel", "Travel", "plane"),
    ("people", "People", "users-round"),
    ("fun", "Culture", "ticket"),
    ("home", "Home & life", "house"),
    ("health", "Health", "heart-pulse"),
    ("work", "Work & study", "briefcase"),
];

/// One row of the list.
enum Row {
    Title,
    Chips,
    Section {
        id: String,
        title: SharedString,
        /// Whether the section has more than it shows, and whether it shows it.
        expandable: bool,
        expanded: bool,
    },
    Heading(&'static str),
    Offer {
        offer: Arc<Offer>,
        divider: bool,
    },
    Note(SharedString),
    Retry(SharedString),
}

pub struct ExploreView {
    identity: Identity,
    pictures: Entity<Pictures>,
    explore: Explore,
    /// Why the offers could not be read, when nothing is shown instead.
    failed: Option<SharedString>,
    /// The chosen area. Empty is "For you".
    area: String,
    /// The section of "For you" that shows all of its offers.
    expanded: Option<String>,
    rows: Vec<Row>,
    list: ListState,
    /// The offer whose sheet is open.
    detail: Option<Arc<Offer>>,
    /// Offers that were asked for, which are drawn dimmed.
    asked: HashSet<String>,
    /// Offers the server was told about.
    seen: HashSet<String>,
    _seen: Option<Task<()>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ExploreEvent> for ExploreView {}

impl ExploreView {
    /// How long offers must stay on screen before the server hears of them.
    const SEEN_AFTER: Duration = Duration::from_millis(700);

    pub fn new(identity: Identity, pictures: Entity<Pictures>, cx: &mut Context<Self>) -> Self {
        let list = ListState::new(0, ListAlignment::Top, px(400.));
        let view = cx.entity().downgrade();
        list.set_scroll_handler(move |_, _, cx| {
            view.update(cx, |this, cx| this.watch_seen(cx)).ok();
        });
        let subscriptions = vec![cx.observe(&pictures, |_, _, cx| cx.notify())];
        let mut view = Self {
            identity,
            pictures,
            explore: Explore::default(),
            failed: None,
            area: String::new(),
            expanded: None,
            rows: Vec::new(),
            list,
            detail: None,
            asked: HashSet::new(),
            seen: HashSet::new(),
            _seen: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        };
        view.rebuild(true);
        view
    }

    pub fn set_identity(&mut self, identity: Identity, cx: &mut Context<Self>) {
        self.identity = identity;
        cx.notify();
    }

    /// Takes in the offers, or why they could not be read. A failure keeps what is shown.
    pub fn set_explore(&mut self, explore: Result<Explore, String>, cx: &mut Context<Self>) {
        match explore {
            Ok(explore) if explore == self.explore => self.failed = None,
            Ok(explore) => {
                self.explore = explore;
                self.failed = None;
                self.rebuild(false);
            }
            Err(reason) => {
                self.failed = Some(reason.into());
                self.rebuild(false);
            }
        }
        self.watch_seen(cx);
        cx.notify();
    }

    /// Lays the offers out as rows. `to_top` starts the list over; otherwise it stays where
    /// it is scrolled to.
    fn rebuild(&mut self, to_top: bool) {
        let rows = rows_of(
            &self.explore,
            &self.area,
            self.expanded.as_deref(),
            self.failed.as_ref(),
        );
        if to_top {
            self.list.reset(rows.len());
        } else {
            self.list.splice(0..self.rows.len(), rows.len());
        }
        self.rows = rows;
    }

    fn choose_area(&mut self, area: &str, cx: &mut Context<Self>) {
        self.area = area.to_owned();
        self.expanded = None;
        self.rebuild(true);
        self.watch_seen(cx);
        cx.notify();
    }

    fn toggle_section(&mut self, id: &str, cx: &mut Context<Self>) {
        self.expanded = (self.expanded.as_deref() != Some(id)).then(|| id.to_owned());
        self.rebuild(false);
        self.watch_seen(cx);
        cx.notify();
    }

    /// Asks for an offer and closes its sheet. The window goes to the chat, where the
    /// message shows.
    fn ask(&mut self, offer: &Offer, cx: &mut Context<Self>) {
        self.detail = None;
        if self.asked.insert(offer.id.clone()) {
            cx.emit(ExploreEvent::Ask {
                offer: offer.id.clone(),
                text: offer.ask.clone(),
            });
        }
        cx.notify();
    }

    /// Waits until the list holds still, then tells the server which offers were on screen.
    fn watch_seen(&mut self, cx: &mut Context<Self>) {
        self._seen = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Self::SEEN_AFTER).await;
            this.update(cx, |this, cx| {
                let bottom = this.list.viewport_bounds().bottom();
                let first = this.list.logical_scroll_top().item_ix;
                let mut shown = Vec::new();
                for (index, row) in this.rows.iter().enumerate().skip(first) {
                    match this.list.bounds_for_item(index) {
                        Some(bounds) if bounds.top() < bottom => {}
                        _ => break,
                    }
                    if let Row::Offer { offer, .. } = row
                        && this.seen.insert(offer.id.clone())
                    {
                        shown.push(offer.id.clone());
                    }
                }
                if !shown.is_empty() {
                    cx.emit(ExploreEvent::Shown(shown));
                }
            })
            .ok();
        }));
    }

    /// The offer's picture, or its symbol on a disc until the picture is here.
    fn picture(&self, offer: &Offer, cx: &App) -> AnyElement {
        match self.pictures.read(cx).get(&offer.picture) {
            Some(source) => img(source)
                .size(px(37.))
                .flex_none()
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            None => div()
                .size(px(37.))
                .flex_none()
                .rounded_full()
                .bg(theme::alpha(0xffffff, 0.09))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(symbol_icon(&offer.symbol), 17., theme::TEXT_MUTED))
                .into_any_element(),
        }
    }

    fn row(&self, index: usize, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let column = window.viewport_size().width.min(theme::COLUMN_MAX);
        let line = div().w_full().flex().justify_center();
        let column_box = div().w_full().max_w(theme::COLUMN_MAX);
        let inner = match &self.rows[index] {
            Row::Title => column_box
                .px(px(20.))
                .text_size(px(20.))
                .line_height(px(26.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Explore")
                .into_any_element(),
            Row::Chips => self.chips(cx).into_any_element(),
            Row::Section {
                id,
                title,
                expandable,
                expanded,
            } => {
                let id = id.clone();
                column_box
                    .px(px(20.))
                    .pt(px(24.))
                    .pb(px(8.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(
                        div()
                            .text_size(theme::BODY)
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(theme::color(theme::TEXT_FAINT))
                            .truncate()
                            .child(title.clone()),
                    )
                    .when(*expandable, |header| {
                        header.child(
                            div()
                                .id(("see-all", index))
                                .flex_none()
                                .text_size(px(14.))
                                .text_color(theme::color(theme::TEXT_MUTED))
                                .cursor_pointer()
                                .child(if *expanded { "Show less" } else { "See all" })
                                .on_click(
                                    cx.listener(move |this, _, _, cx| this.toggle_section(&id, cx)),
                                ),
                        )
                    })
                    .into_any_element()
            }
            Row::Heading(heading) => column_box
                .px(px(20.))
                .pt(px(24.))
                .pb(px(8.))
                .text_size(theme::BODY)
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme::color(theme::TEXT_FAINT))
                .child(*heading)
                .into_any_element(),
            Row::Offer { offer, divider } => {
                // The text column: the window less the two insets, the picture and the gap.
                let room = column - px(10. * 2. + 10. * 2. + 37. + 12.);
                let opened = offer.clone();
                let text = match offer.title.is_empty() {
                    true => div()
                        .text_size(theme::LABEL)
                        .line_height(theme::LABEL_LINE)
                        .child(keep_words_at(&offer.detail, theme::LABEL, room, window)),
                    false => div()
                        .flex()
                        .flex_col()
                        .gap(px(6.))
                        .child(
                            div()
                                .text_size(theme::LABEL)
                                .line_height(theme::LABEL_LINE)
                                .font_weight(FontWeight::MEDIUM)
                                .child(keep_words_at(&offer.title, theme::LABEL, room, window)),
                        )
                        .child(
                            div()
                                .text_color(theme::color(theme::TEXT_MUTED))
                                .child(keep_words(&offer.detail, room, window)),
                        ),
                };
                let footer = div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8.))
                    .child(match offer.asked_by.is_empty() {
                        true => div(),
                        false => div()
                            .min_w_0()
                            .flex()
                            .items_center()
                            .gap(px(5.))
                            .text_size(px(14.))
                            .text_color(theme::color(theme::TEXT_FAINT))
                            .child(icon("users-round", 13., theme::TEXT_FAINT))
                            .child(div().truncate().child(offer.asked_by.clone())),
                    })
                    .child(
                        div()
                            .flex_none()
                            .h(px(29.))
                            .px(px(12.))
                            .rounded_full()
                            .bg(theme::alpha(0xffffff, 0.09))
                            .flex()
                            .items_center()
                            .text_size(px(14.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Ask"),
                    );
                column_box
                    .px(px(10.))
                    .child(
                        div()
                            .id(("offer", index))
                            .px(px(10.))
                            .py(px(16.))
                            .rounded(px(16.))
                            .flex()
                            .items_start()
                            .gap(px(12.))
                            .when(self.asked.contains(&offer.id), |row| row.opacity(0.4))
                            .cursor_pointer()
                            .hover(|row| row.bg(theme::alpha(0xffffff, 0.05)))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.detail = Some(opened.clone());
                                cx.notify();
                            }))
                            .child(self.picture(offer, cx))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(12.))
                                    .child(text)
                                    .child(footer),
                            ),
                    )
                    .when(*divider, |row| {
                        row.child(div().mx(px(10.)).h(px(1.)).bg(theme::alpha(0xffffff, 0.14)))
                    })
                    .into_any_element()
            }
            Row::Note(note) => column_box
                .px(px(20.))
                .pt(px(40.))
                .flex()
                .justify_center()
                .text_color(theme::alpha(0xffffff, 0.6))
                .child(note.clone())
                .into_any_element(),
            Row::Retry(reason) => column_box
                .px(px(20.))
                .pt(px(40.))
                .flex()
                .flex_col()
                .items_center()
                .gap(px(14.))
                .text_center()
                .text_color(theme::alpha(0xffffff, 0.6))
                .child(reason.clone())
                .child(
                    div()
                        .id("retry")
                        .h(px(36.))
                        .px(px(16.))
                        .rounded_full()
                        .bg(theme::alpha(0xffffff, 0.09))
                        .flex()
                        .items_center()
                        .text_color(theme::color(theme::TEXT))
                        .font_weight(FontWeight::MEDIUM)
                        .cursor_pointer()
                        .child("Try again")
                        .on_click(cx.listener(|_, _, _, cx| cx.emit(ExploreEvent::Reload))),
                )
                .into_any_element(),
        };
        line.child(inner).into_any_element()
    }

    /// The row of filters: "For you" and the areas the server lists.
    fn chips(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let for_you = is_for_you(&self.explore, &self.area);
        let chips = AREAS
            .iter()
            .filter(|(area, ..)| area.is_empty() || self.explore.areas.iter().any(|a| a == area))
            .map(|&(area, label, symbol)| {
                let chosen = if area.is_empty() {
                    for_you
                } else {
                    self.area == area
                };
                div()
                    .id(label)
                    .flex_none()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(4.))
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_area(area, cx)))
                    .child(
                        div()
                            .size(px(50.))
                            .rounded_full()
                            .bg(theme::alpha(0xffffff, if chosen { 0.14 } else { 0.05 }))
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(icon(symbol, 24., theme::TEXT)),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(15.))
                            .when(chosen, |label| label.font_weight(FontWeight::MEDIUM))
                            .text_color(theme::color(if chosen {
                                theme::TEXT
                            } else {
                                theme::TEXT_MUTED
                            }))
                            .child(label),
                    )
            });
        div()
            .id("areas")
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .pt(px(16.))
            .px(px(20.))
            .flex()
            .gap(px(12.))
            .overflow_x_scroll()
            .children(chips)
    }

    /// The sheet of one offer: what it is, how the character goes about it, and Start.
    fn sheet(&self, offer: &Arc<Offer>, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let width = window.viewport_size().width.min(px(520.));
        let room = width - px(24. * 2.);
        let asked = offer.clone();
        let heading = if offer.title.is_empty() {
            &offer.detail
        } else {
            &offer.title
        };
        let steps = offer.steps.iter().enumerate().map(|(index, step)| {
            div()
                .flex()
                .items_start()
                .gap(px(10.))
                .child(
                    div()
                        .size(px(20.))
                        .flex_none()
                        .rounded_full()
                        .bg(theme::alpha(0xffffff, 0.09))
                        .flex()
                        .items_center()
                        .justify_center()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child((index + 1).to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_height(px(21.))
                        .text_color(theme::color(0xc9cdd6))
                        .child(keep_words(step, room - px(30.), window)),
                )
        });
        let panel = div()
            .id("offer-sheet")
            .occlude()
            .w(width)
            .max_h(relative(0.86))
            .overflow_y_scroll()
            .rounded_t(px(28.))
            .bg(theme::color(theme::SHEET))
            .pt(px(28.))
            .px(px(24.))
            .pb(px(20.))
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(20.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(16.))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(12.))
                                    .child(self.picture(offer, cx))
                                    .child(
                                        div()
                                            .flex_1()
                                            .min_w_0()
                                            .text_size(px(18.))
                                            .line_height(px(24.))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child(keep_words_at(
                                                heading,
                                                px(18.),
                                                room - px(49.),
                                                window,
                                            )),
                                    ),
                            )
                            .when(!offer.title.is_empty(), |top| {
                                top.child(
                                    div()
                                        .text_color(theme::color(theme::TEXT_MUTED))
                                        .child(keep_words(&offer.detail, room, window)),
                                )
                            }),
                    )
                    .when(!offer.steps.is_empty(), |body| {
                        body.child(div().h(px(1.)).bg(theme::alpha(0xffffff, 0.08)))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap(px(8.))
                                    .child(
                                        div()
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .child("Here's how I'll do it"),
                                    )
                                    .children(steps),
                            )
                    }),
            )
            .child(
                div()
                    .id("start")
                    .h(px(48.))
                    .flex_none()
                    .rounded_full()
                    .bg(theme::color(theme::TEXT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(theme::LABEL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme::color(theme::PAGE))
                    .cursor_pointer()
                    .child("Start")
                    .on_click(cx.listener(move |this, _, _, cx| this.ask(&asked, cx))),
            );
        // A press beside the sheet closes it.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(theme::alpha(0x000000, 0.5))
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.detail = None;
                    cx.notify();
                }),
            )
            .child(panel)
            .into_any_element()
    }
}

/// Whether an area is "For you": the empty one, or one the server no longer lists.
fn is_for_you(explore: &Explore, area: &str) -> bool {
    area.is_empty() || !explore.areas.iter().any(|listed| listed == area)
}

/// The rows of the list for the chosen area. `expanded` is the section of "For you" that
/// shows all of its offers; `failed` says why the offers could not be read.
fn rows_of(
    explore: &Explore,
    area: &str,
    expanded: Option<&str>,
    failed: Option<&SharedString>,
) -> Vec<Row> {
    let mut rows = vec![Row::Title];
    if !explore.areas.is_empty() {
        rows.push(Row::Chips);
    }
    let offers = |rows: &mut Vec<Row>, offers: Vec<&Arc<Offer>>| {
        let last = offers.len().saturating_sub(1);
        rows.extend(
            offers
                .into_iter()
                .enumerate()
                .map(|(index, offer)| Row::Offer {
                    offer: offer.clone(),
                    divider: index < last,
                }),
        );
    };
    if is_for_you(explore, area) {
        for section in explore
            .sections
            .iter()
            .filter(|section| section.preview > 0)
        {
            let open = expanded == Some(section.id.as_str());
            rows.push(Row::Section {
                id: section.id.clone(),
                title: section.title.clone().into(),
                expandable: section.expandable,
                expanded: open,
            });
            // An expanded section shows the rest too, except what "popular" showed already.
            let shown = section.offers.iter().enumerate().filter(|(index, offer)| {
                *index < section.preview || (open && !explore.popular.contains(&offer.id))
            });
            offers(&mut rows, shown.map(|(_, offer)| offer).collect());
        }
        // The offers no section previews. Those of the expanded section moved up.
        let more: Vec<&Arc<Offer>> = explore
            .more
            .iter()
            .filter(|offer| expanded != Some(offer.section.as_str()))
            .collect();
        if !more.is_empty() {
            rows.push(Row::Heading("More to explore"));
            offers(&mut rows, more);
        }
    } else {
        let ranked = explore
            .ranked
            .iter()
            .filter(|offer| offer.areas.iter().any(|listed| listed == area));
        offers(&mut rows, ranked.collect());
    }
    if explore.sections.is_empty() {
        rows.push(match failed {
            Some(reason) => Row::Retry(reason.clone()),
            None => Row::Note("Nothing here yet.".into()),
        });
    }
    rows
}

/// The icon that stands in for a system symbol the server names.
fn symbol_icon(symbol: &str) -> &'static str {
    match symbol {
        "banknote" => "banknote",
        "doc.text" => "file-text",
        "house" => "house",
        "person.2" => "users-round",
        "airplane" => "plane",
        "cross.case" => "briefcase-medical",
        "bag" => "shopping-bag",
        "ticket" => "ticket",
        _ => "sparkles",
    }
}

impl Render for ExploreView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let offers = list(
            self.list.clone(),
            cx.processor(|this, index: usize, window: &mut Window, cx| this.row(index, window, cx)),
        )
        .size_full()
        .pt(theme::HEADER_HEIGHT - px(10.))
        .pb(px(115.));
        let bar = div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .px(px(20.))
            .pb(px(21.))
            .child(tab_bar(
                Tab::Explore,
                cx.listener(|_, tab: &Tab, _, cx| cx.emit(ExploreEvent::Open(*tab))),
            ));
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                this.detail = None;
                cx.notify();
            }))
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .child(offers)
            .child(header_cover())
            .child(tab_header(
                self.identity.name.clone(),
                self.identity.avatar.clone(),
                cx.listener(|_, _, _, cx| cx.emit(ExploreEvent::Menu)),
                cx.listener(|_, _, _, cx| cx.emit(ExploreEvent::Invite)),
            ))
            .child(bar)
            .children(
                self.detail
                    .clone()
                    .map(|offer| self.sheet(&offer, window, cx)),
            )
    }
}

impl Focusable for ExploreView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::OfferSection;

    fn offer(id: &str, section: &str, area: &str) -> Arc<Offer> {
        Arc::new(Offer {
            id: id.into(),
            section: section.into(),
            title: String::new(),
            detail: String::new(),
            ask: String::new(),
            picture: String::new(),
            symbol: String::new(),
            asked_by: String::new(),
            steps: Vec::new(),
            areas: vec![area.into()],
        })
    }

    /// Two sections: "popular" shows its one offer; "money" previews one of its three, one
    /// of which is the popular offer again.
    fn explore() -> Explore {
        let (p, a, b) = (
            offer("p", "popular", "home"),
            offer("a", "money", "money"),
            offer("b", "money", "travel"),
        );
        Explore {
            areas: vec!["money".into(), "travel".into()],
            popular: vec!["p".into()],
            sections: vec![
                OfferSection {
                    id: "popular".into(),
                    title: "Popular".into(),
                    offers: vec![p.clone()],
                    preview: 1,
                    expandable: false,
                },
                OfferSection {
                    id: "money".into(),
                    title: "Money".into(),
                    offers: vec![a.clone(), b.clone(), p.clone()],
                    preview: 1,
                    expandable: true,
                },
                OfferSection {
                    id: "hidden".into(),
                    title: "Never previewed".into(),
                    offers: vec![offer("h", "hidden", "money")],
                    preview: 0,
                    expandable: false,
                },
            ],
            more: vec![b.clone()],
            ranked: vec![b, a, p],
        }
    }

    fn outline(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| match row {
                Row::Title => "title".to_owned(),
                Row::Chips => "chips".to_owned(),
                Row::Section { id, expanded, .. } => format!("section {id} {expanded}"),
                Row::Heading(_) => "more".to_owned(),
                Row::Offer { offer, divider } => format!("{} {divider}", offer.id),
                Row::Note(_) => "note".to_owned(),
                Row::Retry(_) => "retry".to_owned(),
            })
            .collect()
    }

    #[test]
    fn for_you_previews_each_section_and_lists_the_rest_after_them() {
        let rows = rows_of(&explore(), "", None, None);
        assert_eq!(
            outline(&rows),
            [
                "title",
                "chips",
                "section popular false",
                "p false",
                "section money false",
                "a false",
                "more",
                "b false",
            ]
        );

        // The expanded section shows its other offers, but not the one "popular" shows, and
        // its offers leave "More to explore".
        let rows = rows_of(&explore(), "", Some("money"), None);
        assert_eq!(
            outline(&rows),
            [
                "title",
                "chips",
                "section popular false",
                "p false",
                "section money true",
                "a true",
                "b false",
            ]
        );
    }

    #[test]
    fn an_area_lists_its_offers_by_rank() {
        assert_eq!(
            outline(&rows_of(&explore(), "money", None, None)),
            ["title", "chips", "a false"]
        );
        // An area the server does not list is shown as "For you".
        assert_eq!(
            outline(&rows_of(&explore(), "pets", None, None)).len(),
            outline(&rows_of(&explore(), "", None, None)).len()
        );
    }

    #[test]
    fn nothing_to_show_says_so_or_offers_to_try_again() {
        let empty = Explore::default();
        assert_eq!(outline(&rows_of(&empty, "", None, None)), ["title", "note"]);
        let reason = SharedString::from("Could not reach Pickle.");
        assert_eq!(
            outline(&rows_of(&empty, "", None, Some(&reason))),
            ["title", "retry"]
        );
    }
}
