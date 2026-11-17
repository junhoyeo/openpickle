//! Phone-call history and read-only transcripts.
//!
//! This view never polls and never starts a call. Its owner loads the selected call and decides
//! while to refresh it. Closing or going back only stops watching; only the confirmation dialog
//! can emit [`PhoneEvent::End`].

use gpui::{
    App, ClipboardItem, Context, Div, EventEmitter, FocusHandle, Focusable, FontWeight,
    HighlightStyle, ImageSource, ListAlignment, ListOffset, ListState, Pixels, SharedString,
    Stateful, StyledText, Window, div, linear_color_stop, linear_gradient, list, prelude::*, px,
};
use openpickle_api::{PhoneCallLine, PhoneCallTranscript};
use uuid::Uuid;

use crate::{
    phone::CallSummary,
    theme,
    ui::{Cancel, Identity, chrome, icon, input, own_face, wrap::keep_hangul},
};

/// The top of the header's row of controls. The phone app puts it 6 pt under its status bar;
/// here that is the height of the window's title bar.
const HEADER_TOP: f32 = 44.;
/// The header ends 18 pt under its 44 pt row.
const HEADER_HEIGHT: f32 = HEADER_TOP + 44. + 18.;
/// The footer's text stands this far above the window's edge.
const FOOTER_BOTTOM: f32 = 24.;
/// What the End call button adds to the footer: its 42 pt and the 12 pt above it.
const END_BUTTON_ROOM: f32 = 54.;

pub enum PhoneEvent {
    Close,
    /// Open the call and begin owner-managed refreshing if it is live.
    Open(String),
    /// Return to the list and stop owner-managed refreshing.
    Back,
    Reload,
    Refresh(String),
    /// End a call after explicit confirmation. Retries reuse the same action id.
    End {
        call_id: String,
        action_id: String,
    },
}

#[derive(Clone, Debug)]
enum EndState {
    Idle,
    Confirm {
        action_id: String,
    },
    Pending {
        action_id: String,
    },
    Sent,
    Failed {
        action_id: String,
        reason: SharedString,
    },
}

pub struct PhoneView {
    identity: Identity,
    calls: Vec<CallSummary>,
    calls_loaded: bool,
    calls_failed: Option<SharedString>,
    selected: Option<String>,
    call: Option<PhoneCallTranscript>,
    call_failed: Option<SharedString>,
    lines: ListState,
    /// False once the reader scrolls up; new lines then do not pull the list to the bottom.
    at_latest: bool,
    end: EndState,
    focus: FocusHandle,
}

impl EventEmitter<PhoneEvent> for PhoneView {}

impl PhoneView {
    pub fn new(identity: Identity, initial_call: Option<String>, cx: &mut Context<Self>) -> Self {
        let selected = initial_call.filter(|id| !id.trim().is_empty());
        // A short call starts under the header; a long one is shown from its end.
        let lines = ListState::new(0, ListAlignment::Top, px(600.));
        let view = cx.entity().downgrade();
        lines.set_scroll_handler(move |event, _, cx| {
            view.update(cx, |this, cx| {
                let at_latest = event.visible_range.end >= event.count;
                if this.at_latest != at_latest {
                    this.at_latest = at_latest;
                    cx.notify();
                }
            })
            .ok();
        });
        Self {
            identity,
            calls: Vec::new(),
            calls_loaded: false,
            calls_failed: None,
            selected,
            call: None,
            call_failed: None,
            lines,
            at_latest: true,
            end: EndState::Idle,
            focus: cx.focus_handle(),
        }
    }

    pub fn set_identity(&mut self, identity: Identity, cx: &mut Context<Self>) {
        self.identity = identity;
        cx.notify();
    }

    pub fn set_calls(&mut self, calls: Result<Vec<CallSummary>, String>, cx: &mut Context<Self>) {
        self.calls_loaded = true;
        match calls {
            Ok(calls) => {
                self.calls = calls;
                self.calls_failed = None;
            }
            Err(reason) => self.calls_failed = Some(reason.into()),
        }
        cx.notify();
    }

