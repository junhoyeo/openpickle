//! The menu on the left: the search button, the sections of the app and Friends.

use gpui::{
    App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, MouseButton,
    SharedString, Subscription, Window, div, prelude::*, px, relative,
};

use crate::{
    model::RoomInfo,
    theme,
    ui::{Cancel, Pictures, Tab, chrome, face, icon},
};

#[derive(Clone)]
pub enum DrawerEvent {
    Search,
    Friends,
    Open(Tab),
    /// Show this other chat room.
    Room(String),
    Close,
}

pub struct DrawerView {
    /// The tab the window is on, which the menu marks.
    selected: Tab,
    /// The other chat rooms, newest first.
    rooms: Vec<RoomInfo>,
    pictures: Entity<Pictures>,
    _pictures: Subscription,
    /// Holds the keyboard while the menu is open, so that Escape closes it.
    focus: FocusHandle,
}

impl EventEmitter<DrawerEvent> for DrawerView {}

impl DrawerView {
    pub fn new(selected: Tab, pictures: Entity<Pictures>, cx: &mut Context<Self>) -> Self {
        let changed = cx.observe(&pictures, |_, _, cx| cx.notify());
        Self {
            selected,
            rooms: Vec::new(),
            pictures,
            _pictures: changed,
            focus: cx.focus_handle(),
        }
    }

    pub fn set_rooms(&mut self, rooms: Vec<RoomInfo>, cx: &mut Context<Self>) {
        self.rooms = rooms;
        cx.notify();
    }
}

impl Focusable for DrawerView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DrawerView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A row of the menu. One that leads somewhere says where.
        let row = |name: &'static str, label: &'static str, leads_to: Option<DrawerEvent>| {
            let marked = match &leads_to {
                Some(DrawerEvent::Open(tab)) => *tab == self.selected,
                _ => false,
            };
            let color = if leads_to.is_some() {
                theme::TEXT
            } else {
                theme::PLACEHOLDER
            };
            let row = div()
                .id(name)
                .h(px(50.))
                .flex_none()
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(14.))
                .rounded(px(17.))
                .text_color(theme::color(color))
                .when(marked, |row| row.bg(theme::color(theme::CHROME_SELECTED)))
                .child(icon(name, 23., color))
                .child(label);
            match leads_to {
                Some(event) => row
                    .cursor_pointer()
                    .on_click(cx.listener(move |_, _, _, cx| {
                        cx.emit(event.clone());
                    })),
                None => row,
            }
        };
        let panel = div()
            .id("drawer")
            .occlude()
            .h_full()
            .overflow_y_scroll()
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
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DrawerEvent::Search))),
                    ),
            )
            .child(row(
                "message-circle",
                "Chat",
                Some(DrawerEvent::Open(Tab::Chat)),
            ))
            .child(row("users-round", "Friends", Some(DrawerEvent::Friends)))
            .child(row(
                "compass",
                "Explore",
                Some(DrawerEvent::Open(Tab::Explore)),
            ))
            .child(row(
                "archive",
                "Archive",
                Some(DrawerEvent::Open(Tab::Archive)),
            ))
            .child(row("globe", "3D World", None))
            .child(row("user-round", "My Pickle", None))
            .when(!self.rooms.is_empty(), |menu| {
                menu.child(
                    div()
                        .pl(px(12.))
                        .pt(px(18.))
                        .pb(px(4.))
                        .text_size(px(13.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(theme::color(theme::TEXT_FAINT))
                        .child("Other chats"),
                )
                .children(self.rooms.iter().map(|room| {
                    let room_id = room.room_id.clone();
                    div()
                        .id(SharedString::from(room_id.clone()))
                        .h(px(50.))
                        .flex_none()
                        .px(px(14.))
                        .flex()
                        .items_center()
                        .gap(px(14.))
                        .rounded(px(17.))
                        .cursor_pointer()
                        .hover(|row| row.bg(theme::alpha(0xffffff, 0.05)))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let _ = this;
                            cx.emit(DrawerEvent::Room(room_id.clone()));
                        }))
                        .child(face(self.pictures.read(cx).get(&room.picture), 32.))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .child(room.title.clone()),
                        )
                }))
            });
        // A press beside the panel closes it.
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .bg(theme::alpha(0x000000, 0.5))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(DrawerEvent::Close)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(DrawerEvent::Close)),
            )
            .child(panel)
    }
}
