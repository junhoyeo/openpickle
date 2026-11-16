use gpui::{App, AppContext, Application, KeyBinding, Menu, MenuItem, actions};
use openpickle_desktop::{app::Shell, assets::Assets, backend, ui};

actions!(openpickle, [Quit]);

fn main() {
    // Started before GPUI so the cached conversation loads while the window is being created.
    let backend::Backend {
        commands,
        updates,
        stop,
    } = match backend::start() {
        Ok(backend) => backend,
        Err(backend::StartError::AlreadyRunning) => {
            eprintln!("OpenPickle is already running.");
            return;
        }
        Err(backend::StartError::Store(error)) => {
            eprintln!("OpenPickle cannot use its data folder: {error}");
            std::process::exit(1);
        }
    };
    Application::new()
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            ui::bind_keys(cx);
            cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
            cx.on_action(|_: &Quit, cx| cx.quit());
            // Runs on every way out: the menu, the last window closing, the Dock, logging out.
            cx.on_app_quit(move |_| {
                stop.stop();
                async {}
            })
            .detach();
            cx.set_menus(vec![Menu {
                name: "OpenPickle".into(),
                items: vec![MenuItem::action("Quit OpenPickle", Quit)],
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
