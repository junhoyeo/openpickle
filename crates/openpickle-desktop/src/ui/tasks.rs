//! The progress of a task Pickle handed off: a sheet over the chat with the task's plan and the
//! log of what it has done so far. The sheet only reads. The phone app's menu, which cancels
//! the task, is not here.

use std::{rc::Rc, sync::Arc, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, App, Context, Div, EventEmitter, FocusHandle, Focusable,
    FontWeight, MouseButton, Pixels, SharedString, Task, Window, div, linear_color_stop,
    linear_gradient, prelude::*, px, relative,
};
use openpickle_api::{HandoffItem, HandoffLogNode, HandoffScreen};

use crate::{
    content::{Block, LinkTarget, parse_message},
    model::{Part, Row, RowKind, Side},
    phone, theme,
    ui::{
        Cancel, Pictures, chrome, icon,
        phone::TRACKED,
        rows::{RowContext, rich_text},
        select::SharedSelection,
        wrap::{fit, keep_paths, line_width},
    },
};

/// The sheet stands this far inside the window until it is expanded.
const INSET: f32 = 8.;
const RADIUS: f32 = 36.;
/// How much of the window's height the sheet takes: as it opens, and expanded.
const HEIGHT: f32 = 0.52;
const HEIGHT_EXPANDED: f32 = 0.935;
/// The header: the handle, then the 44 pt row with the back button and the title.
const HEADER: f32 = 71.;
/// Where the header's row starts, from the sheet's top and from its sides.
const HEADER_ROW_TOP: f32 = 20.5;
const HEADER_ROW_SIDE: f32 = 22.5;
/// The space between a header button and the title.
const TITLE_GAP: f32 = 12.5;
/// The body's sides: as the sheet opens, and expanded.
const BODY_SIDE: f32 = 23.5;
const BODY_SIDE_EXPANDED: f32 = 25.;
/// The phone app's 16 pt with its tighter spacing is as wide as this here: the title, and the
/// text of the body, whose lines then break where they break there.
const TEXT: f32 = 14.9;
const TEXT_LINE: f32 = 19.25;
/// The status of a running task under its title, in the phone app's blue for it.
const STATUS: f32 = 13. * TRACKED;
const RUNNING: u32 = 0x5799f5;

pub enum TaskEvent {
    Close,
}

pub struct TaskView {
    path: String,
    /// What the handoff list said about the task, shown until its progress has been read.
    summary: HandoffItem,
    handoff: Option<HandoffScreen>,
    error: Option<SharedString>,
    /// Whether the plan is open instead of the log.
    plan: bool,
    expanded: bool,
    pictures: gpui::Entity<Pictures>,
    selection: SharedSelection,
    focus: FocusHandle,
    /// Redraws a running task each second, for the time it has taken.
    clock: Option<Task<()>>,
}

impl EventEmitter<TaskEvent> for TaskView {}

