//! The invite sheet: your invite link, how many invites are left, and who joined with it.

use std::time::Duration;

use gpui::{
    App, ClipboardItem, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    MouseButton, SharedString, Subscription, Task, Window, div, prelude::*, px, relative,
};

use crate::{
    model::{Invite, Person},
    theme,
    ui::{Cancel, Pictures, face, icon},
};

pub enum InviteEvent {
    Close,
}

pub struct InviteView {
    invite: Invite,
    pictures: Entity<Pictures>,
    /// Whether the list of people who joined is open.
    joined_open: bool,
    /// What the pill at the bottom says, for a moment.
    notice: Option<SharedString>,
    _notice: Option<Task<()>>,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<InviteEvent> for InviteView {}

impl InviteView {
    /// How long the pill at the bottom stays.
    const NOTICE: Duration = Duration::from_millis(2500);

    pub fn new(invite: Invite, pictures: Entity<Pictures>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&pictures, |_, _, cx| cx.notify())];
        Self {
            invite,
            pictures,
            joined_open: false,
            notice: None,
            _notice: None,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_invite(&mut self, invite: Invite, cx: &mut Context<Self>) {
        self.invite = invite;
        cx.notify();
    }

    fn copy_link(&mut self, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.invite.url.clone()));
        self.notice = Some("Link copied".into());
        self._notice = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Self::NOTICE).await;
            this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            })
            .ok();
        }));
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
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(theme::LABEL)
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(person.name.clone()),
                    )
                    .when(!person.note.is_empty(), |text| {
                        text.child(
                            div()
                                .text_size(px(13.))
                                .line_height(px(18.))
                                .text_color(theme::color(theme::TEXT_FAINT))
                                .child(person.note.clone()),
                        )
                    }),
            )
    }
}

impl Render for InviteView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let invite = &self.invite;
        let has_link = !invite.url.is_empty();
        let left = match invite.left {
            1 => "1 invite left".to_owned(),
            left => format!("{left} invites left"),
        };
        let link = div()
            .p(px(20.))
            .rounded(px(24.))
            .bg(theme::color(theme::CARD))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(5.))
                    .child(
                        div()
                            .text_size(theme::LABEL)
                            .font_weight(FontWeight::MEDIUM)
                            .child("Pickle invite link"),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(18.))
                            .text_color(theme::color(theme::TEXT_MUTED))
                            .truncate()
                            .child(invite.url.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(14.))
                            .line_height(px(18.))
                            .text_color(theme::color(theme::LINK))
                            .child(left),
                    ),
            );
        let copy = div()
            .id("copy-link")
            .w(px(58.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(6.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.copy_link(cx)))
            .child(
                div()
                    .size(px(46.))
                    .rounded_full()
                    .bg(theme::color(theme::TEXT))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon("link", 20., theme::PAGE)),
            )
            .child(
                div()
                    .text_size(px(10.))
                    .line_height(px(12.5))
                    .text_color(theme::alpha(theme::TEXT, 0.5))
                    .child("Copy link"),
            );
        let joined_header = div()
            .id("joined")
            .py(px(4.))
            .flex()
            .items_center()
            .gap(px(4.))
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| {
                this.joined_open = !this.joined_open;
                cx.notify();
            }))
            .child(div().flex_1().child("People who joined with my invite"))
            .child(
                div()
                    .text_color(theme::color(theme::TEXT_FAINT))
                    .child(invite.joined.len().to_string()),
            )
            .child(icon(
                if self.joined_open {
                    "chevron-down"
                } else {
                    "chevron-right"
                },
                17.,
                theme::TEXT,
            ));
        let joined: Vec<Div> = invite
            .joined
            .iter()
            .map(|person| self.person(person, cx))
            .collect();
        let inviters: Vec<Div> = invite
            .inviters
            .iter()
            .map(|person| self.person(person, cx))
            .collect();
        let nobody = div()
            .py(px(10.))
            .text_center()
            .text_color(theme::color(theme::TEXT_MUTED))
            .child("No one has joined with your invite yet.");
        let card = div()
            .p(px(16.))
            .rounded(px(24.))
            .bg(theme::alpha(0xffffff, 0.05))
            .flex()
            .flex_col()
            .gap(px(14.))
            .when(has_link, |card| {
                card.child(link).child(div().flex().child(copy))
            })
            .when(!has_link, |card| {
                card.child(
                    div()
                        .py(px(8.))
                        .text_center()
                        .text_color(theme::color(theme::TEXT_MUTED))
                        .child("Pickle has not given you an invite link yet."),
                )
            })
            .child(joined_header)
            .when(self.joined_open, |card| match joined.is_empty() {
                true => card.child(nobody),
                false => card.child(div().flex().flex_col().gap(px(4.)).children(joined)),
            });
        let width = window.viewport_size().width.min(px(520.));
        let panel = div()
            .id("invite-sheet")
            .occlude()
            .w(width)
            .max_h(relative(0.86))
            .overflow_y_scroll()
            .rounded_t(px(28.))
            .bg(theme::color(theme::SHEET))
            .px(px(20.))
            .pt(px(26.))
            .pb(px(32.))
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .pb(px(14.))
                    .text_center()
                    .text_size(px(18.))
                    .line_height(px(26.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("Invite friends to Pickle"),
            )
            .child(card)
            .when(!inviters.is_empty(), |panel| {
                panel.child(
                    div()
                        .p(px(20.))
                        .rounded(px(24.))
                        .bg(theme::color(theme::CARD))
                        .flex()
                        .flex_col()
                        .gap(px(8.))
                        .child(
                            div()
                                .text_size(theme::LABEL)
                                .font_weight(FontWeight::MEDIUM)
                                .child("People who invited me"),
                        )
                        .children(inviters),
                )
            });
        let pill = self.notice.clone().map(|notice| {
            div()
                .absolute()
                .bottom(px(18.))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .px(px(16.))
                        .py(px(10.))
                        .rounded_full()
                        .bg(theme::alpha(0x363a43, 0.96))
                        .text_size(px(14.))
                        .text_color(theme::alpha(0xffffff, 0.7))
                        .child(notice),
                )
        });
        // A press beside the sheet closes it.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(InviteEvent::Close)))
            .bg(theme::alpha(0x000000, 0.5))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(InviteEvent::Close)),
            )
            .child(panel)
            .children(pill)
    }
}

impl Focusable for InviteView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
