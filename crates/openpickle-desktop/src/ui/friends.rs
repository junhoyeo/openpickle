//! The Friends section: who asked to be friends, the friends themselves, and the way to the
//! invite sheet.

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString,
    Stateful, Subscription, Window, div, prelude::*, px,
};

use crate::{
    model::{Person, Social},
    theme,
    ui::{
        Cancel, Identity, Pictures, Tab, face, header_cover, icon,
        input::{InputEvent, TextInput},
        tab_bar, tab_header,
    },
};

pub enum FriendsEvent {
    Menu,
    Invite,
    Open(Tab),
    /// Read the friends again.
    Reload,
}

/// The list gets a search field from this many friends on.
const SEARCH_FROM: usize = 10;

pub struct FriendsView {
    identity: Identity,
    pictures: Entity<Pictures>,
    /// `None` until the friends were read once.
    social: Option<Social>,
    /// Why they could not be read, when nothing is shown instead.
    failed: Option<SharedString>,
    requests_folded: bool,
    friends_folded: bool,
    search: Entity<TextInput>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<FriendsEvent> for FriendsView {}

impl FriendsView {
    pub fn new(identity: Identity, pictures: Entity<Pictures>, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| TextInput::new("Search", cx));
        let subscriptions = vec![
            cx.observe(&pictures, |_, _, cx| cx.notify()),
            cx.subscribe(&search, |_, _, event, cx| {
                if matches!(event, InputEvent::Changed) {
                    cx.notify();
                }
            }),
        ];
        Self {
            identity,
            pictures,
            social: None,
            failed: None,
            requests_folded: false,
            friends_folded: false,
            search,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_identity(&mut self, identity: Identity, cx: &mut Context<Self>) {
        self.identity = identity;
        cx.notify();
    }

    /// Takes in the friends, or why they could not be read. A failure keeps what is shown.
    pub fn set_social(&mut self, social: Result<Social, String>, cx: &mut Context<Self>) {
        match social {
            Ok(social) => {
                self.social = Some(social);
                self.failed = None;
            }
            Err(reason) => self.failed = Some(reason.into()),
        }
        cx.notify();
    }

    fn person(&self, person: &Person, cx: &App) -> Div {
        div()
            .flex()
            .items_center()
            .gap(px(12.))
            .py(px(6.))
            .child(face(self.pictures.read(cx).get(&person.picture), 44.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(theme::LABEL)
                    .line_height(px(21.))
                    .font_weight(FontWeight::MEDIUM)
                    .truncate()
                    .child(person.name.clone()),
            )
    }

    /// A card that folds: its title and count, and the chevron that says which way it is.
    fn folding(id: &'static str, title: &'static str, count: usize, folded: bool) -> Stateful<Div> {
        div()
            .id(id)
            .h(px(21.))
            .flex()
            .items_center()
            .gap(px(5.))
            .cursor_pointer()
            .child(
                div()
                    .text_size(theme::LABEL)
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(title),
            )
            .child(
                div()
                    .flex_1()
                    .text_color(theme::color(theme::TEXT_FAINT))
                    .child(count.to_string()),
            )
            .child(icon(
                if folded {
                    "chevron-right"
                } else {
                    "chevron-down"
                },
                17.,
                theme::TEXT,
            ))
    }
}

fn card() -> Div {
    div()
        .w_full()
        .rounded(px(24.))
        .bg(theme::color(theme::CARD))
        .p(px(18.))
        .flex()
        .flex_col()
        .gap(px(10.))
}

impl Render for FriendsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut content = div()
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .px(px(20.))
            .pt(theme::HEADER_HEIGHT - px(10.))
            .pb(px(120.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .text_size(px(20.))
                    .line_height(px(26.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Friends"),
            );
        let invite = |label: &'static str, cx: &mut Context<Self>| {
            card().p(px(0.)).child(
                div()
                    .id("invite")
                    .px(px(20.))
                    .py(px(18.))
                    .rounded(px(24.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .cursor_pointer()
                    .hover(|row| row.bg(theme::alpha(0xffffff, 0.04)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(FriendsEvent::Invite)))
                    .child(icon("gift", 21., theme::TEXT))
                    .child(
                        div()
                            .flex_1()
                            .text_size(theme::LABEL)
                            .font_weight(FontWeight::MEDIUM)
                            .child(label),
                    )
                    .child(icon("chevron-right", 18., theme::TEXT)),
            )
        };
        match &self.social {
            None => {
                if let Some(reason) = &self.failed {
                    content = content.child(
                        card()
                            .items_center()
                            .gap(px(14.))
                            .text_center()
                            .text_color(theme::color(theme::TEXT_MUTED))
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
                                    .on_click(
                                        cx.listener(|_, _, _, cx| cx.emit(FriendsEvent::Reload)),
                                    ),
                            ),
                    );
                }
            }
            Some(social) => {
                if !social.requests.is_empty() {
                    let folded = self.requests_folded;
                    let header =
                        Self::folding("requests", "Friend requests", social.requests.len(), folded)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.requests_folded = !this.requests_folded;
                                cx.notify();
                            }));
                    let rows = social.requests.iter().map(|person| self.person(person, cx));
                    content = content.child(
                        card()
                            .child(header)
                            .when(!folded, |card| card.children(rows)),
                    );
                }
                if social.friends.is_empty() {
                    content = content.child(
                        card()
                            .py(px(28.))
                            .items_center()
                            .gap(px(8.))
                            .text_center()
                            .child(
                                div()
                                    .text_size(px(17.))
                                    .line_height(px(22.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .child("No Pickle friends yet"),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(18.))
                                    .text_color(theme::color(theme::TEXT_MUTED))
                                    .child("Invite someone, or add each other in the Pickle app."),
                            ),
                    );
                    content = content.child(invite("Send an invite", cx));
                } else {
                    let folded = self.friends_folded;
                    let header = Self::folding("friends", "Friends", social.friends.len(), folded)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.friends_folded = !this.friends_folded;
                            cx.notify();
                        }));
                    let query = self.search.read(cx).text().trim().to_lowercase();
                    let searchable = social.friends.len() >= SEARCH_FROM;
                    let rows = social
                        .friends
                        .iter()
                        .filter(|friend| {
                            !searchable
                                || query.is_empty()
                                || friend.name.to_lowercase().contains(&query)
                        })
                        .map(|friend| self.person(friend, cx));
                    let field = div()
                        .id("friend-search")
                        .h(px(44.))
                        .px(px(14.))
                        .rounded(px(14.))
                        .bg(theme::alpha(0xffffff, 0.06))
                        .flex()
                        .items_center()
                        .gap(px(10.))
                        .on_click(cx.listener(|this, _, window, cx| {
                            window.focus(&this.search.focus_handle(cx));
                        }))
                        .child(icon("search", 18., theme::TEXT))
                        .child(self.search.clone());
                    content = content.child(card().child(header).when(!folded, |card| {
                        card.when(searchable, |card| card.child(field))
                            .child(div().flex().flex_col().gap(px(4.)).children(rows))
                    }));
                    content = content.child(invite("Invite friends to Pickle", cx));
                }
                if social.blocked > 0 {
                    content = content.child(
                        div()
                            .px(px(10.))
                            .pt(px(10.))
                            .flex()
                            .gap(px(6.))
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::color(theme::TEXT_MUTED))
                            .child("Blocked")
                            .child(
                                div()
                                    .font_weight(FontWeight::NORMAL)
                                    .text_color(theme::color(theme::TEXT_FAINT))
                                    .child(social.blocked.to_string()),
                            ),
                    );
                }
            }
        }
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
                Tab::Friends,
                cx.listener(|_, tab: &Tab, _, cx| cx.emit(FriendsEvent::Open(*tab))),
            ));
        let _ = window;
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Cancel, window, cx| {
                // Escape leaves the search field.
                window.focus(&this.focus);
                cx.notify();
            }))
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .child(
                // A column, so the content is as tall as it is: in a row it would be stretched
                // to the window's height and leave nothing to scroll.
                div()
                    .id("friends-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(content),
            )
            .child(header_cover())
            .child(tab_header(
                self.identity.name.clone(),
                self.identity.avatar.clone(),
                cx.listener(|_, _, _, cx| cx.emit(FriendsEvent::Menu)),
                cx.listener(|_, _, _, cx| cx.emit(FriendsEvent::Invite)),
                true,
            ))
            .child(bar)
    }
}