impl TaskView {
    pub fn new(
        path: String,
        summary: HandoffItem,
        pictures: gpui::Entity<Pictures>,
        cx: &mut Context<Self>,
    ) -> Self {
        let clock = (summary.state == "running").then(|| {
            cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    let running = this.update(cx, |this, cx| {
                        let running = this.running();
                        if running {
                            cx.notify();
                        } else {
                            this.clock = None;
                        }
                        running
                    });
                    if !matches!(running, Ok(true)) {
                        break;
                    }
                }
            })
        });
        Self {
            path,
            summary,
            handoff: None,
            error: None,
            plan: false,
            expanded: false,
            pictures,
            selection: SharedSelection::default(),
            focus: cx.focus_handle(),
            clock,
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows the sheet expanded, or its plan, without a click: for the gallery.
    pub fn show(&mut self, expanded: bool, plan: bool, cx: &mut Context<Self>) {
        self.expanded = expanded;
        self.plan = plan;
        cx.notify();
    }

    /// Takes a reading of the task's progress, or the reason there is none.
    pub fn set_handoff(
        &mut self,
        path: &str,
        result: Result<HandoffScreen, String>,
        cx: &mut Context<Self>,
    ) {
        if path != self.path {
            return;
        }
        match result {
            Ok(handoff) => {
                self.handoff = Some(handoff);
                self.error = None;
            }
            Err(reason) => self.error = Some(reason.into()),
        }
        cx.notify();
    }

    fn title(&self) -> &str {
        self.handoff
            .as_ref()
            .map_or(&self.summary.title, |handoff| &handoff.title)
    }

    fn running(&self) -> bool {
        self.handoff
            .as_ref()
            .map_or(&self.summary.state, |handoff| &handoff.state)
            == "running"
    }

    fn latest_log(&self) -> Option<&str> {
        match &self.handoff {
            Some(handoff) => handoff.latest_log_text.as_deref(),
            None => self.summary.latest_log_text.as_deref(),
        }
    }

    fn requested_at(&self) -> f64 {
        self.handoff
            .as_ref()
            .map_or(self.summary.requested_at_epoch_ms, |handoff| {
                handoff.requested_at_epoch_ms
            })
    }

    fn side(&self) -> f32 {
        if self.expanded {
            BODY_SIDE_EXPANDED
        } else {
            BODY_SIDE
        }
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if self.plan {
            self.plan = false;
            cx.notify();
        } else {
            cx.emit(TaskEvent::Close);
        }
    }

    /// Under the title: what a running task is doing and for how long, or the handoff list's
    /// line about a task that has ended.
    fn status(&self, room: Pixels, window: &Window) -> Option<Div> {
        let line = div()
            .flex()
            .justify_center()
            .gap(px(3.))
            .text_size(px(STATUS))
            .line_height(px(17.))
            .whitespace_nowrap();
        let fit_status =
            |text: &str, room: Pixels| fit(text, px(STATUS), FontWeight::NORMAL, room, window);
        if !self.running() {
            let subtitle = self.summary.subtitle.as_deref()?;
            return Some(
                line.text_color(theme::color(theme::TEXT_FAINT))
                    .child(fit_status(subtitle, room)),
            );
        }
        let elapsed = elapsed_label(self.requested_at(), phone::now_ms());
        let line = line.text_color(theme::color(RUNNING));
        Some(match self.latest_log() {
            Some(log) => {
                let elapsed = format!("· {elapsed}");
                let taken = line_width(&elapsed, px(STATUS), FontWeight::NORMAL, window);
                line.child(fit_status(log, room - px(3.) - taken))
                    .child(elapsed)
            }
            None => line.child(elapsed),
        })
    }

    /// The header lies over the body, which scrolls out under it.
    fn header(&self, width: Pixels, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let room = width - px(2. * (HEADER_ROW_SIDE + 44. + TITLE_GAP));
        let title = if self.plan { "Plan" } else { self.title() };
        let solid = 36. / (HEADER + 16.);
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(HEADER + 16.))
            .rounded_t(px(RADIUS))
            .bg(linear_gradient(
                180.,
                linear_color_stop(theme::color(theme::CHROME), solid),
                linear_color_stop(theme::alpha(theme::CHROME, 0.), 1.),
            ))
            .child(
                div()
                    .id("task-handle")
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(HEADER_ROW_TOP))
                    .flex()
                    .justify_center()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.expanded = !this.expanded;
                        cx.notify();
                    }))
                    .child(
                        div()
                            .mt(px(5.))
                            .w(px(35.))
                            .h(px(5.))
                            .rounded_full()
                            .bg(theme::color(theme::PLACEHOLDER)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px(HEADER_ROW_TOP))
                    .left(px(HEADER_ROW_SIDE))
                    .right(px(HEADER_ROW_SIDE))
                    .h(px(44.))
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        chrome("task-back")
                            .size(px(44.))
                            .rounded_full()
                            .cursor_pointer()
                            .child(icon("chevron-left", 22., theme::TEXT))
                            .on_click(cx.listener(|this, _, _, cx| this.back(cx))),
                    )
                    .child(
                        div()
                            .w(room)
                            .flex()
                            .flex_col()
                            .items_center()
                            .whitespace_nowrap()
                            .child(
                                div()
                                    .text_size(px(TEXT))
                                    .line_height(px(22.))
                                    .font_weight(FontWeight::MEDIUM)
                                    .child(fit(title, px(TEXT), FontWeight::MEDIUM, room, window)),
                            )
                            .children(
                                (!self.plan)
                                    .then(|| self.status(room, window))
                                    .flatten()
                                    .map(|status| status.pt(px(0.5))),
                            ),
                    )
                    // The phone app has the task's menu here.
                    .child(div().size(px(44.))),
            )
    }

    fn log(&self, width: Pixels, window: &Window, cx: &mut Context<Self>) -> impl IntoElement {
        let handoff = self.handoff.as_ref();
        let has_plan = handoff.is_some_and(|handoff| {
            handoff
                .plan_blocks
                .iter()
                .any(|block| block.markdown.is_some())
        });
        let nodes = handoff.map_or(&[][..], |handoff| handoff.log_nodes.as_slice());
        // What a line of the log may fill: the body without the row's icon.
        let side = self.side();
        let room = width - px(2. * side + 3. + 22. + 4. + 4.5);
        let running = self.running();
        div()
            .w_full()
            .px(px(side))
            .pt(px(HEADER + 16.))
            .pb(px(62.))
            .flex()
            .flex_col()
            .children(self.error.clone().map(|error| {
                div()
                    .pb(px(12.))
                    .text_size(px(14.))
                    .line_height(px(19.))
                    .text_color(theme::color(theme::TEXT_FAINT))
                    .child(error)
            }))
            .children(has_plan.then(|| {
                div().py(px(4.)).child(
                    div()
                        .id("task-plan")
                        .h(px(48.))
                        .px(px(12.))
                        .rounded(px(18.))
                        .bg(theme::alpha(0xffffff, 0.05))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .cursor_pointer()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.plan = true;
                            cx.notify();
                        }))
                        .child(
                            div()
                                .size(px(24.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon("clipboard-list", 15., theme::TEXT)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .text_size(px(TEXT))
                                .opacity(0.8)
                                .whitespace_nowrap()
                                .child(fit(
                                    self.title(),
                                    px(TEXT),
                                    FontWeight::NORMAL,
                                    width - px(2. * side + 2. * 12. + 2. * 24. + 2. * 4.),
                                    window,
                                )),
                        )
                        .child(
                            div()
                                .size(px(24.))
                                .flex_none()
                                .flex()
                                .items_center()
                                .justify_center()
                                .opacity(0.35)
                                .child(icon("chevron-right", 18., theme::TEXT)),
                        ),
                )
            }))
            .children((!nodes.is_empty()).then(|| {
                div()
                    .mt(px(2.))
                    .ml(px(12.5))
                    .w(px(1.))
                    .h(px(9.))
                    .rounded_full()
                    .bg(theme::color(theme::HAIRLINE))
            }))
            .child(
                div()
                    .mt(px(2.))
                    .flex()
                    .flex_col()
                    .children(nodes.iter().enumerate().map(|(index, node)| {
                        let last = index + 1 == nodes.len();
                        log_node(node, last, running, room, window)
                    })),
            )
    }

    fn plan_body(
        &self,
        width: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let texts: Vec<_> = self
            .handoff
            .iter()
            .flat_map(|handoff| &handoff.plan_blocks)
            .filter_map(|block| block.markdown.as_deref())
            .flat_map(parse_message)
            .filter_map(|block| match block {
                Block::Bubble(text) => Some(text),
                _ => None,
            })
            .collect();
        let on_link: super::select::LinkHandler = Rc::new(|target, _, cx| {
            if let Some(LinkTarget::Web(url)) = LinkTarget::parse(target) {
                cx.open_url(&url);
            }
        });
        let side = self.side() + 4.;
        let wrap = width - px(2. * side);
        let paragraphs: Vec<AnyElement> = texts
            .iter()
            .enumerate()
            .map(|(index, text)| {
                let row = Row {
                    kind: RowKind::Part(Part::Bubble(Arc::new(text.clone()))),
                    side: Side::Theirs,
                    joined_above: false,
                    joined_below: false,
                    decorations: Default::default(),
                };
                let row = RowContext::new(
                    window,
                    &row,
                    index,
                    &self.selection,
                    on_link.clone(),
                    self.pictures.read(cx),
                );
                rich_text(text, Some(wrap), &row)
            })
            .collect();
        div()
            .w_full()
            .px(px(side))
            .pt(px(HEADER + 16.))
            .pb(px(62.))
            .flex()
            .flex_col()
            .gap(px(14.))
            .children(paragraphs)
    }
}

