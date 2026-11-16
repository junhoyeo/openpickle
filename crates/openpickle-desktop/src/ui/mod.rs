pub mod chat;
pub mod input;
pub mod login;
pub mod rows;
pub mod select;
pub mod wrap;

use gpui::{App, Bounds, TitlebarOptions, WindowBounds, WindowOptions, point, px, size};

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
