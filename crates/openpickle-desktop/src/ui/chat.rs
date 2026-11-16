//! The chat screen: header, message list, composer and tab bar.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    ImageSource, ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent, MouseUpEvent,
    ObjectFit, Pixels, Point, SharedString, Subscription, Task, Window, anchored, deferred, div,
    img, linear_color_stop, linear_gradient, list, point, prelude::*, px, relative,
};

use crate::{
    model::Row,
    theme,
    ui::{
        Cancel, Find, chrome, icon,
        input::{Copy, InputEvent, SelectAll, TextInput},
        rows::{RowContext, render_row},
        select::SharedSelection,
    },
};

/// The header hides the conversation down to here. Further down it fades out and the
/// conversation shows through.
const HEADER_SOLID: Pixels = px(49.);
const HEADER_FADE_END: Pixels = px(140.);

/// How long the message a search result opened stays marked.
const FOUND_MARK: Duration = Duration::from_millis(1600);
/// How much of the conversation before that message shows above it.
const FOUND_CONTEXT: Pixels = px(110.);

/// How fast a drag past an edge scrolls: pixels per second for each pixel past the edge.
const DRAG_SCROLL_RATE: f32 = 14.;
const DRAG_SCROLL_MAX: Pixels = px(2400.);

pub enum ChatEvent {
    /// The user sent this text.
    Send(String),
    /// The user asked for the search screen.
    Search,
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
    /// When a drag past an edge last scrolled the list.
    scrolled_at: Option<Instant>,
    /// Whether the menu on the left is open.
    drawer: bool,
    /// The rows of the message a search result opened, marked for a moment.
    found: Option<Range<usize>>,
    _found: Option<Task<()>>,
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
            scrolled_at: None,
            drawer: false,
            found: None,
            _found: None,
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

    /// Scrolls to a message, given the rows it is drawn as, and marks it for a moment. The
    /// list stays where it is put, so the conversation around the message can be read by
    /// scrolling up and down from it.
    pub fn reveal(&mut self, rows: Range<usize>, window: &mut Window, cx: &mut Context<Self>) {
        self.list.scroll_to(ListOffset {
            item_ix: rows.start,
            offset_in_item: px(0.),
        });
        self.at_latest = false;
        // The message would sit right under the header. The end of what came before it is
        // brought into view as well, which takes two frames: the list only knows the heights
        // of the rows above once it has drawn at the new place.
        cx.on_next_frame(window, |_, window, cx| {
            cx.on_next_frame(window, |this, _, cx| {
                this.scroll_by(-FOUND_CONTEXT);
                cx.notify();
            });
            cx.notify();
        });
        self.found = Some(rows);
        self._found = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(FOUND_MARK).await;
            this.update(cx, |this, cx| {
                this.found = None;
                cx.notify();
            })
            .ok();
        }));
        cx.notify();
    }

    /// Escape closes what is open on top of the conversation, or else drops the selection.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_none() && !std::mem::take(&mut self.drawer) {
            self.selection.borrow_mut().clear();
        }
        cx.notify();
    }

    /// The menu on the left: the search button and the sections of the app. Only Chat exists.
    fn drawer_panel(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let section = |name: &'static str, label: &'static str, color: u32| {
            div()
                .h(px(50.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(14.))
                .rounded(px(17.))
                .text_color(theme::color(color))
                .child(icon(name, 23., color))
                .child(label)
        };
        let panel = div()
            .id("drawer")
            .occlude()
            .h_full()
            .w(px(300.))
            .max_w(relative(0.84))
            .bg(theme::color(theme::CHROME))
            .border_r_1()
            .border_color(theme::color(theme::HAIRLINE))
            .pt(px(50.))
            .px(px(14.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .text_size(px(17.))
            .line_height(theme::LABEL_LINE)
            .font_weight(FontWeight::MEDIUM)
            .child(
                div()
                    .pl(px(12.))
                    .pb(px(16.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_size(px(22.))
                            .line_height(px(28.))
                            .font_weight(FontWeight::BOLD)
                            .child("Pickle"),
                    )
                    .child(
                        chrome("search")
                            .size(px(43.))
                            .rounded_full()
                            .cursor_pointer()
                            .child(icon("search", 20., theme::TEXT))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.drawer = false;
                                cx.emit(ChatEvent::Search);
                                cx.notify();
                            })),
                    ),
            )
            .child(
                section("message-circle", "Chat", theme::TEXT)
                    .id("section-chat")
                    .bg(theme::color(theme::CHROME_SELECTED))
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.drawer = false;
                        cx.notify();
                    })),
            )
            .child(section("users-round", "Friends", theme::TEXT))
            .child(section("compass", "Explore", theme::TEXT))
            .child(section("archive", "Archive", theme::TEXT))
            .child(section("globe", "3D World", theme::PLACEHOLDER))
            .child(section("user-round", "My Pickle", theme::TEXT));
        // A press beside the panel closes it.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(theme::alpha(0x000000, 0.5))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.drawer = false;
                    cx.notify();
                }),
            )
            .child(panel)
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

    /// While a selection is dragged past the top or bottom of what the conversation shows,
    /// scrolls it that way, so a drag can reach text that is off screen.
    fn follow_drag(&mut self, window: &mut Window) {
        let pointer = window.mouse_position().y;
        let bottom = window.viewport_size().height - theme::BOTTOM_BAR_HEIGHT;
        let past = if pointer < HEADER_SOLID {
            pointer - HEADER_SOLID
        } else {
            (pointer - bottom).max(px(0.))
        };
        let scrolling = {
            let mut selection = self.selection.borrow_mut();
            if !window.is_window_active() {
                // The release may have gone to whatever took the window's place.
                selection.release();
            }
            let scrolling = selection.is_extending() && past != px(0.);
            selection.set_scrolling(scrolling);
            scrolling
        };
        if !scrolling {
            self.scrolled_at = None;
            return;
        }
        let now = Instant::now();
        let elapsed = self
            .scrolled_at
            .replace(now)
            .map_or(Duration::from_millis(16), |at| now - at)
            .min(Duration::from_millis(50));
        // Deeper past the edge scrolls faster.
        let speed = (past * DRAG_SCROLL_RATE).clamp(-DRAG_SCROLL_MAX, DRAG_SCROLL_MAX);
        self.scroll_by(speed * elapsed.as_secs_f32());
        window.request_animation_frame();
    }

    /// Scrolls the list by a distance in pixels, down when it is positive.
    fn scroll_by(&mut self, distance: Pixels) {
        let (above, below) = list_padding();
        // While the list follows the newest row it reports a place past its end, so the
        // place is brought back to the end before the distance is added.
        let end = self.list.max_offset_for_scrollbar().height + above + below;
        let top = (-self.list.scroll_px_offset_for_scrollbar().y).min(end);
        self.list
            .set_offset_from_scrollbar(point(px(0.), (top + distance).max(px(0.))));
        self.at_latest = self.list.logical_scroll_top().item_ix >= self.rows.len();
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

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
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
            .h(HEADER_FADE_END)
            .bg(linear_gradient(
                180.,
                linear_color_stop(theme::color(theme::PAGE), HEADER_SOLID / HEADER_FADE_END),
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
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.drawer = true;
                                    cx.notify();
                                }))
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

