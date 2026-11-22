//! The Archive tab: the pictures and files of the conversation, in two pages.

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, ObjectFit,
    SharedString, Subscription, Window, div, img, prelude::*, px,
};

use crate::{
    model::{Attachment, AttachmentKind},
    theme,
    ui::{Identity, Pictures, Tab, chrome, header_cover, icon, tab_bar, tab_header},
};

pub enum ArchiveEvent {
    Menu,
    Invite,
    Open(Tab),
    /// Go to the message that carries this attachment.
    ShowInChat(u64),
    PreviewFile(String),
    DownloadFile(String),
    /// Load the pictures of these attachments.
    Load(Vec<String>),
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    Files,
    Media,
}

pub struct ArchiveView {
    identity: Identity,
    pictures: Entity<Pictures>,
    attachments: Vec<Attachment>,
    /// Whether the whole history was read; until then the lists may still grow.
    complete: bool,
    page: Page,
    focus: FocusHandle,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ArchiveEvent> for ArchiveView {}

/// The key of an attachment's picture in the cache, as the backend names it.
pub fn thumbnail_key(path: &str) -> String {
    format!("thumbnail:{path}")
}

/// Attachments grouped by the day of their message, newest first: the day's label and its
/// attachments.
fn by_day<'a>(
    attachments: impl Iterator<Item = &'a Attachment>,
) -> Vec<(String, Vec<&'a Attachment>)> {
    let mut days: Vec<(String, Vec<&Attachment>)> = Vec::new();
    for attachment in attachments {
        let day = day_label(attachment.at);
        match days.last_mut() {
            Some((last, items)) if *last == day => items.push(attachment),
            _ => days.push((day, vec![attachment])),
        }
    }
    days
}

/// The local calendar date, including the year, e.g. `November 14, 2026`.
fn day_label(at: f64) -> String {
    use chrono::{Local, TimeZone};
    let Some(time) = Local.timestamp_opt(at as i64, 0).single() else {
        return String::new();
    };
    time.format("%B %-d, %Y").to_string()
}

