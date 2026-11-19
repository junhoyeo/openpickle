//! The read-only progress sheet for a task Pickle handed off, with its plan and activity log.

use std::{rc::Rc, sync::Arc, time::Duration};

use gpui::{
    Animation, AnimationExt, AnyElement, App, ClipboardItem, Context, Div, EventEmitter,
    FocusHandle, Focusable, FontWeight, ListAlignment, ListState, MouseButton, Pixels,
    SharedString, Task, Window, div, linear_color_stop, linear_gradient, list, prelude::*, px,
    relative,
};
use openpickle_api::{HandoffItem, HandoffLogNode, HandoffPlanBlock, HandoffScreen};

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
/// Keep a small buffer of measured entries around the visible log.
const LOG_OVERDRAW: f32 = 180.;

#[derive(PartialEq)]
struct TaskDetails {
    title: String,
    state: String,
    requested_at_epoch_ms: f64,
    latest_log_text: Option<String>,
}

#[derive(Default)]
struct TaskPlan {
    markdown: Vec<String>,
    rows: Vec<Row>,
}

impl TaskPlan {
    fn update(&mut self, blocks: Vec<HandoffPlanBlock>) -> bool {
        let markdown = blocks.iter().filter_map(|block| block.markdown.as_ref());
        if self.markdown.iter().eq(markdown) {
            return false;
        }
        self.markdown = blocks
            .into_iter()
            .filter_map(|block| block.markdown)
            .collect();
        self.rows = self
            .markdown
            .iter()
            .flat_map(|source| parse_message(source))
            .filter_map(|block| match block {
                Block::Bubble(text) => Some(Row {
                    kind: RowKind::Part(Part::Bubble(Arc::new(text))),
                    side: Side::Theirs,
                    joined_above: false,
                    joined_below: false,
                    decorations: Default::default(),
                }),
                _ => None,
            })
            .collect();
        true
    }
}

struct LogRow {
    text: SharedString,
    icon: Option<&'static str>,
    report: bool,
    wrapped: Option<(Pixels, SharedString)>,
}

impl LogRow {
    fn new(node: HandoffLogNode) -> Self {
        let icon = node_icon(&node.kind);
        let report = node.kind == "report";
        Self {
            text: node.text.unwrap_or(node.kind).into(),
            icon,
            report,
            wrapped: None,
        }
    }

    fn matches(&self, node: &HandoffLogNode) -> bool {
        self.text == node.text.as_deref().unwrap_or(&node.kind)
            && self.icon == node_icon(&node.kind)
            && self.report == (node.kind == "report")
    }

    fn wrap(&mut self, width: Pixels, window: &Window) -> SharedString {
        if let Some((cached_width, text)) = &self.wrapped
            && *cached_width == width
        {
            return text.clone();
        }
        let text = keep_paths(&self.text, px(TEXT), width, window);
        self.wrapped = Some((width, text.clone()));
        text
    }
}

struct TaskLog {
    rows: Vec<LogRow>,
    list: ListState,
}

impl TaskLog {
    fn new() -> Self {
        Self {
            rows: Vec::new(),
            // Item 0 is the log's header, including the optional plan and error.
            list: ListState::new(1, ListAlignment::Top, px(LOG_OVERDRAW)),
        }
    }

    fn update(&mut self, nodes: Vec<HandoffLogNode>) -> bool {
        let unchanged = self
            .rows
            .iter()
            .zip(&nodes)
            .take_while(|(old, new)| old.matches(new))
            .count();
        let old_len = self.rows.len();
        let new_len = nodes.len();
        if unchanged == old_len && unchanged == new_len {
            return false;
        }
        // Appending or truncating changes the spacing of the last shared row even when its
        // contents haven't changed.
        let remeasure = unchanged.min(old_len.min(new_len).saturating_sub(1));
        let offset = self.list.logical_scroll_top();
        self.list
            .splice(remeasure + 1..old_len + 1, new_len - remeasure);
        if offset.item_ix <= unchanged {
            self.list.scroll_to(offset);
        }
        self.rows.truncate(unchanged);
        self.rows
            .extend(nodes.into_iter().skip(unchanged).map(LogRow::new));
        true
    }

