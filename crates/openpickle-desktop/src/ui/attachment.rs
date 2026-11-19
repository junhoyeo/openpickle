//! Preview, save, and view an attachment from Chat or Archive.

use crate::{
    model::{Attachment, AttachmentKind},
    theme,
    ui::{Cancel, chrome, icon},
};
use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, ImageSource, MouseButton,
    ObjectFit, RetainAllImageCache, Stateful, Window, div, img, prelude::*, px,
};
use std::path::{Path, PathBuf};

pub enum AttachmentEvent {
    Close,
    Preview,
    /// Download the file itself.
    Download,
    CancelDownload,
}
enum State {
    NotAsked,
    Loading,
    Ready(ImageSource),
    Failed(String),
}

/// Where the download of the file itself stands.
enum Download {
    NotAsked,
    Running { received: u64, total: Option<u64> },
    Canceling,
    Saved(PathBuf),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ImageView {
    Preview,
    OriginalFit,
    OriginalActualSize,
}

pub struct AttachmentView {
    attachment: Attachment,
    state: State,
    download: Download,
    image_view: ImageView,
    original_cache: Entity<RetainAllImageCache>,
    focus: FocusHandle,
}

impl EventEmitter<AttachmentEvent> for AttachmentView {}
impl AttachmentView {
    pub fn new(attachment: Attachment, cx: &mut Context<Self>) -> Self {
        Self {
            attachment,
            state: State::NotAsked,
            download: Download::NotAsked,
            image_view: ImageView::Preview,
            original_cache: RetainAllImageCache::new(cx),
            focus: cx.focus_handle(),
        }
    }

    pub fn path(&self) -> &str {
        &self.attachment.path
    }

    pub fn start_preview(&mut self, cx: &mut Context<Self>) {
        self.state = State::Loading;
        cx.emit(AttachmentEvent::Preview);
        cx.notify();
    }

    pub fn loaded(&mut self, result: Result<PathBuf, String>, cx: &mut Context<Self>) {
        self.state = match result {
            Ok(path) => State::Ready(ImageSource::from(path)),
            Err(reason) => State::Failed(reason),
        };
        cx.notify();
    }

    pub fn download_progress(&mut self, received: u64, total: Option<u64>, cx: &mut Context<Self>) {
        if !matches!(self.download, Download::Canceling) {
            self.download = Download::Running { received, total };
            cx.notify();
        }
    }

    /// The download ended: the file it was saved as, or why there is none.
    pub fn downloaded(&mut self, result: Result<PathBuf, String>, cx: &mut Context<Self>) {
        self.download = match result {
            Ok(file) => Download::Saved(file),
            Err(reason) => Download::Failed(reason),
        };
        cx.notify();
    }

    pub fn start_download(&mut self, cx: &mut Context<Self>) {
        self.download = Download::Running {
            received: 0,
            total: None,
        };
        cx.emit(AttachmentEvent::Download);
        cx.notify();
    }

    fn saved_image(&self) -> Option<&Path> {
        let Download::Saved(file) = &self.download else {
            return None;
        };
        (self.attachment.kind == AttachmentKind::Picture
            && file.is_absolute()
            && can_view_image(file))
        .then_some(file.as_path())
    }

    fn show_image(&mut self, mode: ImageView, cx: &mut Context<Self>) {
        self.image_view = mode;
        cx.notify();
    }

