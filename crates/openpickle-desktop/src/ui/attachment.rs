//! An attachment of a message: its thumbnail as a preview, and the download of the file itself
//! into the Downloads folder.

use crate::{
    model::{Attachment, AttachmentKind},
    theme,
    ui::{Cancel, chrome, icon},
};
use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, ImageSource, MouseButton, ObjectFit,
    Window, div, img, prelude::*, px,
};
use std::path::PathBuf;

pub enum AttachmentEvent {
    Close,
    Retry,
    /// Download the file itself.
    Download,
}
enum State {
    Loading,
    Ready(ImageSource),
    Failed(String),
}

/// Where the download of the file itself stands.
enum Download {
    NotAsked,
    Running,
    Saved(PathBuf),
    Failed(String),
}

pub struct AttachmentView {
    attachment: Attachment,
    state: State,
    download: Download,
    focus: FocusHandle,
}

impl EventEmitter<AttachmentEvent> for AttachmentView {}
impl AttachmentView {
    pub fn new(attachment: Attachment, cx: &mut Context<Self>) -> Self {
        Self {
            attachment,
            state: State::Loading,
            download: Download::NotAsked,
            focus: cx.focus_handle(),
        }
    }

    pub fn path(&self) -> &str {
        &self.attachment.path
    }

    pub fn loaded(&mut self, result: Result<PathBuf, String>, cx: &mut Context<Self>) {
        self.state = match result {
            Ok(path) => State::Ready(ImageSource::from(path)),
            Err(reason) => State::Failed(reason),
        };
        cx.notify();
    }

    /// The download ended: the file it was saved as, or why there is none.
    pub fn downloaded(&mut self, result: Result<PathBuf, String>, cx: &mut Context<Self>) {
        self.download = match result {
            Ok(file) => Download::Saved(file),
            Err(reason) => Download::Failed(reason),
        };
        cx.notify();
    }

    fn start_download(&mut self, cx: &mut Context<Self>) {
        self.download = Download::Running;
        cx.emit(AttachmentEvent::Download);
        cx.notify();
    }

    /// The line about the file itself: the button that downloads it, then where it was saved.
    fn download_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = |id: &'static str, label: &'static str| {
            chrome(id)
                .flex_none()
                .rounded_full()
                .px(px(16.))
                .py(px(9.))
                .gap(px(8.))
                .cursor_pointer()
                .child(label)
        };
        let line = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .text_size(px(13.))
            .text_color(theme::color(theme::TEXT_MUTED));
        match &self.download {
            Download::NotAsked => line.child(
                button("download", "Download")
                    .flex_row_reverse()
                    .text_size(theme::BODY)
                    .text_color(theme::color(theme::TEXT))
                    .child(icon("download", 16., theme::TEXT))
                    .on_click(cx.listener(|this, _, _, cx| this.start_download(cx))),
            ),
            Download::Running => line.child("Downloading…"),
            Download::Saved(file) => {
                let name = |path: Option<&std::path::Path>| {
                    path.and_then(|path| path.file_name())
                        .map(|name| name.to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                let saved = format!("Saved as {} in {}", name(Some(file)), name(file.parent()));
                let file = file.clone();
                line.child(div().flex_1().min_w_0().child(saved)).child(
                    button("show-download", SHOW_FILE)
                        .text_color(theme::color(theme::TEXT))
                        .on_click(move |_, _, cx| cx.reveal_path(&file)),
                )
            }
            Download::Failed(reason) => line
                .child(div().flex_1().min_w_0().child(reason.clone()))
                .child(
                    button("retry-download", "Try again")
                        .text_color(theme::color(theme::TEXT))
                        .on_click(cx.listener(|this, _, _, cx| this.start_download(cx))),
                ),
        }
    }
}

/// What the system's file manager is asked to do with a saved file.
const SHOW_FILE: &str = if cfg!(target_os = "macos") {
    "Show in Finder"
} else {
    "Show in folder"
};

impl Render for AttachmentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file = self.attachment.kind == AttachmentKind::File;
        let muted = || {
            div()
                .text_size(px(13.))
                .text_color(theme::color(theme::TEXT_MUTED))
        };
        let content = match &self.state {
            State::Loading => muted().child("Loading preview…").into_any_element(),
            // The picture is a thumbnail: what it shows of the file is said under it.
            State::Ready(source) => div()
                .w_full()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.))
                .child(
                    img(source.clone())
                        .w_full()
                        .h((window.viewport_size().height - px(300.)).max(px(100.)))
                        .object_fit(ObjectFit::Contain),
                )
                .child(muted().child(if file {
                    "The first page, as a preview"
                } else {
                    "A smaller copy of the picture"
                }))
                .into_any_element(),
            // Not every kind of file has a preview. The file can be downloaded all the same.
            State::Failed(reason) => muted()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(12.))
                .child(format!("No preview: {reason}"))
                .child(
                    chrome("retry-preview")
                        .rounded_full()
                        .px(px(16.))
                        .py(px(10.))
                        .cursor_pointer()
                        .child("Try again")
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.state = State::Loading;
                            cx.emit(AttachmentEvent::Retry);
                            cx.notify();
                        })),
                )
                .into_any_element(),
        };
        let close = chrome("close-preview")
            .size(px(36.))
            .rounded_full()
            .cursor_pointer()
            .child(icon("x", 20., theme::TEXT))
            .on_click(cx.listener(|_, _, _, cx| cx.emit(AttachmentEvent::Close)));
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .track_focus(&self.focus)
            .on_action(cx.listener(|_, _: &Cancel, _, cx| cx.emit(AttachmentEvent::Close)))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(AttachmentEvent::Close)),
            )
            .bg(theme::alpha(0x000000, 0.75))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .text_color(theme::color(theme::TEXT))
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .id("attachment-preview")
                    .occlude()
                    .w_full()
                    .max_w(px(720.))
                    .max_h_full()
                    .overflow_y_scroll()
                    .p(px(20.))
                    .rounded(px(20.))
                    .bg(theme::color(theme::CARD))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(12.))
                            .child(div().flex_1().min_w_0().child(self.attachment.name.clone()))
                            .child(close),
                    )
                    .children(
                        (!self.attachment.detail.is_empty())
                            .then(|| muted().mt(px(-10.)).child(self.attachment.detail.clone())),
                    )
                    .child(self.download_line(cx))
                    .child(div().w_full().flex().justify_center().child(content)),
            )
    }
}
impl Focusable for AttachmentView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