    fn remeasure_header(&self) {
        let offset = self.list.logical_scroll_top();
        // Replacing the current item resets its within-item scroll offset in GPUI.
        self.list.splice(0..1, 1);
        if offset.item_ix == 0 {
            self.list.scroll_to(offset);
        }
    }

    fn text(&self) -> String {
        self.rows
            .iter()
            .map(|row| row.text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub enum TaskEvent {
    Close,
}

pub struct TaskView {
    path: String,
    /// What the handoff list said about the task, shown until its progress has been read.
    summary: HandoffItem,
    details: Option<TaskDetails>,
    error: Option<SharedString>,
    plan: TaskPlan,
    log: TaskLog,
    /// Whether the plan is open instead of the log.
    showing_plan: bool,
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
        let mut view = Self {
            path,
            summary,
            details: None,
            error: None,
            plan: TaskPlan::default(),
            log: TaskLog::new(),
            showing_plan: false,
            expanded: false,
            pictures,
            selection: SharedSelection::default(),
            focus: cx.focus_handle(),
            clock: None,
        };
        view.update_clock(cx);
        view
    }

    fn update_clock(&mut self, cx: &mut Context<Self>) {
        if !self.running() || self.showing_plan {
            self.clock = None;
        } else if self.clock.is_none() {
            self.clock = Some(cx.spawn(async move |this, cx| {
                loop {
                    cx.background_executor().timer(Duration::from_secs(1)).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        break;
                    }
                }
            }));
        }
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    /// Shows the sheet expanded, or its plan, without a click: for the gallery.
    pub fn show(&mut self, expanded: bool, plan: bool, cx: &mut Context<Self>) {
        self.expanded = expanded;
        self.set_showing_plan(plan, cx);
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
        let plan_was_empty = self.plan.rows.is_empty();
        let log_was_empty = self.log.rows.is_empty();
        let (changed, error_changed) = match result {
            Ok(handoff) => {
                let details = Some(TaskDetails {
                    title: handoff.title,
                    state: handoff.state,
                    requested_at_epoch_ms: handoff.requested_at_epoch_ms,
                    latest_log_text: handoff.latest_log_text,
                });
                let details_changed = details != self.details;
                if details_changed {
                    self.details = details;
                }
                let plan_changed = self.plan.update(handoff.plan_blocks);
                if plan_changed {
                    self.selection.borrow_mut().rows_changed(0);
                    if self.plan.rows.is_empty() {
                        self.showing_plan = false;
                    }
                }
                let log_changed = self.log.update(handoff.log_nodes);
                (
                    details_changed || plan_changed || log_changed,
                    self.error.take().is_some(),
                )
            }
            Err(reason) => {
                let error_changed = self.error.as_ref().is_none_or(|error| error != &reason);
                if error_changed {
                    self.error = Some(reason.into());
                }
                (false, error_changed)
            }
        };
        if error_changed
            || plan_was_empty != self.plan.rows.is_empty()
            || log_was_empty != self.log.rows.is_empty()
        {
            self.log.remeasure_header();
        }
        if changed || error_changed {
            self.update_clock(cx);
            cx.notify();
        }
    }

    fn title(&self) -> &str {
        self.details
            .as_ref()
            .map_or(&self.summary.title, |details| &details.title)
    }

    fn running(&self) -> bool {
        self.details
            .as_ref()
            .map_or(&self.summary.state, |details| &details.state)
            == "running"
    }

    fn latest_log(&self) -> Option<&str> {
        match &self.details {
            Some(details) => details.latest_log_text.as_deref(),
            None => self.summary.latest_log_text.as_deref(),
        }
    }

    fn requested_at(&self) -> f64 {
        self.details
            .as_ref()
            .map_or(self.summary.requested_at_epoch_ms, |details| {
                details.requested_at_epoch_ms
            })
    }

    fn side(&self) -> f32 {
        if self.expanded {
            BODY_SIDE_EXPANDED
        } else {
            BODY_SIDE
        }
    }

    fn set_showing_plan(&mut self, showing_plan: bool, cx: &mut Context<Self>) {
        self.showing_plan = showing_plan;
        self.update_clock(cx);
        cx.notify();
    }

    fn back(&mut self, cx: &mut Context<Self>) {
        if self.showing_plan {
            self.set_showing_plan(false, cx);
        } else {
            cx.emit(TaskEvent::Close);
        }
    }

    fn copy(&self, cx: &mut Context<Self>) {
        let text = if self.showing_plan {
            self.plan.markdown.join("\n\n")
        } else {
            self.log.text()
        };
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
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
        let (title, can_copy) = if self.showing_plan {
            ("Plan", !self.plan.rows.is_empty())
        } else {
            (self.title(), !self.log.rows.is_empty())
        };
        let solid = 36. / (HEADER + 16.);
        div()
            .absolute()
            .top_0()
            .left_0()
            .right_0()
            .h(px(HEADER + 16.))
            .block_mouse_except_scroll()
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
                                (!self.showing_plan)
                                    .then(|| self.status(room, window))
                                    .flatten()
                                    .map(|status| status.pt(px(0.5))),
                            ),
                    )
                    .child(if can_copy {
                        chrome("task-copy")
                            .size(px(44.))
                            .rounded_full()
                            .text_size(px(12.))
                            .cursor_pointer()
                            .child("Copy")
                            .on_click(cx.listener(|this, _, _, cx| this.copy(cx)))
                            .into_any_element()
                    } else {
                        div().size(px(44.)).into_any_element()
                    }),
            )
    }

    fn log(&self, width: Pixels, cx: &mut Context<Self>) -> impl IntoElement {
        let side = self.side();
        let room = width - px(2. * side + 3. + 22. + 4. + 4.5);
        list(
            self.log.list.clone(),
            cx.processor(move |this, index: usize, window, cx| {
                if index == 0 {
                    return this.log_header(width, window, cx).into_any_element();
                }
                let last = index == this.log.rows.len();
                let running = this.running();
                div()
                    .w_full()
                    .px(px(side))
                    .child(log_node(
                        &mut this.log.rows[index - 1],
                        last,
                        running,
                        room,
                        window,
                    ))
                    .into_any_element()
            }),
        )
        .size_full()
        .pb(px(62.))
    }

    fn log_header(
        &self,
        width: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let side = self.side();
        div()
            .w_full()
            .px(px(side))
            .pt(px(HEADER + 16.))
            .pb(px(2.))
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
            .children((!self.plan.rows.is_empty()).then(|| {
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
                        .on_click(cx.listener(|this, _, _, cx| this.set_showing_plan(true, cx)))
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
            .children((!self.log.rows.is_empty()).then(|| {
                div()
                    .mt(px(2.))
                    .ml(px(12.5))
                    .w(px(1.))
                    .h(px(9.))
                    .rounded_full()
                    .bg(theme::color(theme::HAIRLINE))
            }))
    }

    fn plan_body(
        &self,
        width: Pixels,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let on_link: super::select::LinkHandler = Rc::new(|target, _, cx| {
            if let Some(LinkTarget::Web(url)) = LinkTarget::parse(target) {
                cx.open_url(&url);
            }
        });
        let side = self.side() + 4.;
        let wrap = width - px(2. * side);
        let pictures = self.pictures.read(cx);
        let paragraphs = self.plan.rows.iter().enumerate().map(|(index, row)| {
            let RowKind::Part(Part::Bubble(text)) = &row.kind else {
                unreachable!("only parsed plan paragraphs are stored");
            };
            let cx = RowContext::new(
                window,
                row,
                index,
                &self.selection,
                on_link.clone(),
                pictures,
            );
            rich_text(text, Some(wrap), &cx)
        });
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
    node: &mut LogRow,
    last: bool,
    running: bool,
    room: Pixels,
    window: &Window,
) -> AnyElement {
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
                .children(node.icon.map(|name| icon(name, 14., theme::TEXT))),
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
                .opacity(if node.report { 0.9 } else { 0.45 })
                .whitespace_nowrap()
                .child(node.wrap(room, window)),
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
        let body = if self.showing_plan {
            div()
                .id("task-plan-body")
                .size_full()
                .overflow_y_scroll()
                .child(self.plan_body(width, window, cx))
                .into_any_element()
        } else {
            self.log(width, cx).into_any_element()
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
            .child(body)
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
    use gpui::{
        Entity, ListOffset, Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase,
        VisualTestContext, point, size,
    };

    use super::*;

    fn node(kind: &str, text: Option<&str>) -> HandoffLogNode {
        HandoffLogNode {
            kind: kind.into(),
            text: text.map(str::to_owned),
        }
    }

    fn plan_block(markdown: &str) -> HandoffPlanBlock {
        HandoffPlanBlock {
            r#type: "prose".into(),
            markdown: Some(markdown.into()),
        }
    }

    const PATH: &str = "handoffs/example";
    const WIDTH: Pixels = px(440.);
    const HEIGHT_PX: Pixels = px(900.);
    const PLAN: &str = "Check the [museum hours](https://example.com/museum).";

    fn open(
        cx: &mut TestAppContext,
        log_nodes: Vec<HandoffLogNode>,
    ) -> (Entity<TaskView>, &mut VisualTestContext) {
        let summary = serde_json::from_value(serde_json::json!({
            "name": "example", "title": "Museum trip", "state": "done",
            "requested_at_epoch_ms": 1000.0, "time_text": "Today"
        }))
        .unwrap();
        let (view, cx) = cx.add_window_view(|window, cx| {
            let pictures = cx.new(|_| Pictures::default());
            let view = TaskView::new(PATH.into(), summary, pictures, cx);
            window.focus(&view.focus_handle(cx));
            window.activate_window();
            view
        });
        cx.simulate_resize(size(WIDTH, HEIGHT_PX));
        view.update(cx, |view, cx| {
            view.set_handoff(
                PATH,
                Ok(HandoffScreen {
                    kind: "handoff".into(),
                    title: "Museum trip".into(),
                    state: "done".into(),
                    requested_at_epoch_ms: 1_000.,
                    latest_log_text: None,
                    latest_log_kind: None,
                    handoff_estimate_pct: None,
                    plan_blocks: vec![plan_block(PLAN)],
                    log_entries: Vec::new(),
                    log_nodes,
                }),
                cx,
            );
        });
        cx.run_until_parked();
        (view, cx)
    }

    #[test]
    fn the_time_of_a_task_counts_from_its_request() {
        assert_eq!(elapsed_label(1_000., 138_999.), "2:17");
        // A clock that is behind the server's does not count backwards.
        assert_eq!(elapsed_label(2_000., 1_000.), "0:00");
    }

    #[test]
    fn unchanged_plan_markdown_reuses_the_parsed_rows() {
        let mut plan = TaskPlan::default();
        assert!(plan.update(vec![plan_block(PLAN)]));
        let RowKind::Part(Part::Bubble(before)) = &plan.rows[0].kind else {
            panic!("expected a plan paragraph");
        };
        let before = before.clone();
        assert!(!plan.update(vec![plan_block(PLAN)]));
        let RowKind::Part(Part::Bubble(after)) = &plan.rows[0].kind else {
            panic!("expected the same plan paragraph");
        };
        assert!(Arc::ptr_eq(&before, after));
        assert_eq!(after.text, "Check the museum hours.");
        assert!(plan.update(vec![plan_block("**Completed**")]));
        assert_eq!(plan.rows[0].texts(), ["Completed"]);
    }

    #[test]
    fn a_log_update_keeps_the_reader_on_an_unchanged_row() {
        let mut log = TaskLog::new();
        let initial = || vec![node("start", None), node("shell", Some("Reading a guide"))];
        assert!(log.update(initial()));
        log.list.scroll_to(ListOffset {
            item_ix: 2,
            offset_in_item: px(5.),
        });

        let mut appended = initial();
        appended.push(node("new_kind", None));
        assert!(log.update(appended));
        assert_eq!(log.list.item_count(), 4);
        assert_eq!(log.list.logical_scroll_top().item_ix, 2);
        assert_eq!(log.list.logical_scroll_top().offset_in_item, px(5.));
        assert_eq!(log.text(), "start\nReading a guide\nnew_kind");
        assert!(log.rows[2].icon.is_none());

        assert!(log.update(initial()));
        assert_eq!(log.list.item_count(), 3);
        assert_eq!(log.list.logical_scroll_top().item_ix, 2);
        assert_eq!(log.list.logical_scroll_top().offset_in_item, px(5.));
        assert!(!log.update(initial()));
        assert!(log.update(vec![node("report", Some("Guide missing"))]));
        assert_eq!(log.list.item_count(), 2);
        assert_eq!(log.text(), "Guide missing");
    }

    #[test]
    fn remeasuring_the_log_header_does_not_rewind_it() {
        let log = TaskLog::new();
        log.list.scroll_to(ListOffset {
            item_ix: 0,
            offset_in_item: px(30.),
        });
        log.remeasure_header();
        assert_eq!(log.list.logical_scroll_top().item_ix, 0);
        assert_eq!(log.list.logical_scroll_top().offset_in_item, px(30.));
    }

    #[gpui::test]
    fn a_long_log_wraps_only_a_window_of_entries_and_scrolls(cx: &mut TestAppContext) {
        let log_nodes = (0..1_000)
            .map(|index| {
                node(
                    "shell",
                    Some(&format!("Checking notes/museum/{index}/guide.md")),
                )
            })
            .collect();
        let (view, cx) = open(cx, log_nodes);
        view.read_with(cx, |view, _| {
            assert_eq!(view.log.list.item_count(), 1_001);
            let wrapped = view
                .log
                .rows
                .iter()
                .filter(|row| row.wrapped.is_some())
                .count();
            assert!((1..100).contains(&wrapped), "wrapped {wrapped} entries");
        });
        cx.simulate_event(ScrollWheelEvent {
            position: point(WIDTH / 2., sheet_top() + px(HEADER + 4.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-260.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        view.read_with(cx, |view, _| {
            assert!(view.expanded);
            assert!(view.log.list.logical_scroll_top().item_ix > 0);
        });
    }

    fn sheet_top() -> Pixels {
        HEIGHT_PX * (1. - HEIGHT) - px(INSET)
    }

    #[gpui::test]
    fn the_sticky_header_blocks_clicks_through_to_the_scrolled_plan(cx: &mut TestAppContext) {
        let nodes = (0..30)
            .map(|_| node("shell", Some("Reading a guide")))
            .collect();
        let (view, cx) = open(cx, nodes);
        view.update(cx, |view, cx| {
            view.log.list.scroll_to(ListOffset {
                item_ix: 0,
                offset_in_item: px(30.),
            });
            cx.notify();
        });
        cx.run_until_parked();

        cx.simulate_click(
            point(WIDTH / 2., sheet_top() + px(HEADER + 4.)),
            Modifiers::none(),
        );
        assert!(!view.read_with(cx, |view, _| view.showing_plan));
        cx.simulate_click(
            point(WIDTH / 2., sheet_top() + px(HEADER + 29.)),
            Modifiers::none(),
        );
        assert!(view.read_with(cx, |view, _| view.showing_plan));
    }

    #[gpui::test]
    fn copy_uses_the_current_log_or_plan_even_after_a_refresh_error(cx: &mut TestAppContext) {
        let log = "Read notes/museum/visitor-guide.md";
        let (view, cx) = open(cx, vec![node("page", Some(log))]);
        view.update(cx, |view, cx| {
            view.set_handoff(PATH, Err("Offline".into()), cx)
        });
        cx.run_until_parked();
        let copy = point(
            WIDTH - px(INSET + HEADER_ROW_SIDE + 22.),
            sheet_top() + px(HEADER_ROW_TOP + 22.),
        );
        cx.simulate_click(copy, Modifiers::none());
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some(log)
        );

        view.update(cx, |view, cx| view.set_showing_plan(true, cx));
        cx.run_until_parked();
        cx.simulate_click(copy, Modifiers::none());
        assert_eq!(
            cx.read_from_clipboard()
                .and_then(|item| item.text())
                .as_deref(),
            Some(PLAN)
        );
    }
}
