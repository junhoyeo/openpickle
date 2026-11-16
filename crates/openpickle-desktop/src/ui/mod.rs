pub mod chat;
pub mod input;
pub mod login;
pub mod rows;
pub mod search;
pub mod select;
pub mod wrap;

use gpui::{
    App, Bounds, Div, KeyBinding, Stateful, TitlebarOptions, WindowBounds, WindowOptions, actions,
    div, point, prelude::*, px, size, svg,
};

use crate::theme;

actions!(openpickle, [Find, Cancel, Previous, Next]);

/// Binds the keys of the text field and of the screens.
pub fn bind_keys(cx: &mut App) {
    input::bind_keys(cx);
    cx.bind_keys([
        KeyBinding::new("cmd-f", Find, None),
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("up", Previous, None),
        KeyBinding::new("down", Next, None),
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
