#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

use futures::StreamExt;
use gpui::{App, AppContext, Application, KeyBinding, Menu, MenuItem, actions};
use openpickle_desktop::{app::Shell, assets::Assets, backend, ui};

mod kakao_login;

actions!(openpickle, [Quit]);

fn main() {
    if std::env::args().nth(1).as_deref() == Some("--kakao-login") {
        if let Err(error) = kakao_login::run() {
            eprintln!("{error}");
            std::process::exit(1);
        }
        return;
    }
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
    let callback_commands = commands.clone();
    let (callbacks, mut callback_urls) = futures::channel::mpsc::unbounded();
    let app = Application::new().with_assets(Assets);
    app.on_open_urls(move |urls| {
        for url in urls {
            // The backend additionally requires an in-memory pending verifier.
            if url.starts_with(&format!("{}?", openpickle_api::kakao::KAKAO_REDIRECT_URI)) {
                callbacks.unbounded_send(url).ok();
            }
        }
    });
    app.run(move |cx: &mut App| {
        ui::bind_keys(cx);
        cx.bind_keys([KeyBinding::new("secondary-q", Quit, None)]);
        cx.on_action(|_: &Quit, cx| cx.quit());
        // Runs on every way out: the menu, the last window closing, the Dock, logging out.
        cx.on_app_quit(move |_| {
            #[cfg(target_os = "macos")]
            openpickle_desktop::asweb::cancel();
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
        let main_window = cx
            .open_window(ui::window_options(cx), |window, cx| {
                cx.new(|cx| Shell::new(commands, updates, window, cx))
            })
            .expect("the main window");
        cx.spawn(async move |cx| {
            while let Some(url) = callback_urls.next().await {
                // Return from the browser before exchanging the code, including on failure.
                cx.update(|cx| {
                    cx.activate(true);
                    main_window
                        .update(cx, |_, window, _| window.activate_window())
                        .ok();
                })
                .ok();
                callback_commands
                    .send(openpickle_desktop::protocol::Command::CompleteKakao {
                        redirect_or_code: url,
                    })
                    .ok();
            }
        })
        .detach();
        cx.activate(true);
    });
}