    /// Takes a transcript result only when it belongs to the call that is still open.
    /// A failed refresh keeps the last good transcript on screen.
    pub fn set_call(
        &mut self,
        call_id: &str,
        call: Result<PhoneCallTranscript, String>,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_deref() != Some(call_id) {
            return;
        }
        match call {
            Ok(call) if call.call_id == call_id => {
                let old_count = self.call.as_ref().map_or(0, |call| call.lines.len() + 1);
                let unchanged = self
                    .call
                    .as_ref()
                    .map_or(0, |old| unchanged_items(old, &call));
                let new_count = call.lines.len() + 1;
                if unchanged < old_count || unchanged < new_count {
                    self.lines
                        .splice(unchanged..old_count, new_count - unchanged);
                }
                if self.at_latest {
                    self.lines.scroll_to(ListOffset {
                        item_ix: new_count,
                        offset_in_item: px(0.),
                    });
                }
                // A successful end request remains authoritative while the server still says
                // the call is live. A later stale live snapshot must not re-enable End call.
                if !call.live
                    || (call.hangup_requested
                        && matches!(self.end, EndState::Confirm { .. } | EndState::Failed { .. }))
                {
                    self.end = EndState::Idle;
                }
                self.call = Some(call);
                self.call_failed = None;
            }
            Ok(_) => return,
            Err(reason) => self.call_failed = Some(reason.into()),
        }
        cx.notify();
    }

    /// Finishes only the end request for the call that is still open. Results for a call the user
    /// has left cannot change the new screen.
    pub fn set_end_result(
        &mut self,
        call_id: &str,
        result: Result<(), String>,
        cx: &mut Context<Self>,
    ) {
        if self.selected.as_deref() != Some(call_id)
            || !matches!(self.end, EndState::Pending { .. })
        {
            return;
        }
        let action_id = match &self.end {
            EndState::Pending { action_id } => action_id.clone(),
            _ => unreachable!(),
        };
        self.end = match result {
            Ok(()) => EndState::Sent,
            Err(reason) => EndState::Failed {
                action_id,
                reason: reason.into(),
            },
        };
        cx.notify();
    }

    pub fn selected_call(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    fn open(&mut self, call_id: String, cx: &mut Context<Self>) {
        if call_id.trim().is_empty() {
            return;
        }
        let old_count = self.call.as_ref().map_or(0, |call| call.lines.len() + 1);
        if old_count > 0 {
            self.lines.splice(0..old_count, 0);
        }
        self.selected = Some(call_id.clone());
        self.call = None;
        self.call_failed = None;
        self.end = EndState::Idle;
        self.at_latest = true;
        cx.emit(PhoneEvent::Open(call_id));
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        let old_count = self.call.as_ref().map_or(0, |call| call.lines.len() + 1);
        if old_count > 0 {
            self.lines.splice(0..old_count, 0);
        }
        self.selected = None;
        self.call = None;
        self.call_failed = None;
        self.end = EndState::Idle;
        self.at_latest = true;
        cx.emit(PhoneEvent::Back);
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        match self.end {
            EndState::Confirm { .. } => {
                self.end = EndState::Idle;
                cx.notify();
            }
            _ if self.selected.is_some() => self.back(cx),
            _ => cx.emit(PhoneEvent::Close),
        }
    }

    fn can_end(&self) -> bool {
        self.call
            .as_ref()
            .is_some_and(|call| call.live && !call.hangup_requested)
    }

    fn show_end_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.can_end() && matches!(self.end, EndState::Idle) {
            self.end = EndState::Confirm {
                action_id: Uuid::new_v4().to_string(),
            };
            cx.notify();
        }
    }

    fn cancel_end(&mut self, cx: &mut Context<Self>) {
        if matches!(self.end, EndState::Confirm { .. }) {
            self.end = EndState::Idle;
            cx.notify();
        }
    }

    fn confirm_end(&mut self, cx: &mut Context<Self>) {
        if !self.can_end() {
            return;
        }
        let Some(call_id) = self.selected.clone() else {
            return;
        };
        let EndState::Confirm { action_id } = &self.end else {
            return;
        };
        let action_id = action_id.clone();
        self.end = EndState::Pending {
            action_id: action_id.clone(),
        };
        cx.emit(PhoneEvent::End { call_id, action_id });
        cx.notify();
    }

    fn retry_end(&mut self, cx: &mut Context<Self>) {
        if !self.can_end() {
            return;
        }
        let Some(call_id) = self.selected.clone() else {
            return;
        };
        let EndState::Failed { action_id, .. } = &self.end else {
            return;
        };
        let action_id = action_id.clone();
        self.end = EndState::Pending {
            action_id: action_id.clone(),
        };
        cx.emit(PhoneEvent::End { call_id, action_id });
        cx.notify();
    }