impl Focusable for FriendsView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use gpui::{Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, point, size};

    use super::*;

    fn friend(n: usize) -> Person {
        Person {
            id: n.to_string(),
            name: format!("친구 {n}"),
            picture: String::new(),
            note: String::new(),
        }
    }

    #[gpui::test]
    fn a_long_list_of_friends_scrolls_to_its_end(cx: &mut TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (view, cx) = cx.add_window_view(|_, cx| {
            let pictures = cx.new(|_| Pictures::default());
            FriendsView::new(Identity::default(), pictures, cx)
        });
        cx.simulate_resize(size(px(440.), px(900.)));
        let social = Social {
            friends: (0..30).map(friend).collect(),
            ..Social::default()
        };
        view.update(cx, |view, cx| view.set_social(Ok(social), cx));
        let invited = Rc::new(Cell::new(false));
        cx.update(|_, cx| {
            let invited = invited.clone();
            cx.subscribe(&view, move |_, event, _| {
                if matches!(event, FriendsEvent::Invite) {
                    invited.set(true);
                }
            })
            .detach();
        });
        cx.run_until_parked();

        // The row to the invite sheet comes after thirty friends, far below the window.
        let above_the_tab_bar = point(px(200.), px(750.));
        cx.simulate_click(above_the_tab_bar, Modifiers::none());
        assert!(!invited.get(), "the row is not on screen yet");

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(220.), px(450.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-5000.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.simulate_click(above_the_tab_bar, Modifiers::none());
        assert!(invited.get(), "scrolled to the end, the row is there");
    }
}
