//! The chat screen: header, message list, composer and tab bar.

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, ImageSource,
    ListAlignment, ListState, MouseButton, ObjectFit, SharedString, Stateful, Subscription, Window,
    div, img, linear_color_stop, linear_gradient, list, prelude::*, px, svg,
};

use crate::{
    model::Row,
    theme,
    ui::{
        input::{InputEvent, TextInput},
        rows::{RowContext, render_row},
    },
};

pub enum ChatEvent {
    /// The user sent this text.
    Send(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Tab {
    Chat,
    Explore,
    Library,
    Profile,
}

pub struct ChatView {
    name: SharedString,
    avatar: Option<ImageSource>,
    rows: Vec<Row>,
    list: ListState,
    input: Entity<TextInput>,
    /// False once the user scrolled up; the list then stops following new messages.
    at_latest: bool,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ChatEvent> for ChatView {}

impl ChatView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Ask Pickle", cx));
        let subscriptions = vec![cx.subscribe(&input, |this, input, event, cx| match event {
            InputEvent::Submitted(_) => this.send(&input, cx),
            // The send button appears and disappears with the text.
            InputEvent::Changed => cx.notify(),
        })];
        let list = ListState::new(0, ListAlignment::Bottom, px(600.));
        let view = cx.entity().downgrade();
        list.set_scroll_handler(move |event, _, cx| {
            let scrolled = event.is_scrolled;
            view.update(cx, |this, cx| {
                if this.at_latest == scrolled {
                    this.at_latest = !scrolled;
                    cx.notify();
                }
            })
            .ok();
        });
        Self {
            name: "Pickle".into(),
            avatar: None,
            rows: Vec::new(),
            list,
            input,
            at_latest: true,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_identity(
        &mut self,
        name: impl Into<SharedString>,
        avatar: Option<ImageSource>,
        cx: &mut Context<Self>,
    ) {
        self.name = name.into();
        self.avatar = avatar;
        cx.notify();
    }

    /// Replaces the rows. Rows before the first changed one keep their measured heights.
    pub fn set_rows(&mut self, rows: Vec<Row>, cx: &mut Context<Self>) {
        let unchanged = self
            .rows
            .iter()
            .zip(&rows)
            .take_while(|(old, new)| old.same_as(new))
            .count();
        if unchanged < self.rows.len() || unchanged < rows.len() {
            self.list
                .splice(unchanged..self.rows.len(), rows.len() - unchanged);
        }
        self.rows = rows;
        cx.notify();
    }

    fn send(&mut self, input: &Entity<TextInput>, cx: &mut Context<Self>) {
        let text = input.update(cx, |input, cx| input.take(cx));
        if !text.trim().is_empty() {
            self.scroll_to_latest(cx);
            cx.emit(ChatEvent::Send(text.trim().to_owned()));
        }
    }

    fn scroll_to_latest(&mut self, cx: &mut Context<Self>) {
        // A reset is the list's way back to "follow the newest row".
        self.list.reset(self.rows.len());
        self.at_latest = true;
        cx.notify();
    }

    fn header(&self) -> impl IntoElement {
        let avatar = div()
            .size(px(58.))
            .rounded_full()
            .overflow_hidden()
            .bg(theme::color(theme::TEXT));
        let avatar = match &self.avatar {
            Some(source) => {
                avatar.child(img(source.clone()).size_full().object_fit(ObjectFit::Cover))
            }
            None => avatar,
        };
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(140.))
            .bg(linear_gradient(
                180.,
                linear_color_stop(theme::color(theme::PAGE), 0.35),
                linear_color_stop(theme::alpha(theme::PAGE, 0.), 1.),
            ))
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(theme::COLUMN_MAX)
                    .px(theme::GUTTER)
                    .pt(px(44.))
                    .flex()
                    .items_start()
                    .justify_between()
                    .text_size(theme::LABEL)
                    .line_height(theme::LABEL_LINE)
                    .font_weight(FontWeight::MEDIUM)
                    .child(
                        div().w(px(74.)).child(
                            chrome("menu")
                                .size(px(43.))
                                .rounded_full()
                                .flex_col()
                                .gap(px(3.8))
                                .children((0..2).map(|_| {
                                    div()
                                        .w(px(15.5))
                                        .h(px(2.2))
                                        .rounded_full()
                                        .bg(theme::color(theme::TEXT))
                                })),
                        ),
                    )
                    .child(
                        div().flex().flex_col().items_center().child(avatar).child(
                            chrome("name")
                                .mt(px(-6.))
                                .h(px(33.))
                                .px(px(10.5))
                                .rounded_full()
                                .child(self.name.clone()),
                        ),
                    )
                    .child(
                        chrome("invite")
                            .w(px(74.))
                            .h(px(42.))
                            .rounded_full()
                            .child("Invite"),
                    ),
            )
    }

    fn bottom_bar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let has_text = !self.input.read(cx).is_empty();
        let action = div()
            .id("send")
            .size(px(36.))
            .flex_none()
            .rounded_full()
            .bg(theme::color(theme::TEXT))
            .flex()
            .items_center()
            .justify_center();
        let action = if has_text {
            action
                .cursor_pointer()
                .child(icon("arrow-up", 19., theme::CHROME))
                .on_click(cx.listener(|this, _, _, cx| {
                    let input = this.input.clone();
                    this.send(&input, cx);
                }))
        } else {
            action.child(icon("phone", 17., theme::CHROME))
        };
        let composer = chrome("composer")
            .w_full()
            .h(px(53.))
            .rounded_full()
            .pl(px(15.))
            .pr(px(9.))
            .gap(px(10.))
            .justify_start()
            .child(icon("plus", 24., theme::TEXT))
            .child(self.input.clone())
            .child(icon("mic", 24., theme::TEXT))
            .child(div().w(px(4.)))
            .child(action);

        let tabs = [
            (Tab::Chat, "message-circle-filled"),
            (Tab::Explore, "compass"),
            (Tab::Library, "archive"),
            (Tab::Profile, "user-round"),
        ];
        let tab_bar = chrome("tabs")
            .w_full()
            .max_w(px(399.))
            .h(px(62.))
            .rounded_full()
            .p(px(4.))
            .children(tabs.map(|(tab, name)| {
                let selected = tab == Tab::Chat;
                div()
                    .h_full()
                    .flex_1()
                    .when(selected, |tab| {
                        tab.flex_grow()
                            .flex_shrink_0()
                            .min_w(px(107.))
                            .bg(theme::color(theme::CHROME_SELECTED))
                    })
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(icon(name, 23., theme::TEXT))
            }));

        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(theme::COLUMN_MAX)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .pb(px(21.))
                    .child(div().w_full().px(px(24.)).child(composer))
                    .child(
                        div()
                            .w_full()
                            .px(px(20.))
                            .flex()
                            .justify_center()
                            .child(tab_bar),
                    ),
            )
    }
}