    fn dismiss_end_failure(&mut self, cx: &mut Context<Self>) {
        if matches!(self.end, EndState::Failed { .. }) {
            self.end = EndState::Idle;
            cx.notify();
        }
    }

    fn copy_transcript(&mut self, cx: &mut Context<Self>) {
        let Some(call) = &self.call else {
            return;
        };
        if call.lines.is_empty() {
            return;
        }
        let counterpart = nonempty(&call.counterpart_label).unwrap_or(&call.title);
        let text = call
            .lines
            .iter()
            .map(|line| {
                let speaker: &str = if line.speaker == "me" {
                    &self.identity.name
                } else {
                    counterpart
                };
                format!("{speaker}: {}", line.text)
            })
            .collect::<Vec<_>>()
            .join("\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn calls_screen(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let header = div()
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .px(px(20.))
            .pt(px(52.))
            .pb(px(14.))
            .flex()
            .items_center()
            .gap(px(12.))
            .child(own_face(self.identity.avatar.clone(), 40.))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(
                        div()
                            .text_size(px(20.))
                            .font_weight(FontWeight::MEDIUM)
                            .child("Phone calls"),
                    )
                    .child(
                        div()
                            .text_size(px(13.))
                            .line_height(px(18.))
                            .text_color(theme::color(theme::TEXT_MUTED))
                            .child("Review call status and transcripts"),
                    ),
            )
            .child(
                chrome("close-phone")
                    .size(px(38.))
                    .rounded_full()
                    .cursor_pointer()
                    .child(icon("x", 18., theme::TEXT))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(PhoneEvent::Close))),
            );

        let mut body = div()
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .px(px(20.))
            .pb(px(28.))
            .flex()
            .flex_col()
            .gap(px(10.));
        if !self.calls_loaded {
            body = body.child(status_card("Loading phone calls…", None, cx));
        } else if self.calls.is_empty() {
            body = body.child(status_card(
                self.calls_failed
                    .clone()
                    .unwrap_or_else(|| "No phone calls yet.".into()),
                self.calls_failed.as_ref().map(|_| PhoneEvent::Reload),
                cx,
            ));
        } else {
            if let Some(reason) = &self.calls_failed {
                body = body.child(status_card(reason.clone(), Some(PhoneEvent::Reload), cx));
            }
            body = body.children(
                self.calls
                    .iter()
                    .enumerate()
                    .map(|(index, call)| self.call_row(index, call, cx)),
            );
        }
        body = body.child(
            div()
                .pt(px(10.))
                .text_size(px(12.))
                .line_height(px(17.))
                .text_color(theme::color(theme::TEXT_FAINT))
                .child("Closing this view stops updates. It does not end a call."),
        );
        div()
            .id("phone-list-scroll")
            .size_full()
            .overflow_y_scroll()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(theme::COLUMN_MAX)
                    .child(header)
                    .child(body),
            )
    }

    fn call_row(&self, index: usize, call: &CallSummary, cx: &mut Context<Self>) -> Stateful<Div> {
        let call_id = call.id.clone();
        let is_live = is_live_state(&call.state);
        div()
            .id(("phone-call", index))
            .w_full()
            .p(px(16.))
            .rounded(px(20.))
            .bg(theme::color(theme::CARD))
            .border_1()
            .border_color(theme::color(theme::HAIRLINE))
            .flex()
            .items_center()
            .gap(px(12.))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| this.open(call_id.clone(), cx)))
            .child(
                div()
                    .size(px(38.))
                    .rounded_full()
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .bg(theme::alpha(0xffffff, 0.06))
                    .child(icon("phone", 18., theme::TEXT_MUTED)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(
                        div()
                            .text_size(px(16.))
                            .line_height(px(21.))
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(call.title.clone()),
                    )
                    .when(!call.subtitle.trim().is_empty(), |row| {
                        row.child(
                            div()
                                .text_size(px(14.))
                                .line_height(px(19.))
                                .text_color(theme::color(theme::TEXT_MUTED))
                                .truncate()
                                .child(call.subtitle.clone()),
                        )
                    })
                    .when(!call.state.trim().is_empty(), |row| {
                        row.child(
                            div()
                                .text_size(px(12.))
                                .line_height(px(16.))
                                .text_color(theme::color(if is_live {
                                    theme::MINE
                                } else {
                                    theme::TEXT_FAINT
                                }))
                                .child(call.state.clone()),
                        )
                    }),
            )
            .when(!call.at.trim().is_empty(), |row| {
                row.child(
                    div()
                        .max_w(px(90.))
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(theme::color(theme::TEXT_FAINT))
                        .text_align(gpui::TextAlign::Right)
                        .child(call.at.clone()),
                )
            })
            .child(icon("chevron-right", 17., theme::TEXT_FAINT))
    }

    fn detail_screen(&self, cx: &mut Context<Self>) -> Div {
        let call = self.call.as_ref();
        let title: SharedString = call
            .map(|call| call.title.clone().into())
            .or_else(|| {
                self.selected.as_ref().and_then(|id| {
                    self.calls
                        .iter()
                        .find(|call| &call.id == id)
                        .map(|call| call.title.clone().into())
                })
            })
            .unwrap_or_else(|| "Phone call".into());
        let header = self.detail_header(title, call, cx);

        let content = if let Some(call) = call {
            let items = list(
                self.lines.clone(),
                cx.processor(|this, index: usize, window, _| {
                    let call = this.call.as_ref().expect("list exists only for a call");
                    let item = if index == 0 {
                        div()
                            .w_full()
                            .text_align(gpui::TextAlign::Center)
                            .text_size(px(12. * TRACKED))
                            .line_height(px(14.5))
                            .text_color(theme::color(theme::TEXT_FAINT))
                            .child(call.started_label.clone())
                    } else {
                        transcript_line(
                            &call.lines[index - 1],
                            this.identity.avatar.clone(),
                            1.,
                            1.,
                            window,
                        )
                    };
                    // The lines keep to the design column of a wider window.
                    div()
                        .w_full()
                        .flex()
                        .justify_center()
                        .child(
                            div()
                                .w_full()
                                .max_w(theme::COLUMN_MAX)
                                .px(px(16.))
                                .child(item),
                        )
                        .into_any_element()
                }),
            )
            .size_full()
            .pt(px(HEADER_HEIGHT + 16.))
            .pb(detail_bottom_padding(call, &self.end));
            div().size_full().child(items)
        } else if let Some(reason) = &self.call_failed {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .px(px(24.))
                .child(status_card(
                    reason.clone(),
                    self.selected
                        .as_ref()
                        .map(|id| PhoneEvent::Refresh(id.clone())),
                    cx,
                ))
        } else {
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(
                    div()
                        .text_size(px(14.))
                        .text_color(theme::color(theme::TEXT_MUTED))
                        .child("Loading transcript…"),
                )
        };

        div()
            .size_full()
            .relative()
            .child(content)
            .child(header)
            .children(call.map(|call| self.detail_footer(call, cx)))
            .children(matches!(self.end, EndState::Confirm { .. }).then(|| self.end_dialog(cx)))
    }

    fn detail_header(
        &self,
        title: SharedString,
        call: Option<&PhoneCallTranscript>,
        cx: &mut Context<Self>,
    ) -> Div {
        // "Pickle's call with 02-000-0000": the server words it, and only the other side
        // stands out.
        let subtitle = call.map(|call| {
            let lead = call.subtitle_lead.len();
            let counterpart = lead..lead + call.counterpart_label.len();
            let text = format!(
                "{}{}{}",
                call.subtitle_lead, call.counterpart_label, call.subtitle_tail
            );
            StyledText::new(text).with_highlights([(
                counterpart,
                HighlightStyle {
                    color: Some(theme::color(theme::TEXT)),
                    ..Default::default()
                },
            )])
        });
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(HEADER_HEIGHT))
            .child(backdrop(px(HEADER_TOP + 11.), px(HEADER_HEIGHT)))
            .child(
                div()
                    .absolute()
                    .top(px(HEADER_TOP))
                    .left_0()
                    .right_0()
                    .h(px(44.))
                    .flex()
                    .justify_center()
                    .child(
                        div()
                            .relative()
                            .size_full()
                            .max_w(theme::COLUMN_MAX)
                            .px(px(72.))
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(3.))
                            .child(
                                div()
                                    .max_w_full()
                                    .text_size(px(16. * TRACKED))
                                    .line_height(px(21.))
                                    .truncate()
                                    .child(title),
                            )
                            .children(subtitle.map(|subtitle| {
                                div()
                                    .max_w_full()
                                    .pr(px(2.))
                                    .flex()
                                    .items_center()
                                    .gap(px(3.))
                                    .child(own_face(self.identity.avatar.clone(), 20.))
                                    .child(
                                        div()
                                            .min_w_0()
                                            .text_size(px(13. * TRACKED))
                                            .line_height(px(20.))
                                            .text_color(theme::color(theme::TEXT_MUTED))
                                            .truncate()
                                            .child(subtitle),
                                    )
                            }))
                            .child(
                                chrome("phone-back")
                                    .absolute()
                                    .top_0()
                                    .left(px(16.))
                                    .size(px(44.))
                                    .rounded_full()
                                    .cursor_pointer()
                                    .child(icon("chevron-left", 24., theme::TEXT))
                                    .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                            ),
                    ),
            )
    }

    fn detail_footer(&self, call: &PhoneCallTranscript, cx: &mut Context<Self>) -> Div {
        let mut inner = div()
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .px(px(12.))
            .pb(px(FOOTER_BOTTOM))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(12.));
        if let Some(reason) = &self.call_failed {
            let selected = self.selected.clone().unwrap_or_default();
            inner = inner.child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(px(8.))
                    .text_size(px(12.))
                    .text_color(theme::color(theme::DANGER))
                    .child(reason.clone())
                    .child(
                        div()
                            .id("refresh-phone-call")
                            .cursor_pointer()
                            .text_color(theme::color(theme::LINK))
                            .child("Retry")
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(PhoneEvent::Refresh(selected.clone()))
                            })),
                    ),
            );
        }
        inner = inner.child(
            div()
                .w_full()
                .text_align(gpui::TextAlign::Center)
                .text_size(px(15. * TRACKED))
                .line_height(px(18.))
                .text_color(theme::color(theme::TEXT_FAINT))
                .truncate()
                .child(call.footer.clone()),
        );
        inner = match &self.end {
            EndState::Idle if call.live && !call.hangup_requested => inner.child(
                chrome("end-phone-call")
                    .h(px(42.))
                    .pl(px(14.))
                    .pr(px(16.))
                    .rounded_full()
                    .cursor_pointer()
                    .gap(px(6.))
                    .text_size(px(15. * TRACKED))
                    .text_color(theme::color(theme::CALL_END))
                    .child(icon("phone-off", 18., theme::CALL_END))
                    .child("End call")
                    .on_click(cx.listener(|this, _, _, cx| this.show_end_confirmation(cx))),
            ),
            EndState::Pending { .. } | EndState::Sent => inner.child(
                div()
                    .h(px(42.))
                    .flex()
                    .items_center()
                    .text_size(px(14.))
                    .text_color(theme::color(theme::TEXT_MUTED))
                    .child("Ending call…"),
            ),
            EndState::Failed { reason, .. } => inner.child(
                div()
                    .w_full()
                    .rounded(px(16.))
                    .bg(theme::color(theme::CARD))
                    .p(px(12.))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_size(px(13.))
                            .text_color(theme::color(theme::DANGER))
                            .child(reason.clone()),
                    )
                    .child(
                        div()
                            .id("dismiss-end-failure")
                            .cursor_pointer()
                            .text_size(px(13.))
                            .child("Dismiss")
                            .on_click(cx.listener(|this, _, _, cx| this.dismiss_end_failure(cx))),
                    )
                    .child(
                        div()
                            .id("retry-end-phone-call")
                            .cursor_pointer()
                            .text_size(px(13.))
                            .text_color(theme::color(theme::LINK))
                            .child("Retry")
                            .on_click(cx.listener(|this, _, _, cx| this.retry_end(cx))),
                    ),
            ),
            EndState::Confirm { .. } | EndState::Idle => inner,
        };
        // The page covers the transcript up to the footer's text and fades out above it.
        div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .flex_col()
            .child(fade(px(28.), true))
            .child(
                div()
                    .w_full()
                    .flex()
                    .justify_center()
                    .bg(theme::alpha(theme::PAGE, BACKDROP))
                    .child(inner),
            )
    }

    fn end_dialog(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        div()
            .id("end-phone-scrim")
            .absolute()
            .inset_0()
            .occlude()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::alpha(0x000000, 0.45))
            .on_click(cx.listener(|this, _, _, cx| this.cancel_end(cx)))
            .child(
                div()
                    .id("end-phone-dialog")
                    .w_full()
                    .max_w(px(394.))
                    .mx(px(23.))
                    .p(px(20.))
                    .rounded(px(30.))
                    .bg(theme::color(theme::SHEET))
                    .occlude()
                    .flex()
                    .flex_col()
                    .gap(px(18.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_size(px(18.))
                                    .line_height(px(26.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child("End the call?"),
                            )
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .line_height(px(20.))
                                    .text_color(theme::color(theme::TEXT_MUTED))
                                    .child("Pickle will wrap up with a short goodbye."),
                            ),
                    )
                    .child(
                        div()
                            .w_full()
                            .flex()
                            .gap(px(8.))
                            .child(
                                div()
                                    .id("cancel-end-phone-call")
                                    .h(px(44.))
                                    .flex_1()
                                    .rounded_full()
                                    .bg(theme::alpha(0xffffff, 0.08))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .child("Cancel")
                                    .on_click(cx.listener(|this, _, _, cx| this.cancel_end(cx))),
                            )
                            .child(
                                div()
                                    .id("confirm-end-phone-call")
                                    .h(px(44.))
                                    .flex_1()
                                    .rounded_full()
                                    .bg(theme::color(theme::TEXT))
                                    .text_color(theme::color(theme::PAGE))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .child("Hang up")
                                    .on_click(cx.listener(|this, _, _, cx| this.confirm_end(cx))),
                            ),
                    ),
            )
    }
}

