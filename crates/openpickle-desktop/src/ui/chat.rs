//! The chat screen: header, message list, composer and tab bar.

use gpui::{
    App, ClipboardItem, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    ImageSource, ListAlignment, ListState, MouseButton, MouseDownEvent, MouseUpEvent, ObjectFit,
    Pixels, Point, SharedString, Stateful, Subscription, Window, anchored, deferred, div, img,
    linear_color_stop, linear_gradient, list, prelude::*, px, svg,
};

use crate::{
    model::Row,
    theme,
    ui::{
        input::{Copy, InputEvent, SelectAll, TextInput},
        rows::{RowContext, render_row},
        select::SharedSelection,
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
    /// The text selected in the conversation. Rows share it to draw and to change it.
    selection: SharedSelection,
    menu: Option<ContextMenu>,
    _subscriptions: Vec<Subscription>,
}

/// The menu a right click in the conversation opens.
struct ContextMenu {
    at: Point<Pixels>,
    /// The text of the message that was clicked, as it was then.
    message: Option<String>,
}

impl EventEmitter<ChatEvent> for ChatView {}

impl ChatView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Ask Pickle", cx));
        let subscriptions = vec![cx.subscribe(&input, |this, input, event, cx| match event {
            InputEvent::Submitted(_) => this.send(&input, cx),
            // The send button appears and disappears with the text.
            InputEvent::Changed => cx.notify(),
            // One selection at a time: Copy takes whichever is shown.
            InputEvent::Selected => {
                this.selection.borrow_mut().clear();
                cx.notify();
            }
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
            selection: SharedSelection::default(),
            menu: None,
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
        self.selection.borrow_mut().rows_changed(unchanged);
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

    /// The left button went down in the conversation.
    fn press(&mut self, _: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection.borrow_mut().press_anywhere() {
            cx.notify();
        }
        self.input.update(cx, |input, cx| input.deselect(cx));
    }

    fn release(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selection.borrow_mut().release();
    }

    /// The right button went down in the conversation.
    fn open_menu(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let row = self.selection.borrow_mut().take_context_row();
        let message = row
            .and_then(|row| self.rows.get(row))
            .map(|row| row.texts().join("\n"))
            .filter(|text| !text.is_empty());
        self.menu = Some(ContextMenu {
            at: event.position,
            message,
        });
        cx.notify();
    }

    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.selection.borrow().text(&self.rows) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn copy_message(&mut self, cx: &mut Context<Self>) {
        if let Some(text) = self.menu.as_ref().and_then(|menu| menu.message.clone()) {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn select_everything(&mut self, cx: &mut Context<Self>) {
        self.selection.borrow_mut().select_all(&self.rows);
        self.input.update(cx, |input, cx| input.deselect(cx));
        cx.notify();
    }

    /// While a selection is dragged past the top or bottom of the conversation, scrolls it
    /// that way, so a drag can reach text that is off screen.
    fn follow_drag(&mut self, window: &mut Window) {
        let pointer = window.mouse_position().y;
        let top = theme::HEADER_HEIGHT;
        let bottom = window.viewport_size().height - theme::BOTTOM_BAR_HEIGHT;
        let past = if pointer < top {
            pointer - top
        } else {
            (pointer - bottom).max(px(0.))
        };
        let scrolling = self.selection.borrow().is_dragging() && past != px(0.);
        self.selection.borrow_mut().set_scrolling(scrolling);
        if scrolling {
            // Deeper past the edge scrolls faster.
            self.list.scroll_by((past * 0.15).clamp(px(-40.), px(40.)));
            window.request_animation_frame();
        }
    }

    fn context_menu(&self, menu: &ContextMenu, cx: &mut Context<Self>) -> impl IntoElement {
        let item = |label: &'static str, action: fn(&mut Self, &mut Context<Self>)| {
            div()
                .id(label)
                .h(px(30.))
                .px(px(11.))
                .flex()
                .items_center()
                .rounded(px(7.))
                .hover(|item| item.bg(theme::color(theme::CHROME_SELECTED)))
                .child(label)
                .on_click(cx.listener(move |this, _, _, cx| {
                    action(this, cx);
                    this.menu = None;
                    cx.notify();
                }))
        };
        let has_selection = !self.selection.borrow().is_empty();
        let items = div()
            .id("context-menu")
            .occlude()
            .min_w(px(176.))
            .p(px(5.))
            .rounded(px(12.))
            .bg(theme::color(theme::CHROME))
            .border_1()
            .border_color(theme::color(theme::HAIRLINE))
            .shadow_lg()
            .text_size(px(14.))
            .line_height(px(18.))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                this.menu = None;
                cx.notify();
            }))
            .when(has_selection, |menu| {
                menu.child(item("Copy", Self::copy_selection))
            })
            .when(menu.message.is_some(), |menu| {
                menu.child(item("Copy Message", Self::copy_message))
            })
            .when(!self.rows.is_empty(), |menu| {
                menu.child(item("Select All", Self::select_everything))
            });
        deferred(
            anchored()
                .position(menu.at)
                .snap_to_window_with_margin(px(8.))
                .child(items),
        )
        .with_priority(1)
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

        // The bar takes the mouse, so a press on it is not a press in the conversation under it.
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .occlude()
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
        .occlude()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_drag(window);
        let messages = list(
            self.list.clone(),
            cx.processor(|this, index: usize, window: &mut Window, _| {
                render_row(&RowContext::new(
                    window,
                    &this.rows[index],
                    index,
                    &this.selection,
                    open_link,
                ))
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
            .on_action(cx.listener(|this, _: &Copy, _, cx| this.copy_selection(cx)))
            .on_action(cx.listener(|this, _: &SelectAll, _, cx| this.select_everything(cx)))
            .child(
                div()
                    .size_full()
                    .flex()
                    .justify_center()
                    .on_mouse_down(MouseButton::Left, cx.listener(Self::press))
                    .on_mouse_down(MouseButton::Right, cx.listener(Self::open_menu))
                    .on_mouse_up(MouseButton::Left, cx.listener(Self::release))
                    .on_mouse_up_out(MouseButton::Left, cx.listener(Self::release))
                    .child(
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
            .children(self.menu.as_ref().map(|menu| self.context_menu(menu, cx)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{Modifiers, TestAppContext, VisualTestContext, point, size};

    use super::*;
    use crate::{
        content::RichText,
        model::{Part, RowKind, Side},
        ui::input,
    };

    const WIDTH: Pixels = px(440.);
    const HEIGHT: Pixels = px(900.);

    /// A chat window with one bubble per text and the cursor in the composer.
    fn open<'a>(
        cx: &'a mut TestAppContext,
        texts: &[String],
    ) -> (Entity<ChatView>, &'a mut VisualTestContext) {
        cx.update(input::bind_keys);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = ChatView::new(cx);
            window.focus(&view.focus_handle(cx));
            view
        });
        cx.simulate_resize(size(WIDTH, HEIGHT));
        let rows = texts
            .iter()
            .map(|text| Row {
                kind: RowKind::Part(Part::Bubble(Arc::new(RichText {
                    text: text.clone(),
                    spans: vec![],
                }))),
                side: Side::Theirs,
                joined_above: false,
                joined_below: false,
            })
            .collect();
        view.update(cx, |view, cx| view.set_rows(rows, cx));
        cx.run_until_parked();
        (view, cx)
    }

    fn copied(cx: &VisualTestContext) -> Option<String> {
        cx.read_from_clipboard().and_then(|item| item.text())
    }

    #[gpui::test]
    fn a_drag_over_the_conversation_is_copied_with_the_keyboard(cx: &mut TestAppContext) {
        let texts = [
            "첫 번째 말".to_owned(),
            "가운데".to_owned(),
            "마지막 말".to_owned(),
        ];
        let (_, cx) = open(cx, &texts);

        // From the top left corner of the conversation to its bottom right one.
        let from = point(px(4.), theme::HEADER_HEIGHT + px(1.));
        let to = point(WIDTH - px(4.), HEIGHT - theme::BOTTOM_BAR_HEIGHT - px(1.));
        cx.simulate_mouse_down(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(from, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(copied(cx).as_deref(), Some("첫 번째 말\n가운데\n마지막 말"));

        // Moving the pointer after the release changes nothing.
        cx.simulate_mouse_move(from, None, Modifiers::none());
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(copied(cx).as_deref(), Some("첫 번째 말\n가운데\n마지막 말"));

        // A press on empty space drops the selection, so there is nothing to copy.
        cx.write_to_clipboard(ClipboardItem::new_string("before".into()));
        cx.simulate_click(from, Modifiers::none());
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(copied(cx).as_deref(), Some("before"));
    }

    #[gpui::test]
    fn select_all_copies_rows_that_were_never_drawn(cx: &mut TestAppContext) {
        let texts: Vec<String> = (0..300).map(|n| format!("말 {n}")).collect();
        let (_, cx) = open(cx, &texts);

        // The composer is empty, so Select All means the conversation.
        cx.simulate_keystrokes("cmd-a cmd-c");
        assert_eq!(copied(cx), Some(texts.join("\n")));

        // With text in the composer it means that text, and the conversation lets go of its own.
        cx.simulate_input("hello");
        cx.simulate_keystrokes("cmd-a cmd-c");
        assert_eq!(copied(cx).as_deref(), Some("hello"));
    }

    #[gpui::test]
    fn a_right_click_offers_the_message_under_it_and_select_all(cx: &mut TestAppContext) {
        let texts: Vec<String> = (0..60).map(|n| format!("말 {n}")).collect();
        let (_, cx) = open(cx, &texts);

        // Rows fill the window, so its middle is on one of them. The first item is Copy Message.
        let at = point(px(60.), HEIGHT / 2.);
        let first_item = at + point(px(20.), px(20.));
        cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
        cx.simulate_click(first_item, Modifiers::none());
        let message = copied(cx).expect("Copy Message copies");
        assert!(texts.contains(&message), "copied {message:?}");

        // The second item is Select All, after which Copy comes first.
        cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
        cx.simulate_click(first_item + point(px(0.), px(30.)), Modifiers::none());
        cx.simulate_mouse_down(at, MouseButton::Right, Modifiers::none());
        cx.simulate_mouse_up(at, MouseButton::Right, Modifiers::none());
        cx.simulate_click(first_item, Modifiers::none());
        assert_eq!(copied(cx), Some(texts.join("\n")));
    }
}