/// A dark pill or circle with a hairline edge: the header buttons, the composer, the tab bar.
fn chrome(id: &'static str) -> Stateful<Div> {
    div()
        .id(id)
        .flex()
        .items_center()
        .justify_center()
        .bg(theme::alpha(theme::CHROME, 0.94))
        .border_1()
        .border_color(theme::color(theme::HAIRLINE))
}

fn icon(name: &'static str, size: f32, color: u32) -> impl IntoElement {
    svg()
        .path(format!("icons/{name}.svg"))
        .size(px(size))
        .flex_none()
        .text_color(theme::color(color))
}

fn open_link(url: &str, _: &mut Window, cx: &mut App) {
    if url.starts_with("https://") || url.starts_with("http://") {
        cx.open_url(url);
    }
}

impl Focusable for ChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.input.focus_handle(cx)
    }
}

impl Render for ChatView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let messages = list(
            self.list.clone(),
            cx.processor(|this, index: usize, window: &mut Window, _| {
                let content_width =
                    window.viewport_size().width.min(theme::COLUMN_MAX) - theme::GUTTER * 2.;
                let context = RowContext {
                    window,
                    content_width,
                    on_link: open_link,
                };
                render_row(&this.rows[index], index, &context)
            }),
        )
        .size_full()
        .pt(theme::HEADER_HEIGHT)
        .pb(theme::BOTTOM_BAR_HEIGHT + px(16.));

        let to_latest = div()
            .absolute()
            .bottom(theme::BOTTOM_BAR_HEIGHT + px(9.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(theme::COLUMN_MAX)
                    .px(px(12.))
                    .flex()
                    .justify_end()
                    .child(
                        chrome("to-latest")
                            .size(px(34.))
                            .rounded_full()
                            .cursor_pointer()
                            .child(icon("chevron-down", 21., theme::TEXT))
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| this.scroll_to_latest(cx)),
                            ),
                    ),
            );

        div()
            .size_full()
            .relative()
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .child(
                div().size_full().flex().justify_center().child(
                    div()
                        .h_full()
                        .w_full()
                        .max_w(theme::COLUMN_MAX)
                        .child(messages),
                ),
            )
            .child(self.header())
            .when(!self.at_latest, |screen| screen.child(to_latest))
            .child(self.bottom_bar(cx))
    }
}