/// The room the list leaves above its first row and below its last, for the header and the
/// bottom bar.
fn list_padding() -> (Pixels, Pixels) {
    (theme::HEADER_HEIGHT, theme::BOTTOM_BAR_HEIGHT + px(16.))
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
                let mut row =
                    RowContext::new(window, &this.rows[index], index, &this.selection, open_link);
                row.marked = this
                    .found
                    .as_ref()
                    .is_some_and(|found| found.contains(&index));
                render_row(&row)
            }),
        )
        .size_full()
        .pt(list_padding().0)
        .pb(list_padding().1);

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
            .on_action(cx.listener(|_, _: &Find, _, cx| cx.emit(ChatEvent::Search)))
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.cancel(cx)))
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
            // The solid part of the header takes the mouse: text hidden under it cannot be
            // pressed, and a press there is not a press in the conversation.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(HEADER_SOLID)
                    .occlude(),
            )
            .child(self.header(cx))
            .when(!self.at_latest, |screen| screen.child(to_latest))
            .child(self.bottom_bar(cx))
            .when(self.drawer, |screen| screen.child(self.drawer_panel(cx)))
            .children(self.menu.as_ref().map(|menu| self.context_menu(menu, cx)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use gpui::{
        Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext,
        size,
    };

    use super::*;
    use crate::{
        content::RichText,
        model::{Part, RowKind, Side},
    };

    const WIDTH: Pixels = px(440.);
    const HEIGHT: Pixels = px(900.);

    /// A chat window in front, with one bubble per text and the cursor in the composer.
    fn open<'a>(
        cx: &'a mut TestAppContext,
        texts: &[String],
    ) -> (Entity<ChatView>, &'a mut VisualTestContext) {
        cx.update(crate::ui::bind_keys);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = ChatView::new(cx);
            window.focus(&view.focus_handle(cx));
            window.activate_window();
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

        // A press on the header is not a press in the conversation, so a drag from it
        // selects nothing.
        let header = point(px(200.), HEADER_SOLID - px(10.));
        cx.simulate_mouse_down(header, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_move(to, MouseButton::Left, Modifiers::none());
        cx.simulate_mouse_up(to, MouseButton::Left, Modifiers::none());
        cx.simulate_keystrokes("cmd-c");
        assert_eq!(copied(cx).as_deref(), Some("before"));
    }

    #[gpui::test]
    fn the_conversation_scrolls_both_ways_from_a_message_that_was_opened(cx: &mut TestAppContext) {
        let texts: Vec<String> = (0..300).map(|n| format!("말 {n}")).collect();
        let (view, cx) = open(cx, &texts);
        let top_row = |cx: &mut VisualTestContext| {
            view.read_with(cx, |view, _| view.list.logical_scroll_top().item_ix)
        };
        let wheel = |cx: &mut VisualTestContext, up: f32| {
            cx.simulate_event(ScrollWheelEvent {
                position: point(WIDTH / 2., HEIGHT / 2.),
                delta: ScrollDelta::Pixels(point(px(0.), px(up))),
                modifiers: Modifiers::none(),
                touch_phase: TouchPhase::Moved,
            });
        };

        // A search result opens a message far from the newest one.
        view.update_in(cx, |view, window, cx| view.reveal(100..101, window, cx));
        cx.run_until_parked();
        assert_eq!(top_row(cx), 100);

        // What came before it and what came after are one wheel turn away.
        wheel(cx, 400.);
        let before = top_row(cx);
        assert!(before < 100, "scrolled up to row {before}");
        wheel(cx, -1200.);
        let after = top_row(cx);
        assert!(after > 100, "scrolled down to row {after}");
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
