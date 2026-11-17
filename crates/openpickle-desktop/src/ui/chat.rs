//! The chat screen: header, message list, composer and tab bar.

use std::{
    ops::Range,
    time::{Duration, Instant},
};

use gpui::{
    App, ClipboardItem, Context, Entity, EventEmitter, FocusHandle, Focusable, ImageSource,
    ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent, MouseUpEvent, Pixels, Point,
    SharedString, Subscription, Task, Window, anchored, deferred, div, list, point, prelude::*, px,
};

use openpickle_api::PhoneCallTranscript;

use crate::{
    model::{AttachmentKind, Part, Row, RowKind},
    theme,
    ui::{
        Cancel, Find, HEADER_SOLID, Pictures, Tab, chrome, header_cover, icon,
        input::{Copy, InputEvent, SelectAll, TextInput},
        rows::{PhoneCards, RowContext, render_row},
        select::SharedSelection,
        tab_bar, tab_header,
    },
};

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
    /// The header's menu button was pressed.
    Menu,
    /// The header's Invite button was pressed.
    Invite,
    /// Another tab was chosen in the tab bar.
    Open(Tab),
    /// Browse calls placed by Pickle. This is not a microphone or dial action.
    PhoneCalls,
    /// A markdown link or a room/file card was activated.
    Link(String),
    /// Thumbnails for attachments newly shown in the conversation.
    LoadAttachments(Vec<String>),
    /// The call behind a phone card that was drawn for the first time, by its handoff path.
    LoadPhoneCard(String),
}

pub struct ChatView {
    name: SharedString,
    avatar: Option<ImageSource>,
    /// The pictures on disk, which the messages' files and room links draw from.
    pictures: Entity<Pictures>,
    rows: Vec<Row>,
    list: ListState,
    input: Entity<TextInput>,
    can_send: bool,
    room_focus: FocusHandle,
    requested_pictures: std::collections::HashSet<String>,
    /// The calls the phone cards show, by the handoff path of their task.
    phone_cards: PhoneCards,
    requested_phone_cards: std::collections::HashSet<String>,
    /// Redraws the cards of live calls each second, for the time they have lasted.
    call_clock: Option<Task<()>>,
    /// False once the user scrolled up; the list then stops following new messages.
    at_latest: bool,
    /// The text selected in the conversation. Rows share it to draw and to change it.
    selection: SharedSelection,
    menu: Option<ContextMenu>,
    /// When a drag past an edge last scrolled the list.
    scrolled_at: Option<Instant>,
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
    pub fn new(pictures: Entity<Pictures>, cx: &mut Context<Self>) -> Self {
        let input = cx.new(|cx| TextInput::new("Ask Pickle", cx));
        let mut subscriptions = vec![cx.subscribe(&input, |this, input, event, cx| match event {
            InputEvent::Submitted(_) => this.send(&input, cx),
            // The send button appears and disappears with the text.
            InputEvent::Changed => cx.notify(),
            // One selection at a time: Copy takes whichever is shown.
            InputEvent::Selected => {
                this.selection.borrow_mut().clear();
                cx.notify();
            }
        })];
        subscriptions.push(cx.observe(&pictures, |_, _, cx| cx.notify()));
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
            pictures,
            rows: Vec::new(),
            list,
            input,
            can_send: true,
            room_focus: cx.focus_handle(),
            requested_pictures: std::collections::HashSet::new(),
            phone_cards: PhoneCards::new(),
            requested_phone_cards: std::collections::HashSet::new(),
            call_clock: None,
            at_latest: true,
            selection: SharedSelection::default(),
            menu: None,
            scrolled_at: None,
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

    pub fn set_can_send(&mut self, can_send: bool, cx: &mut Context<Self>) {
        self.can_send = can_send;
        cx.notify();
    }

    pub fn draft(&self, cx: &App) -> String {
        self.input.read(cx).text().to_owned()
    }

    /// A room switch must not carry selections, scroll positions or a different room's draft.
    pub fn switch_room(&mut self, can_send: bool, draft: &str, cx: &mut Context<Self>) {
        self.can_send = can_send;
        self.input.update(cx, |input, cx| input.set_text(draft, cx));
        self.selection.borrow_mut().clear();
        self.selection.borrow_mut().release();
        self.menu = None;
        self.found = None;
        self._found = None;
        self.requested_pictures.clear();
        // The cards are asked for again when they are next drawn.
        self.requested_phone_cards.clear();
        self.list.reset(0);
        self.rows.clear();
        self.at_latest = true;
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
        let mut missing = Vec::new();
        for row in &rows {
            if let RowKind::Part(Part::Files(files)) = &row.kind {
                for file in &files.items {
                    let previewable = file.kind == AttachmentKind::Picture
                        || file.path.to_ascii_lowercase().ends_with(".pdf");
                    if previewable && self.requested_pictures.insert(file.path.clone()) {
                        missing.push(file.path.clone());
                    }
                }
            }
        }
        self.rows = rows;
        if !missing.is_empty() {
            cx.emit(ChatEvent::LoadAttachments(missing));
        }
        cx.notify();
    }

    /// Shows the call of a phone task in its card. The card grows with it, so its rows are
    /// measured again.
    pub fn set_phone_card(
        &mut self,
        path: String,
        call: PhoneCallTranscript,
        cx: &mut Context<Self>,
    ) {
        for (index, row) in self.rows.iter().enumerate() {
            if matches!(&row.kind, RowKind::Part(Part::PhoneTask { path: shown, .. }) if *shown == path)
            {
                self.list.splice(index..index + 1, 1);
            }
        }
        self.phone_cards.insert(path, call);
        if self.call_clock.is_none() && self.phone_cards.values().any(|call| call.live) {
            self.call_clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let live = this.update(cx, |this, cx| {
                        let live = this.phone_cards.values().any(|call| call.live);
                        if live {
                            cx.notify();
                        } else {
                            this.call_clock = None;
                        }
                        live
                    });
                    if !matches!(live, Ok(true)) {
                        break;
                    }
                }
            }));
        }
        cx.notify();
    }

    /// Takes a newer reading of a call for the cards that already show it.
    pub fn refresh_phone_card(&mut self, call: &PhoneCallTranscript, cx: &mut Context<Self>) {
        let paths: Vec<String> = self
            .phone_cards
            .iter()
            .filter(|(_, shown)| shown.call_id == call.call_id)
            .map(|(path, _)| path.clone())
            .collect();
        for path in paths {
            self.set_phone_card(path, call.clone(), cx);
        }
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

    /// Escape closes the menu of a right click, or else drops the selection.
    fn cancel(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_none() {
            self.selection.borrow_mut().clear();
        }
        cx.notify();
    }

    fn send(&mut self, input: &Entity<TextInput>, cx: &mut Context<Self>) {
        if !self.can_send {
            return;
        }
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

    pub fn scroll_to_latest(&mut self, cx: &mut Context<Self>) {
        // A reset is the list's way back to "follow the newest row".
        self.list.reset(self.rows.len());
        self.at_latest = true;
        cx.notify();
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
            action
                .cursor_pointer()
                .child(icon("phone", 17., theme::CHROME))
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ChatEvent::PhoneCalls)))
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

        let composer = if self.can_send {
            composer.into_any_element()
        } else {
            div()
                .h(px(53.))
                .w_full()
                .rounded_full()
                .bg(theme::color(theme::CHROME))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(13.))
                .text_color(theme::color(theme::TEXT_MUTED))
                .child("Read-only conversation")
                .into_any_element()
        };
        let tab_bar = tab_bar(
            Tab::Chat,
            cx.listener(|_, tab: &Tab, _, cx| cx.emit(ChatEvent::Open(*tab))),
        );

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

