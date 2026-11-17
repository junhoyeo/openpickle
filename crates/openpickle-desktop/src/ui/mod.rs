pub mod archive;
pub mod attachment;
pub mod chat;
pub mod drawer;
pub mod explore;
pub mod friends;
pub mod input;
pub mod invite;
pub mod login;
pub mod phone;
pub mod profile;
pub mod rows;
pub mod search;
pub mod select;
pub mod wrap;

use gpui::{
    App, Bounds, ClickEvent, Div, FontWeight, ImageSource, KeyBinding, ObjectFit, Pixels,
    SharedString, Stateful, TitlebarOptions, Window, WindowBounds, WindowOptions, actions, div,
    img, linear_color_stop, linear_gradient, point, prelude::*, px, size, svg,
};

use crate::theme;

actions!(
    openpickle,
    [
        Find,
        Cancel,
        Previous,
        Next,
        OpenChat,
        OpenExplore,
        OpenFriends,
        OpenArchive,
        OpenProfile,
        OpenPhoneCalls
    ]
);

/// Binds the keys of the text field and of the screens.
pub fn bind_keys(cx: &mut App) {
    input::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-c", input::Copy, Some("Chat")),
        KeyBinding::new("cmd-a", input::SelectAll, Some("Chat")),
        KeyBinding::new("cmd-f", Find, None),
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("up", Previous, None),
        KeyBinding::new("down", Next, None),
        KeyBinding::new("cmd-1", OpenChat, None),
        KeyBinding::new("cmd-2", OpenExplore, None),
        KeyBinding::new("cmd-3", OpenFriends, None),
        KeyBinding::new("cmd-4", OpenArchive, None),
        KeyBinding::new("cmd-5", OpenProfile, None),
        KeyBinding::new("cmd-shift-p", OpenPhoneCalls, None),
    ]);
}

/// A window the size of the design frame, with the traffic lights drawn over the content.
pub fn window_options(cx: &App) -> WindowOptions {
    WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
            None,
            size(px(440.), px(900.)),
            cx,
        ))),
        window_min_size: Some(size(px(380.), px(560.))),
        titlebar: Some(TitlebarOptions {
            title: Some("Pickle".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(16.), px(16.))),
        }),
        ..Default::default()
    }
}

/// A dark pill or circle with a hairline edge: the header buttons, the composer, the tab bar,
/// the search field. It takes the mouse, so a press on it is not a press on what is under it.
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

/// The sections of the app. Friends is reached through the menu; the other four are tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    Chat,
    Friends,
    Explore,
    Archive,
    Profile,
}

/// The pictures the backend has put on disk, by their key: friends' characters and offers'
/// icons. Views watch it and draw a picture once it is here.
#[derive(Default)]
pub struct Pictures(std::collections::HashMap<String, ImageSource>);

impl Pictures {
    pub fn add(&mut self, pictures: Vec<(String, std::path::PathBuf)>) {
        self.0.extend(
            pictures
                .into_iter()
                .map(|(key, path)| (key, ImageSource::from(path))),
        );
    }

    pub fn get(&self, key: &str) -> Option<ImageSource> {
        self.0.get(key).cloned()
    }
}

/// What the header of a section shows of the character.
#[derive(Clone)]
pub struct Identity {
    pub name: SharedString,
    pub avatar: Option<ImageSource>,
}

impl Default for Identity {
    fn default() -> Self {
        Self {
            name: "Pickle".into(),
            avatar: None,
        }
    }
}

/// A round picture of someone's character, or an empty disc until it is here.
fn face(picture: Option<ImageSource>, size: f32) -> impl IntoElement {
    let disc = div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .overflow_hidden()
        .bg(theme::alpha(0xffffff, 0.05))
        .border_1()
        .border_color(theme::alpha(0xffffff, 0.05));
    // The picture is rounded itself: clipping to the disc only cuts a rectangle.
    match picture {
        Some(source) => disc.child(
            img(source)
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover),
        ),
        None => disc,
    }
}

/// The header hides what is under it down to here. Further down it fades out and the screen
/// shows through.
const HEADER_SOLID: Pixels = px(49.);
const HEADER_FADE_END: Pixels = px(140.);

/// The header of a tab: the menu button, the character's picture and name, the Invite button.
fn tab_header(
    name: SharedString,
    avatar: Option<ImageSource>,
    on_menu: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    on_invite: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    show_invite: bool,
) -> impl IntoElement {
    let picture = div()
        .size(px(58.))
        .rounded_full()
        .overflow_hidden()
        .bg(theme::color(theme::TEXT));
    let picture = match avatar {
        Some(source) => picture.child(
            img(source)
                .size_full()
                .rounded_full()
                .object_fit(ObjectFit::Cover),
        ),
        None => picture,
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
                            .on_click(on_menu)
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
                    div().flex().flex_col().items_center().child(picture).child(
                        chrome("name")
                            .mt(px(-6.))
                            .h(px(33.))
                            .px(px(10.5))
                            .rounded_full()
                            .child(name),
                    ),
                )
                .child(div().w(px(74.)).when(show_invite, |slot| {
                    slot.child(
                        chrome("invite")
                            .w(px(74.))
                            .h(px(42.))
                            .rounded_full()
                            .cursor_pointer()
                            .on_click(on_invite)
                            .child("Invite"),
                    )
                })),
        )
}

/// The solid part of the header takes the mouse: what is hidden under it cannot be pressed.
fn header_cover() -> impl IntoElement {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(HEADER_SOLID)
        .occlude()
}

/// The pill with the four tabs.
fn tab_bar(
    selected: Tab,
    on_select: impl Fn(&Tab, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let on_select = std::rc::Rc::new(on_select);
    let tabs = [
        (Tab::Chat, "message-circle", "message-circle-filled"),
        (Tab::Explore, "compass", "compass"),
        (Tab::Archive, "archive", "archive"),
        (Tab::Profile, "user-round", "user-round"),
    ];
    chrome("tabs")
        .w_full()
        .max_w(px(399.))
        .h(px(62.))
        .rounded_full()
        .p(px(4.))
        .children(tabs.map(|(tab, name, chosen_name)| {
            let chosen = tab == selected;
            let on_select = on_select.clone();
            div()
                .id(name)
                .h_full()
                .flex_1()
                .when(chosen, |tab| {
                    tab.flex_grow()
                        .flex_shrink_0()
                        .min_w(px(107.))
                        .bg(theme::color(theme::CHROME_SELECTED))
                })
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .child(icon(
                    if chosen { chosen_name } else { name },
                    23.,
                    theme::TEXT,
                ))
                .when(!chosen, |element| {
                    element
                        .cursor_pointer()
                        .on_click(move |_, window, cx| on_select(&tab, window, cx))
                })
        }))
}