/// The time since a task was requested, as the phone app counts it: "2:17".
pub(super) fn elapsed_label(requested_at_epoch_ms: f64, now_ms: f64) -> String {
    let seconds = ((now_ms - requested_at_epoch_ms) / 1000.).max(0.) as u64;
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// The icon of a kind of log entry. A kind the phone app has no icon for has none here.
fn node_icon(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "report" => "message-square",
        "start" => "play",
        "finish" => "clock",
        "page" => "files",
        "shell" => "square-terminal",
        "web" => "globe",
        "browser" => "monitor",
        _ => return None,
    })
}

/// One entry of the log: its icon, the line down to the next entry, and its text. What the
/// character reported stands out from what the task did. The last entry of a running task
/// pulses, where the phone app has a shimmer.
fn log_node(
    node: &HandoffLogNode,
    last: bool,
    running: bool,
    room: Pixels,
    window: &Window,
) -> AnyElement {
    let report = node.kind == "report";
    let text = node.text.as_deref().unwrap_or(&node.kind);
    let column = div()
        .w(px(22.))
        .flex_none()
        .flex()
        .flex_col()
        .items_center()
        .child(
            div()
                .size(px(22.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .opacity(0.42)
                .children(node_icon(&node.kind).map(|name| icon(name, 14., theme::TEXT))),
        )
        .when(!last, |column| {
            column.child(
                div()
                    .mt(px(3.))
                    .mb(px(3.5))
                    .mr(px(1.))
                    .w(px(1.))
                    .flex_1()
                    .rounded_full()
                    .bg(theme::color(theme::HAIRLINE)),
            )
        });
    let row = div()
        .pl(px(3.))
        .pr(px(4.5))
        .pt(px(0.75))
        .flex()
        .gap(px(4.))
        .child(column)
        .child(
            div()
                .pt(px(2.))
                .pb(px(if last { 2. } else { 16.5 }))
                .text_size(px(TEXT))
                .line_height(px(TEXT_LINE))
                // Opacity, not a paler color: an emoji in the text is dimmed with it.
                .opacity(if report { 0.9 } else { 0.45 })
                .whitespace_nowrap()
                .child(keep_paths(text, px(TEXT), room, window)),
        );
    if last && running {
        row.with_animation(
            "running-task",
            Animation::new(Duration::from_millis(1500))
                .repeat()
                .with_easing(gpui::pulsating_between(0.5, 1.)),
            |row, opacity| row.opacity(opacity),
        )
        .into_any_element()
    } else {
        row.into_any_element()
    }
}

impl Render for TaskView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let inset = if self.expanded { 0. } else { INSET };
        let width = window.viewport_size().width.min(theme::COLUMN_MAX) - px(2. * inset);
        let body = if self.plan {
            self.plan_body(width, window, cx).into_any_element()
        } else {
            self.log(width, window, cx).into_any_element()
        };
        let sheet = div()
            .id("task-sheet")
            .occlude()
            .relative()
            .w(width)
            .h(relative(if self.expanded {
                HEIGHT_EXPANDED
            } else {
                HEIGHT
            }))
            .mb(px(inset))
            .rounded_t(px(RADIUS))
            .when(!self.expanded, |sheet| sheet.rounded_b(px(RADIUS)))
            .bg(theme::color(theme::CARD))
            .overflow_hidden()
            // Scrolling the log asks for more of it: the sheet takes the window.
            .on_scroll_wheel(cx.listener(|this, _, _, cx| {
                if !this.expanded {
                    this.expanded = true;
                    cx.notify();
                }
            }))
            .child(
                div()
                    .id("task-body")
                    .size_full()
                    .overflow_y_scroll()
                    .child(body),
            )
            .child(self.header(width, window, cx));
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .occlude()
            .track_focus(&self.focus)
            .key_context("Task")
            .on_action(cx.listener(|this, _: &Cancel, _, cx| this.back(cx)))
            .bg(theme::alpha(0x000000, 0.5))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .flex()
            .flex_col()
            .justify_end()
            .items_center()
            // A press beside the sheet closes it.
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|_, _, _, cx| cx.emit(TaskEvent::Close)),
            )
            .child(sheet)
    }
}

impl Focusable for TaskView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_of_a_task_counts_from_its_request() {
        assert_eq!(elapsed_label(1_000., 138_999.), "2:17");
        // A clock that is behind the server's does not count backwards.
        assert_eq!(elapsed_label(2_000., 1_000.), "0:00");
    }
}