impl Focusable for ChatView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.can_send {
            self.input.focus_handle(cx)
        } else {
            self.room_focus.clone()
        }
    }
}

impl Render for ChatView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.follow_drag(window);
        let owner = cx.entity().downgrade();
        let on_link: super::select::LinkHandler = std::rc::Rc::new(move |target, _, cx| {
            owner
                .update(cx, |_, cx| cx.emit(ChatEvent::Link(target.to_owned())))
                .ok();
        });
        let messages = list(
            self.list.clone(),
            cx.processor(move |this, index: usize, window: &mut Window, cx| {
                // A phone card asks for its call when it is first drawn.
                if let RowKind::Part(Part::PhoneTask { path, .. }) = &this.rows[index].kind
                    && this.requested_phone_cards.insert(path.clone())
                {
                    cx.emit(ChatEvent::LoadPhoneCard(path.clone()));
                }
                let mut row = RowContext::new(
                    window,
                    &this.rows[index],
                    index,
                    &this.selection,
                    on_link.clone(),
                    this.pictures.read(cx),
                );
                row.marked = this
                    .found
                    .as_ref()
                    .is_some_and(|found| found.contains(&index));
                row.phone_cards = Some(&this.phone_cards);
                row.avatar = this.avatar.clone();
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
            .track_focus(&self.room_focus)
            .key_context("Chat")
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
            .child(header_cover())
            .child(tab_header(
                self.name.clone(),
                self.avatar.clone(),
                cx.listener(|_, _, _, cx| cx.emit(ChatEvent::Menu)),
                cx.listener(|_, _, _, cx| cx.emit(ChatEvent::Invite)),
                true,
            ))
            .when(!self.at_latest, |screen| screen.child(to_latest))
            .child(self.bottom_bar(cx))
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
            let pictures = cx.new(|_| Pictures::default());
            let view = ChatView::new(pictures, cx);
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

    #[gpui::test]
    fn file_and_room_cards_dispatch_links_and_still_copy_their_labels(cx: &mut TestAppContext) {
        use crate::model::{Attachment, Files};
        use std::{cell::RefCell, rc::Rc};
        let (view, cx) = open(cx, &[]);
        let links = Rc::new(RefCell::new(Vec::<String>::new()));
        cx.update(|_, cx| {
            let links = links.clone();
            cx.subscribe(&view, move |_, event, _| {
                if let ChatEvent::Link(target) = event {
                    links.borrow_mut().push(target.clone());
                }
            })
            .detach();
        });
        let file = Attachment {
            path: "attachments/2026-01-01/report.pdf".into(),
            seq: 1,
            at: 1.,
            name: "Sample report.pdf".into(),
            detail: "PDF · 4 pages".into(),
            kind: AttachmentKind::File,
        };
        let row = |part| Row {
            kind: RowKind::Part(part),
            side: Side::Theirs,
            joined_above: false,
            joined_below: false,
        };
        view.update(cx, |view, cx| {
            view.set_rows(
                vec![row(Part::Files(Arc::new(Files { items: vec![file] })))],
                cx,
            )
        });
        // The only row is bottom-aligned immediately above the composer's reserved space.
        let at = point(px(60.), HEIGHT - theme::BOTTOM_BAR_HEIGHT - px(35.));
        cx.simulate_click(at, Modifiers::none());
        assert_eq!(
            *links.borrow(),
            ["attachment:attachments/2026-01-01/report.pdf"]
        );
        cx.simulate_keystrokes("cmd-a cmd-c");
        assert_eq!(
            copied(cx).as_deref(),
            Some("Sample report.pdf\nPDF · 4 pages")
        );

        view.update(cx, |view, cx| {
            view.set_rows(
                vec![row(Part::RoomLink {
                    room_id: "11111111-1111-4111-8111-111111111111".into(),
                    label: "Sample room".into(),
                    picture: String::new(),
                })],
                cx,
            )
        });
        cx.simulate_click(at, Modifiers::none());
        assert!(
            links
                .borrow()
                .last()
                .unwrap()
                .starts_with("pickle://chat?room_id=")
        );
        cx.simulate_keystrokes("cmd-a cmd-c");
        assert_eq!(copied(cx).as_deref(), Some("Sample room"));
    }

    #[gpui::test]
    fn a_phone_card_asks_for_its_call_once_and_opens_the_task(cx: &mut TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        let (view, cx) = open(cx, &[]);
        let events = Rc::new(RefCell::new(Vec::<String>::new()));
        cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event, _| match event {
                ChatEvent::LoadPhoneCard(path) => events.borrow_mut().push(format!("load {path}")),
                ChatEvent::Link(target) => events.borrow_mut().push(format!("open {target}")),
                _ => {}
            })
            .detach();
        });
        let rows = || {
            vec![Row {
                kind: RowKind::Part(Part::PhoneTask {
                    title: "📞 Sample call".into(),
                    path: "handoffs/sample-task".into(),
                }),
                side: Side::Theirs,
                joined_above: false,
                joined_below: false,
            }]
        };
        view.update(cx, |view, cx| view.set_rows(rows(), cx));
        cx.run_until_parked();
        assert_eq!(*events.borrow(), ["load handoffs/sample-task"]);

        // The call arrives, a newer reading of it follows, and the card is drawn again.
        let call = |live: bool| -> PhoneCallTranscript {
            serde_json::from_value(serde_json::json!({
                "kind": "phone_call", "call_id": "sample-call", "title": "Sample call",
                "subtitle_lead": "with ", "counterpart_label": "someone", "subtitle_tail": "",
                "started_label": "Today", "live": live, "hangup_requested": false,
                "footer": "", "duration_s": 5,
                "lines": [{
                    "id": "1", "line_id": "1", "speaker": "them", "text": "Hello",
                    "joins_previous": false, "joins_next": false,
                }],
            }))
            .unwrap()
        };
        view.update(cx, |view, cx| {
            view.set_phone_card("handoffs/sample-task".into(), call(true), cx);
            view.refresh_phone_card(&call(false), cx);
            assert!(!view.phone_cards["handoffs/sample-task"].live);
            // A reading of another call changes no card.
            let mut other = call(true);
            other.call_id = "another-call".into();
            view.refresh_phone_card(&other, cx);
            assert!(!view.phone_cards["handoffs/sample-task"].live);
        });
        cx.run_until_parked();
        assert_eq!(events.borrow().len(), 1, "a drawn card is asked for once");

        // The only row is bottom-aligned immediately above the composer's reserved space.
        let at = point(px(60.), HEIGHT - theme::BOTTOM_BAR_HEIGHT - px(60.));
        cx.simulate_click(at, Modifiers::none());
        assert_eq!(
            events.borrow().last().map(String::as_str),
            Some("open handoff:handoffs/sample-task")
        );
    }

    #[gpui::test]
    fn an_observer_composer_does_not_send_or_consume_its_draft(cx: &mut TestAppContext) {
        use std::{cell::Cell, rc::Rc};
        let (view, cx) = open(cx, &[]);
        let sent = Rc::new(Cell::new(false));
        cx.update(|_, cx| {
            let sent = sent.clone();
            cx.subscribe(&view, move |_, event, _| {
                if matches!(event, ChatEvent::Send(_)) {
                    sent.set(true);
                }
            })
            .detach();
        });
        view.update(cx, |view, cx| {
            view.switch_room(false, "draft", cx);
            let input = view.input.clone();
            view.send(&input, cx);
        });
        assert!(!sent.get());
        view.read_with(cx, |view, cx| assert_eq!(view.draft(cx), "draft"));
    }
}