impl ArchiveView {
    pub fn new(identity: Identity, pictures: Entity<Pictures>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&pictures, |_, _, cx| cx.notify())];
        Self {
            identity,
            pictures,
            attachments: Vec::new(),
            complete: false,
            page: Page::Media,
            focus: cx.focus_handle(),
            _subscriptions: subscriptions,
        }
    }

    pub fn set_identity(&mut self, identity: Identity, cx: &mut Context<Self>) {
        self.identity = identity;
        cx.notify();
    }

    /// Takes in the attachments and asks for the pictures of those not drawn yet.
    pub fn set_attachments(
        &mut self,
        attachments: Vec<Attachment>,
        complete: bool,
        cx: &mut Context<Self>,
    ) {
        let pictures = self.pictures.read(cx);
        let missing: Vec<String> = attachments
            .iter()
            .filter(|attachment| pictures.get(&thumbnail_key(&attachment.path)).is_none())
            .map(|attachment| attachment.path.clone())
            .collect();
        if !missing.is_empty() {
            cx.emit(ArchiveEvent::Load(missing));
        }
        self.attachments = attachments;
        self.complete = complete;
        cx.notify();
    }

    fn of_kind(&self, kind: AttachmentKind) -> impl Iterator<Item = &Attachment> {
        self.attachments
            .iter()
            .filter(move |attachment| attachment.kind == kind)
    }

    pub fn file(&self, path: &str) -> Option<&Attachment> {
        self.of_kind(AttachmentKind::File)
            .find(|attachment| attachment.path == path)
    }

    fn picture(&self, attachment: &Attachment, cx: &App) -> Option<gpui::ImageSource> {
        self.pictures.read(cx).get(&thumbnail_key(&attachment.path))
    }

    fn day_heading(label: String) -> Div {
        div()
            .px(px(20.))
            .text_size(px(13.))
            .line_height(px(18.))
            .child(label)
    }

    fn media(&self, window: &Window, cx: &mut Context<Self>) -> Div {
        let width = window.viewport_size().width.min(theme::COLUMN_MAX);
        let cell = (width - px(4. * 2.)) / 3.;
        let days = by_day(self.of_kind(AttachmentKind::Picture));
        let mut page = div().flex().flex_col().gap(px(16.));
        for (day, pictures) in days {
            let cells = pictures.into_iter().map(|attachment| {
                let seq = attachment.seq;
                let tile = div()
                    .id(SharedString::from(attachment.path.clone()))
                    .size(cell)
                    .flex_none()
                    .rounded(px(2.))
                    .overflow_hidden()
                    .bg(theme::alpha(0xffffff, 0.05))
                    .cursor_pointer()
                    .on_click(
                        cx.listener(move |_, _, _, cx| cx.emit(ArchiveEvent::ShowInChat(seq))),
                    );
                match self.picture(attachment, cx) {
                    Some(source) => {
                        tile.child(img(source).size_full().object_fit(ObjectFit::Cover))
                    }
                    None => tile,
                }
            });
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(Self::day_heading(day))
                    .child(div().flex().flex_wrap().gap(px(4.)).children(cells)),
            );
        }
        page
    }

    fn files(&self, cx: &mut Context<Self>) -> Div {
        let days = by_day(self.of_kind(AttachmentKind::File));
        let mut page = div().flex().flex_col().gap(px(16.));
        for (day, files) in days {
            let rows = files.into_iter().map(|attachment| {
                let seq = attachment.seq;
                let preview = div()
                    .size(px(58.))
                    .flex_none()
                    .rounded(px(6.))
                    .overflow_hidden()
                    .bg(theme::alpha(0xffffff, 0.05))
                    .border_1()
                    .border_color(theme::alpha(0xffffff, 0.075))
                    .flex()
                    .items_center()
                    .justify_center();
                let preview = match self.picture(attachment, cx) {
                    Some(source) => preview.child(
                        img(source)
                            .size_full()
                            .rounded(px(6.))
                            .object_fit(ObjectFit::Cover),
                    ),
                    None => preview.child(icon("file-text", 22., theme::TEXT_FAINT)),
                };
                let preview_path = attachment.path.clone();
                let download_path = attachment.path.clone();
                let action = |id, label| {
                    chrome(id)
                        .px(px(10.))
                        .py(px(4.))
                        .rounded_full()
                        .text_size(px(12.))
                        .cursor_pointer()
                        .child(label)
                };
                div()
                    .id(SharedString::from(attachment.path.clone()))
                    .px(px(10.))
                    .py(px(8.))
                    .rounded(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .hover(|row| row.bg(theme::alpha(0xffffff, 0.05)))
                    .child(
                        div()
                            .id("archive-preview-file")
                            .debug_selector(|| "archive-preview-file".into())
                            .flex()
                            .items_center()
                            .gap(px(16.))
                            .cursor_pointer()
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.emit(ArchiveEvent::PreviewFile(preview_path.clone()));
                            }))
                            .child(preview)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(4.))
                                    .child(
                                        div()
                                            .text_size(theme::LABEL)
                                            .truncate()
                                            .child(attachment.name.clone()),
                                    )
                                    .when(!attachment.detail.is_empty(), |text| {
                                        text.child(
                                            div()
                                                .text_size(px(14.))
                                                .text_color(theme::color(theme::TEXT_MUTED))
                                                .truncate()
                                                .child(attachment.detail.clone()),
                                        )
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .pl(px(74.))
                            .flex()
                            .flex_wrap()
                            .gap(px(6.))
                            .child(
                                action("archive-download-file", "Download")
                                    .debug_selector(|| "archive-download-file".into())
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ArchiveEvent::DownloadFile(download_path.clone()));
                                    })),
                            )
                            .child(
                                action("archive-show-file-in-chat", "Show in Chat")
                                    .debug_selector(|| "archive-show-file-in-chat".into())
                                    .on_click(cx.listener(move |_, _, _, cx| {
                                        cx.emit(ArchiveEvent::ShowInChat(seq));
                                    })),
                            ),
                    )
            });
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(Self::day_heading(day))
                    .child(div().px(px(10.)).flex().flex_col().children(rows)),
            );
        }
        page
    }

    /// The empty page: what will be here, or that the history is still being read.
    fn empty(&self, page: Page) -> Div {
        let (title, body): (&str, &str) = match (self.complete, page) {
            (false, _) => (
                "Reading your conversation…",
                "Pictures and files show here as they are found.",
            ),
            (true, Page::Media) => (
                "No photos yet",
                "Pictures you and your Pickle share show here.",
            ),
            (true, Page::Files) => (
                "No documents yet",
                "Documents your Pickle makes or you send show here.",
            ),
        };
        div()
            .pt(px(120.))
            .px(px(20.))
            .flex()
            .flex_col()
            .items_center()
            .gap(px(8.))
            .text_center()
            .child(
                div()
                    .text_size(theme::LABEL)
                    .font_weight(FontWeight::MEDIUM)
                    .child(title),
            )
            .child(
                div()
                    .text_size(px(14.))
                    .line_height(px(18.))
                    .text_color(theme::color(theme::TEXT_FAINT))
                    .child(body),
            )
    }

    fn switcher(&self, cx: &mut Context<Self>) -> Div {
        let segment = |page: Page, label: &'static str, cx: &mut Context<Self>| {
            div()
                .id(label)
                .debug_selector(|| format!("archive-{label}"))
                .flex_1()
                .h(px(36.))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .text_size(theme::BODY)
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .when(self.page == page, |segment| {
                    segment.bg(theme::color(theme::CHROME_SELECTED))
                })
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }))
                .child(label)
        };
        div()
            .w_full()
            .p(px(4.))
            .gap(px(2.))
            .rounded_full()
            .bg(theme::alpha(theme::CHROME, 0.94))
            .border_1()
            .border_color(theme::color(theme::HAIRLINE))
            .flex()
            .child(segment(Page::Files, "Files", cx))
            .child(segment(Page::Media, "Media", cx))
    }
}