impl Focusable for PhoneView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for PhoneView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .track_focus(&self.focus)
            .key_context("Phone")
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.cancel(cx)))
            // The phone app has no control for it; here Cmd-C copies the open transcript.
            .on_action(cx.listener(|this, _: &input::Copy, _, cx| this.copy_transcript(cx)))
            .child(if self.selected.is_some() {
                self.detail_screen(cx).into_any_element()
            } else {
                self.calls_screen(cx).into_any_element()
            })
    }
}

/// The phone app sets its text tighter than the font's own spacing, which GPUI cannot do.
/// Labels drawn this much smaller are as wide as they are there.
pub(super) const TRACKED: f32 = 0.96;
/// The sizes at which the two sides' lines of a call are as wide as the phone app's 16 and
/// 15, so that they break at the same words.
const THEIR_TEXT: f32 = 15.5;
const MY_TEXT: f32 = 14.8;

/// How much of the page the backdrop of the header and of the footer keeps where it is solid.
const BACKDROP: f32 = 0.8;

/// The backdrop of the header: the page down to `solid`, fading out by `height`.
fn backdrop(solid: Pixels, height: Pixels) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(height)
        .flex()
        .flex_col()
        .child(
            div()
                .w_full()
                .h(solid)
                .flex_none()
                .bg(theme::alpha(theme::PAGE, BACKDROP)),
        )
        .child(fade(height - solid, false))
}