    /// The line about the file itself: the button that downloads it, then where it was saved.
    fn download_line(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let button = attachment_button;
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
            Download::Running { received, total } => {
                let status = match (received, total) {
                    (_, Some(0)) => "Downloading…".to_owned(),
                    (received, Some(total)) => {
                        format!("Downloading… {}%", (received * 100 / total).min(100))
                    }
                    (0, None) => "Waiting for download…".to_owned(),
                    (received, None) => {
                        format!("Downloading… {:.1} MiB", *received as f64 / (1024. * 1024.))
                    }
                };
                line.child(status).child(
                    button("cancel-download", "Cancel")
                        .text_color(theme::color(theme::TEXT))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.download = Download::Canceling;
                            cx.emit(AttachmentEvent::CancelDownload);
                            cx.notify();
                        })),
                )
            }
            Download::Canceling => line.child("Canceling download…"),
            Download::Saved(file) => {
                let name = file.file_name().unwrap_or_default().to_string_lossy();
                let folder = file
                    .parent()
                    .and_then(Path::file_name)
                    .unwrap_or_default()
                    .to_string_lossy();
                let saved = format!("Saved as {name} in {folder}");
                let mut actions = div().flex().flex_wrap().gap(px(6.));
                if self.saved_image().is_some() {
                    actions = match self.image_view {
                        ImageView::Preview => actions.child(
                            button("view-original", "View original").on_click(cx.listener(
                                |this, _, _, cx| this.show_image(ImageView::OriginalFit, cx),
                            )),
                        ),
                        ImageView::OriginalFit | ImageView::OriginalActualSize => {
                            let (label, mode) = if self.image_view == ImageView::OriginalFit {
                                ("Actual size", ImageView::OriginalActualSize)
                            } else {
                                ("Fit to view", ImageView::OriginalFit)
                            };
                            actions
                                .child(button("resize-original", label).on_click(
                                    cx.listener(move |this, _, _, cx| this.show_image(mode, cx)),
                                ))
                                .child(button("show-preview", "Show preview").on_click(
                                    cx.listener(|this, _, _, cx| {
                                        this.show_image(ImageView::Preview, cx)
                                    }),
                                ))
                        }
                    };
                } else if self.attachment.kind == AttachmentKind::File && can_open_document(file) {
                    let file = file.clone();
                    actions = actions.child(
                        button("open-original", "Open original")
                            .on_click(move |_, _, cx| cx.open_with_system(&file)),
                    );
                }
                let file = file.clone();
                line.flex_col()
                    .items_start()
                    .gap(px(6.))
                    .child(div().w_full().truncate().child(saved))
                    .child(
                        actions.child(
                            button("show-download", SHOW_FILE)
                                .on_click(move |_, _, cx| cx.reveal_path(&file)),
                        ),
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

fn attachment_button(id: &'static str, label: &'static str) -> Stateful<Div> {
    chrome(id)
        .debug_selector(move || id.into())
        .flex_none()
        .rounded_full()
        .px(px(16.))
        .py(px(9.))
        .gap(px(8.))
        .text_color(theme::color(theme::TEXT))
        .cursor_pointer()
        .child(label)
}

fn saved_extension(file: &Path) -> Option<String> {
    file.extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
}

fn can_view_image(file: &Path) -> bool {
    matches!(
        saved_extension(file).as_deref(),
        Some("jpg" | "jpeg" | "png" | "webp" | "gif")
    )
}

/// Only hand documents to the OS on an explicit click, using the saved filename, never the
/// attachment's remote path or metadata. Unknown types, scripts, and shortcuts are reveal-only.
fn can_open_document(file: &Path) -> bool {
    file.is_absolute() && matches!(saved_extension(file).as_deref(), Some("pdf" | "txt" | "md"))
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
        let preview_height = (window.viewport_size().height - px(300.)).max(px(100.));
        let original = self
            .saved_image()
            .filter(|_| self.image_view != ImageView::Preview);
        let content = if let Some(file) = original {
            let fit = self.image_view == ImageView::OriginalFit;
            let image = img(file)
                .image_cache(&self.original_cache)
                .id("original-image")
                .debug_selector(|| "attachment-original-image".into())
                .flex_none()
                .with_loading(|| div().child("Loading original…").into_any_element())
                .with_fallback(|| {
                    div()
                        .child("This image could not be displayed.")
                        .into_any_element()
                })
                .when(fit, |image| {
                    image
                        .w_full()
                        .h(preview_height)
                        .object_fit(ObjectFit::Contain)
                });
            div()
                .id("original-image-scroll")
                .w_full()
                .h(preview_height)
                .when(!fit, |area| area.overflow_scroll())
                .child(image)
                .into_any_element()
        } else {
            match &self.state {
                State::NotAsked => attachment_button("load-preview", "Load preview")
                    .on_click(cx.listener(|this, _, _, cx| this.start_preview(cx)))
                    .into_any_element(),
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
                            .id("attachment-thumbnail")
                            .w_full()
                            .h(preview_height)
                            .object_fit(ObjectFit::Contain)
                            .with_loading(|| div().child("Loading preview…").into_any_element())
                            .with_fallback(|| {
                                div()
                                    .child("This preview could not be displayed. Download the file to see the original.")
                                    .into_any_element()
                            }),
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
                        attachment_button("retry-preview", "Try again")
                            .on_click(cx.listener(|this, _, _, cx| this.start_preview(cx))),
                    )
                    .into_any_element(),
            }
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
                    .max_w(px(if original.is_some() { 960. } else { 720. }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_allowed_saved_documents_can_launch_the_system_opener() {
        let root = std::env::temp_dir();
        for name in ["report.PDF", "README.md", "note (1).txt"] {
            assert!(can_open_document(&root.join(name)), "{name}");
        }
        for name in [
            "report.pdf.exe",
            "run.command",
            "run.sh",
            "unknown",
            "web.html",
            "animation.svg",
            "link.url",
            "launch.desktop",
        ] {
            assert!(!can_open_document(&root.join(name)), "{name}");
        }
        assert!(!can_open_document(Path::new("report.pdf")));
    }

    #[gpui::test]
    fn a_saved_photo_can_switch_from_thumbnail_to_original_and_actual_size(
        cx: &mut gpui::TestAppContext,
    ) {
        // A 2 × 3 raster, independent of account data or the thumbnail cache.
        let png = [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x03, 0x08, 0x02, 0x00, 0x00,
            0x00, 0x36, 0x88, 0x49, 0xd6, 0x00, 0x00, 0x00, 0x10, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0x00, 0x44, 0x0c, 0x28, 0x14, 0x00, 0x44, 0xd0, 0x05,
            0xfb, 0xa4, 0xcf, 0xde, 0x80, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae,
            0x42, 0x60, 0x82,
        ];
        let path =
            std::env::temp_dir().join(format!("openpickle-test-{}.png", uuid::Uuid::new_v4()));
        std::fs::write(&path, png).unwrap();
        let (view, cx) = cx.add_window_view(|_, cx| {
            AttachmentView::new(
                Attachment {
                    path: "attachments/2026-01-01/photo.png".into(),
                    seq: 1,
                    at: 1.,
                    name: "Photo.png".into(),
                    detail: String::new(),
                    kind: AttachmentKind::Picture,
                },
                cx,
            )
        });
        let click = |cx: &mut gpui::VisualTestContext, selector| {
            cx.update(|window, cx| window.draw(cx).clear());
            let bounds = cx.debug_bounds(selector).expect(selector);
            cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        };
        cx.update(|window, cx| window.draw(cx).clear());
        assert!(cx.debug_bounds("view-original").is_none());

        view.update(cx, |view, cx| {
            view.loaded(Err("No thumbnail for the test".into()), cx);
            view.downloaded(Ok(path.clone()), cx);
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.saved_image(), Some(path.as_path()))
        });
        click(cx, "view-original");
        view.read_with(cx, |view, _| {
            assert!(view.image_view == ImageView::OriginalFit)
        });
        click(cx, "resize-original");
        view.read_with(cx, |view, _| {
            assert!(view.image_view == ImageView::OriginalActualSize)
        });
        cx.update(|window, cx| window.draw(cx).clear());
        assert_eq!(
            cx.debug_bounds("attachment-original-image").unwrap().size,
            gpui::size(px(2.), px(3.)),
        );
        click(cx, "show-preview");
        view.read_with(cx, |view, _| assert!(view.image_view == ImageView::Preview));
        std::fs::remove_file(path).unwrap();
    }
}
