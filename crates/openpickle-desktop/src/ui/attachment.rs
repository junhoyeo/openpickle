//! A thumbnail preview of an attachment. The capture provides only this endpoint, not a
//! download URL for the original PDF or file.

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
}
enum State {
    Loading,
    Ready(ImageSource),
    Failed(String),
}

pub struct AttachmentView {
    attachment: Attachment,
    state: State,
    focus: FocusHandle,
}

impl EventEmitter<AttachmentEvent> for AttachmentView {}
impl AttachmentView {
    pub fn new(attachment: Attachment, cx: &mut Context<Self>) -> Self {
        Self {
            attachment,
            state: State::Loading,
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
}

impl Render for AttachmentView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let file = self.attachment.kind == AttachmentKind::File;
        let content = match &self.state {
            State::Loading => div().child("Loading preview…").into_any_element(),
            State::Ready(source) => img(source.clone())
                .w_full()
                .h((window.viewport_size().height - px(260.)).max(px(100.)))
                .object_fit(ObjectFit::Contain)
                .into_any_element(),
            State::Failed(reason) => div()
                .flex()
                .flex_col()
                .items_center()
                .gap(px(16.))
                .child(reason.clone())
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
                    .child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme::color(theme::TEXT_MUTED))
                            .child(if file {
                                "First-page preview · not the original document"
                            } else {
                                "Image preview · thumbnail resolution"
                            }),
                    )
                    .child(div().w_full().flex().justify_center().child(content))
                    .when(file, |panel| {
                        panel.child(
                            div()
                                .text_size(px(13.))
                                .text_color(theme::color(theme::TEXT_MUTED))
                                .child(
                                    "Opening or saving the original document is not available yet.",
                                ),
                        )
                    }),
            )
    }
}
impl Focusable for AttachmentView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}
