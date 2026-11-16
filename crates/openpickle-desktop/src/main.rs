use gpui::{App, AppContext, Application, KeyBinding, Menu, MenuItem, actions};
use openpickle_desktop::{app::Shell, assets::Assets, backend, ui};

actions!(openpickle, [Quit]);

fn main() {
    // Started before GPUI so the cached conversation loads while the window is being created.
    let (commands, updates) = backend::start();
    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ui::input::bind_keys(cx);
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            cx.set_menus(vec![Menu {
                name: "Pickle".into(),
                items: vec![MenuItem::action("Quit Pickle", Quit)],
            }]);
            cx.on_window_closed(|cx| {
                if cx.windows().is_empty() {
                    cx.quit();
                }
            })
            .detach();
            cx.open_window(ui::window_options(cx), |window, cx| {
                cx.new(|cx| Shell::new(commands, updates, window, cx))
            })
            .expect("the main window");
            cx.activate(true);
        });
}