/// The edge of a backdrop: the page fading out downward, or upward for a footer. The three
/// steps are the stops of the phone app's backdrop.
fn fade(height: Pixels, upward: bool) -> Div {
    let stops = [(0., BACKDROP), (0.213, 0.695), (0.591, 0.442), (1., 0.)];
    let mut steps: Vec<Div> = stops
        .windows(2)
        .map(|pair| {
            let ((from, solid), (to, clear)) = (pair[0], pair[1]);
            let (top, bottom) = if upward {
                (clear, solid)
            } else {
                (solid, clear)
            };
            div().w_full().h(height * (to - from)).bg(linear_gradient(
                180.,
                linear_color_stop(theme::alpha(theme::PAGE, top), 0.),
                linear_color_stop(theme::alpha(theme::PAGE, bottom), 1.),
            ))
        })
        .collect();
    if upward {
        steps.reverse();
    }
    div().w_full().flex_none().flex().flex_col().children(steps)
}

/// One line of a call. `scale` shrinks every measure of the transcript screen and `text_scale`
/// its text: the phone card of the chat shows the same lines smaller.
pub(super) fn transcript_line(
    line: &PhoneCallLine,
    avatar: Option<ImageSource>,
    scale: f32,
    text_scale: f32,
    window: &Window,
) -> Div {
    let scaled = |measure: f32| px(measure * scale);
    let mine = line.speaker == "me";
    let keypad = line.keys.as_deref().is_some_and(|keys| !keys.is_empty());
    let round = if mine { 19. } else { 20. };
    let above = scaled(if line.joins_previous { 5. } else { round });
    let below = scaled(if line.joins_next { 5. } else { round });
    let (size, height) = if mine {
        (MY_TEXT, 22.)
    } else {
        (THEIR_TEXT, 20.)
    };
    // A bubble is at most 260 wide: its padding, the icon, the gap, and the text. The phone
    // app fills a line to the very edge; one point more lets the same line through here.
    let room = scaled(260. - 24. - 21. - 2. + 1.);
    let bubble = div()
        .max_w(scaled(260.))
        .px(scaled(12.))
        .pt(scaled(9.))
        .pb(scaled(11.))
        .flex()
        .items_start()
        .gap(scaled(2.))
        .bg(theme::color(theme::SURFACE))
        .rounded(scaled(round))
        // The corners toward the avatar's side close up between the lines of one turn.
        .when(mine, |bubble| bubble.rounded_tr(above).rounded_br(below))
        .when(!mine, |bubble| bubble.rounded_tl(above).rounded_bl(below))
        .child(
            div()
                .size(scaled(21.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .child(if keypad {
                    icon("keypad", 15. * scale, theme::TEXT_FAINT)
                } else {
                    icon("audio-lines", 17. * scale, theme::TEXT_FAINT)
                }),
        )
        .child(
            div()
                .min_w_0()
                .text_size(px(size * text_scale))
                .line_height(scaled(height))
                // The lines are broken already, each to the full width.
                .whitespace_nowrap()
                .child(keep_hangul(&line.text, px(size * text_scale), room, window)),
        );
    let row = div()
        .w_full()
        .pt(scaled(if line.joins_previous { 2. } else { 12. }))
        .flex()
        .items_end()
        .gap(scaled(12.));
    if mine {
        // Only the last line of a turn carries the picture; the others keep its room.
        row.justify_end().child(bubble).child(
            div()
                .size(scaled(28.))
                .flex_none()
                .when(!line.joins_next, |slot| {
                    slot.child(own_face(avatar, 28. * scale))
                }),
        )
    } else {
        row.justify_start().child(bubble)
    }
}

fn unchanged_items(old: &PhoneCallTranscript, new: &PhoneCallTranscript) -> usize {
    if old.started_label != new.started_label {
        return 0;
    }
    1 + old
        .lines
        .iter()
        .zip(&new.lines)
        .take_while(|(old, new)| same_line(old, new))
        .count()
}

fn same_line(a: &PhoneCallLine, b: &PhoneCallLine) -> bool {
    a.id == b.id
        && a.line_id == b.line_id
        && a.speaker == b.speaker
        && a.text == b.text
        && a.keys == b.keys
        && a.joins_previous == b.joins_previous
        && a.joins_next == b.joins_next
}

fn detail_bottom_padding(call: &PhoneCallTranscript, end: &EndState) -> gpui::Pixels {
    let has_control = (call.live && !call.hangup_requested) || !matches!(end, EndState::Idle);
    // The footer's text and the fade above it take 46 over the bottom margin.
    px(FOOTER_BOTTOM + 46. + if has_control { END_BUTTON_ROOM } else { 0. })
}

fn nonempty(value: &str) -> Option<&str> {
    (!value.trim().is_empty()).then_some(value)
}

fn is_live_state(state: &str) -> bool {
    matches!(
        state.trim().to_ascii_lowercase().as_str(),
        "live" | "running" | "active" | "in progress" | "in_progress"
    )
}

fn status_card(
    text: impl Into<SharedString>,
    action: Option<PhoneEvent>,
    cx: &mut Context<PhoneView>,
) -> Div {
    div()
        .w_full()
        .p(px(18.))
        .rounded(px(20.))
        .bg(theme::color(theme::CARD))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(14.))
                .line_height(px(20.))
                .text_color(theme::color(theme::TEXT_MUTED))
                .child(text.into()),
        )
        .children(action.map(|event| {
            div()
                .id("retry-phone-list")
                .cursor_pointer()
                .text_color(theme::color(theme::LINK))
                .child("Retry")
                .on_click(cx.listener(move |_, _, _, cx| match &event {
                    PhoneEvent::Reload => cx.emit(PhoneEvent::Reload),
                    PhoneEvent::Refresh(call_id) => cx.emit(PhoneEvent::Refresh(call_id.clone())),
                    _ => {}
                }))
        }))
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{Entity, TestAppContext};
    use serde_json::json;

    use super::*;

    fn transcript(id: &str, live: bool, text: &str) -> PhoneCallTranscript {
        serde_json::from_value(json!({
            "kind": "phone_call",
            "call_id": id,
            "title": "A call",
            "subtitle_lead": "with ",
            "counterpart_label": "someone",
            "subtitle_tail": "",
            "started_label": "Started recently",
            "live": live,
            "hangup_requested": false,
            "footer": if live { "Call in progress" } else { "Call ended" },
            "lines": [{
                "id": "line-1",
                "line_id": "line-1",
                "speaker": "them",
                "text": text,
                "keys": null,
                "joins_previous": false,
                "joins_next": false
            }],
            "started_at_epoch_ms": null,
            "answered_at_epoch_ms": null,
            "ended_at_epoch_ms": null,
            "duration_s": null,
            "outcome": null
        }))
        .expect("synthetic transcript follows the wire model")
    }

    fn summary(id: &str) -> CallSummary {
        CallSummary {
            id: id.into(),
            title: "A call".into(),
            subtitle: "Transcript available".into(),
            state: "ended".into(),
            at: "Today".into(),
        }
    }

    fn view(cx: &mut TestAppContext) -> (Entity<PhoneView>, &mut gpui::VisualTestContext) {
        cx.update(crate::ui::bind_keys);
        let (view, cx) = cx.add_window_view(|window, cx| {
            let view = PhoneView::new(Identity::default(), None, cx);
            window.focus(&view.focus_handle(cx));
            view
        });
        (view, cx)
    }

    #[gpui::test]
    fn list_opens_a_detail_and_back_stops_watching(cx: &mut TestAppContext) {
        let (view, cx) = view(cx);
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let events = events.clone();
            cx.subscribe(&view, move |_, event, _| match event {
                PhoneEvent::Open(id) => events.borrow_mut().push(format!("open:{id}")),
                PhoneEvent::Back => events.borrow_mut().push("back".into()),
                _ => {}
            })
            .detach();
        });
        view.update(cx, |view, cx| {
            view.set_calls(Ok(vec![summary("one")]), cx);
            view.open("one".into(), cx);
            view.set_call("one", Ok(transcript("one", false, "hello")), cx);
        });
        view.read_with(cx, |view, _| assert_eq!(view.selected_call(), Some("one")));
        view.update(cx, |view, cx| view.back(cx));
        view.read_with(cx, |view, _| assert_eq!(view.selected_call(), None));
        assert_eq!(&*events.borrow(), &["open:one", "back"]);
    }

    #[gpui::test]
    fn end_requires_confirmation_and_retries_reuse_the_action_id(cx: &mut TestAppContext) {
        let (view, cx) = view(cx);
        let ends = Rc::new(RefCell::new(Vec::<(String, String)>::new()));
        cx.update(|_, cx| {
            let ends = ends.clone();
            cx.subscribe(&view, move |_, event, _| {
                if let PhoneEvent::End { call_id, action_id } = event {
                    ends.borrow_mut().push((call_id.clone(), action_id.clone()));
                }
            })
            .detach();
        });
        view.update(cx, |view, cx| {
            view.open("live".into(), cx);
            view.set_call("live", Ok(transcript("live", true, "hello")), cx);
            view.show_end_confirmation(cx);
            view.cancel_end(cx);
            view.confirm_end(cx);
            assert!(ends.borrow().is_empty(), "cancel cannot end a call");
            view.show_end_confirmation(cx);
            view.confirm_end(cx);
            view.confirm_end(cx);
        });
        assert_eq!(ends.borrow().len(), 1, "a pending request is emitted once");
        let first_id = ends.borrow()[0].1.clone();
        view.update(cx, |view, cx| {
            view.set_end_result("live", Err("Could not end the call".into()), cx);
            view.retry_end(cx);
        });
        assert_eq!(ends.borrow().len(), 2);
        assert_eq!(ends.borrow()[1].1, first_id);
    }

    #[gpui::test]
    fn stale_results_and_status_transitions_do_not_change_the_open_call(cx: &mut TestAppContext) {
        let (view, cx) = view(cx);
        view.update(cx, |view, cx| {
            view.open("new".into(), cx);
            view.set_call("new", Ok(transcript("new", true, "new text")), cx);
            view.show_end_confirmation(cx);
            view.set_call("old", Ok(transcript("old", false, "stale text")), cx);
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.call.as_ref().unwrap().call_id, "new");
            assert!(matches!(view.end, EndState::Confirm { .. }));
        });
        view.update(cx, |view, cx| {
            view.set_call("new", Ok(transcript("new", false, "final text")), cx);
            view.set_end_result("old", Ok(()), cx);
        });
        view.read_with(cx, |view, _| {
            assert_eq!(view.call.as_ref().unwrap().lines[0].text, "final text");
            assert!(matches!(view.end, EndState::Idle));
        });
    }
}