impl Render for ArchiveView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let has_any = self
            .of_kind(match self.page {
                Page::Media => AttachmentKind::Picture,
                Page::Files => AttachmentKind::File,
            })
            .next()
            .is_some();
        let body = match (has_any, self.page) {
            (false, page) => self.empty(page),
            (true, Page::Media) => self.media(window, cx),
            (true, Page::Files) => self.files(cx),
        };
        // The title and the switch stay at the top while the page scrolls under them.
        let top = div()
            .absolute()
            .top(theme::HEADER_HEIGHT - px(10.))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .w_full()
                    .max_w(theme::COLUMN_MAX)
                    .px(px(20.))
                    .pb(px(12.))
                    .flex()
                    .flex_col()
                    .gap(px(16.))
                    .bg(theme::color(theme::PAGE))
                    .child(
                        div()
                            .text_size(px(20.))
                            .line_height(px(24.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child("Archive"),
                    )
                    .child(self.switcher(cx)),
            );
        let bar = div()
            .absolute()
            .bottom_0()
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .px(px(20.))
            .pb(px(21.))
            .child(tab_bar(
                Tab::Archive,
                cx.listener(|_, tab: &Tab, _, cx| cx.emit(ArchiveEvent::Open(*tab))),
            ));
        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .child(
                div()
                    .id("archive-scroll")
                    .size_full()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(
                        div()
                            .w_full()
                            .max_w(theme::COLUMN_MAX)
                            .pt(theme::HEADER_HEIGHT + px(98.))
                            .pb(px(120.))
                            .child(body),
                    ),
            )
            .child(top)
            .child(header_cover())
            .child(tab_header(
                self.identity.name.clone(),
                self.identity.avatar.clone(),
                cx.listener(|_, _, _, cx| cx.emit(ArchiveEvent::Menu)),
                cx.listener(|_, _, _, cx| cx.emit(ArchiveEvent::Invite)),
                true,
            ))
            .child(bar)
    }
}

impl Focusable for ArchiveView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attachment(path: &str, at: f64) -> Attachment {
        Attachment {
            path: path.into(),
            seq: 1,
            at,
            name: path.into(),
            detail: String::new(),
            kind: AttachmentKind::Picture,
        }
    }

    #[test]
    fn attachments_are_grouped_by_the_day_of_their_message() {
        let noon = |day: u32| {
            use chrono::{Local, TimeZone};
            Local
                .with_ymd_and_hms(2026, 11, day, 12, 0, 0)
                .unwrap()
                .timestamp() as f64
        };
        let list = [
            attachment("c", noon(14)),
            attachment("b", noon(14) - 3600.),
            attachment("a", noon(5)),
        ];
        let days = by_day(list.iter());
        let shape: Vec<(String, usize)> = days
            .iter()
            .map(|(day, items)| (day.clone(), items.len()))
            .collect();
        assert_eq!(shape.len(), 2);
        assert_eq!((shape[0].1, shape[1].1), (2, 1));
        assert_eq!(shape[0].0, "November 14, 2026");
    }

    #[test]
    fn archive_date_labels_include_the_current_year() {
        let now = chrono::Local::now();
        assert_eq!(
            day_label(now.timestamp() as f64),
            now.format("%B %-d, %Y").to_string(),
        );
    }
}
