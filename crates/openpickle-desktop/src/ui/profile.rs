//! The Profile tab and its read-only Usage subpage.

use std::time::Duration;

use chrono::{DateTime, Utc};
use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString,
    Stateful, Subscription, Task, Window, div, prelude::*, px, relative,
};

use crate::{
    profile::{ProfileData, TicketCounts, TicketHistoryItem, UsageSnapshot},
    theme,
    ui::{Cancel, Identity, Pictures, Tab, chrome, header_cover, icon, tab_bar, tab_header},
};

const PRIVACY_URL: &str = "https://pickle.com/privacy?view=app";
const HELP_URL: &str = "https://pickle.com/help?view=app";
const CONTACT_URL: &str = "mailto:contact@pickle.com";

pub enum ProfileEvent {
    Menu,
    Open(Tab),
    Invite,
    Reload,
    Link(String),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Page {
    #[default]
    Profile,
    Usage,
}

pub struct ProfileView {
    identity: Identity,
    _pictures: Entity<Pictures>,
    profile: Option<ProfileData>,
    failed: Option<SharedString>,
    page: Page,
    focus: FocusHandle,
    _countdown_refresh: Task<()>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ProfileEvent> for ProfileView {}

impl ProfileView {
    pub fn new(identity: Identity, pictures: Entity<Pictures>, cx: &mut Context<Self>) -> Self {
        let subscriptions = vec![cx.observe(&pictures, |_, _, cx| cx.notify())];
        let countdown_refresh = cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_secs(60))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            identity,
            _pictures: pictures,
            profile: None,
            failed: None,
            page: Page::Profile,
            focus: cx.focus_handle(),
            _countdown_refresh: countdown_refresh,
            _subscriptions: subscriptions,
        }
    }

    pub fn set_identity(&mut self, identity: Identity, cx: &mut Context<Self>) {
        self.identity = identity;
        cx.notify();
    }

    /// Takes a cache-safe profile snapshot. A failed refresh keeps the last successful snapshot.
    pub fn set_profile(&mut self, profile: Result<ProfileData, String>, cx: &mut Context<Self>) {
        match profile {
            Ok(profile) => {
                self.profile = Some(profile);
                self.failed = None;
            }
            Err(reason) => self.failed = Some(reason.into()),
        }
        cx.notify();
    }

    fn root(&self, cx: &mut Context<Self>) -> Div {
        let mut content = div()
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .px(px(20.))
            .pt(theme::HEADER_HEIGHT - px(10.))
            .pb(px(132.))
            .flex()
            .flex_col()
            .gap(px(16.))
            .child(section_title("My Pickle"));

        match &self.profile {
            None => {
                content = content.child(match &self.failed {
                    Some(reason) => status_card(reason, true, cx),
                    None => status_card("Loading your profile…", false, cx),
                });
            }
            Some(profile) => {
                if let Some(reason) = &self.failed {
                    content = content.child(status_card(reason, true, cx));
                }
                content = content
                    .child(self.usage_summary(profile.usage.as_ref(), cx))
                    .child(self.link_card(
                        "privacy-card",
                        "shield-keyhole",
                        "How we handle your data",
                        "Learn how we keep your information private and secure.",
                        PRIVACY_URL,
                        cx,
                    ))
                    .child(account_card(&profile.display_name))
                    .child(connection_card(profile))
                    .child(settings_group(
                        self.identity.name.clone(),
                        &[("wallet", "Payment methods"), ("lock-keyhole", "Privacy")],
                    ))
                    .child(settings_group(
                        "App".into(),
                        &[
                            ("bell", "Notifications"),
                            ("paintbrush", "Appearance"),
                            ("house", "Home Screen widget"),
                            ("languages", "Language"),
                        ],
                    ))
                    .child(self.support_group(cx))
                    .child(
                        div()
                            .w_full()
                            .pt(px(12.))
                            .text_align(gpui::TextAlign::Center)
                            .text_size(px(14.))
                            .text_color(theme::color(theme::TEXT_FAINT))
                            .child(format!("App version: {}", env!("CARGO_PKG_VERSION"))),
                    );
            }
        }
        content
    }

    fn usage_summary(
        &self,
        usage: Option<&UsageSnapshot>,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let (title, percent, reset, tickets) = match usage {
            Some(usage) => (
                usage_title(usage),
                usage.used_percent(),
                usage.reset_countdown(Utc::now()),
                usage.tickets.clone(),
            ),
            None => ("Usage limit", None, None, TicketCounts::default()),
        };
        let percent_label = percent
            .map(|percent| format!("{percent}% used"))
            .unwrap_or_else(|| "Unavailable".to_owned());
        let reset = reset
            .map(|duration| format!("Resets in {duration}"))
            .unwrap_or_else(|| "Reset time unavailable".to_owned());
        let full = count_text(tickets.full);
        let half = count_text(tickets.half);

        card()
            .id("open-usage")
            .cursor_pointer()
            .hover(|card| card.bg(theme::alpha(0xffffff, 0.065)))
            .on_click(cx.listener(|this, _, _, cx| {
                this.page = Page::Usage;
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(label(title))
                    .child(mutual(percent_label)),
            )
            .child(progress(percent))
            .child(divider())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(7.))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(14.))
                                    .text_color(theme::color(theme::TEXT_MUTED))
                                    .child(reset),
                            )
                            .child(
                                div()
                                    .text_size(theme::LABEL)
                                    .text_color(theme::color(theme::LINK))
                                    .child("Reset tickets"),
                            ),
                    )
                    .child(ticket_holding(theme::LINK, full))
                    .child(ticket_holding(0xc547ef, half))
                    .child(icon("chevron-right", 18., theme::TEXT_FAINT)),
            )
    }

    fn link_card(
        &self,
        id: &'static str,
        icon_name: &'static str,
        title: &'static str,
        subtitle: &'static str,
        url: &'static str,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        card()
            .id(id)
            .cursor_pointer()
            .hover(|card| card.bg(theme::alpha(0xffffff, 0.065)))
            .on_click(cx.listener(move |_, _, _, cx| cx.emit(ProfileEvent::Link(url.to_owned()))))
            .flex()
            .flex_row()
            .items_center()
            .gap(px(12.))
            .child(icon(icon_name, 20., theme::TEXT))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(3.))
                    .child(label(title))
                    .child(mutual(subtitle)),
            )
            .child(icon("chevron-right", 18., theme::TEXT_FAINT))
    }

    fn support_group(&self, cx: &mut Context<Self>) -> Div {
        let rows = card()
            .child(active_row("help", "circle-help", "Help", HELP_URL, cx))
            .child(divider().ml(px(42.)))
            .child(disabled_row("bug", "Report an issue"))
            .child(divider().ml(px(42.)))
            .child(active_row(
                "contact",
                "mail",
                "Contact the team",
                CONTACT_URL,
                cx,
            ))
            .child(divider().ml(px(42.)))
            .child(disabled_row("shield-check", "Legal info"));
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(group_title("Support"))
            .child(rows)
    }

    fn usage_page(&self, cx: &mut Context<Self>) -> Div {
        let usage = self
            .profile
            .as_ref()
            .and_then(|profile| profile.usage.as_ref());
        let history = self
            .profile
            .as_ref()
            .and_then(|profile| profile.ticket_history.as_ref());
        let tickets = usage.map(|usage| usage.tickets.clone()).unwrap_or_default();
        let mut content = div()
            .w_full()
            .max_w(theme::COLUMN_MAX)
            .px(px(20.))
            .pt(px(112.))
            .pb(px(54.))
            .flex()
            .flex_col()
            .gap(px(28.))
            .child(usage_limit_card(usage))
            .child(reset_ticket_card(&tickets))
            .child(self.invite_reward(cx))
            .child(auto_use_card(usage));

        content = content.child(history_section(history));
        content
    }

    fn invite_reward(&self, cx: &mut Context<Self>) -> Div {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(group_title("Earn reset tickets"))
            .child(
                card()
                    .id("invite-reward")
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(12.))
                    .cursor_pointer()
                    .hover(|card| card.bg(theme::alpha(0xffffff, 0.065)))
                    .on_click(cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::Invite)))
                    .child(icon("users-round", 20., theme::TEXT))
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(label("Invite a friend"))
                            .child(mutual("Share your invite link")),
                    )
                    .child(icon("chevron-right", 18., theme::TEXT_FAINT)),
            )
    }
}

impl Render for ProfileView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let screen = div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .on_action(cx.listener(|this, _: &Cancel, _, cx| {
                if this.page == Page::Usage {
                    this.page = Page::Profile;
                    cx.notify();
                }
            }))
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE);

        match self.page {
            Page::Profile => {
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
                        Tab::Profile,
                        cx.listener(|_, tab: &Tab, _, cx| cx.emit(ProfileEvent::Open(*tab))),
                    ));
                screen
                    .child(
                        div()
                            .id("profile-scroll")
                            .size_full()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .items_center()
                            .child(self.root(cx)),
                    )
                    .child(header_cover())
                    .child(tab_header(
                        self.identity.name.clone(),
                        self.identity.avatar.clone(),
                        cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::Menu)),
                        cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::Invite)),
                        false,
                    ))
                    .child(bar)
            }
            Page::Usage => screen
                .child(
                    div()
                        .id("usage-scroll")
                        .size_full()
                        .overflow_y_scroll()
                        .flex()
                        .flex_col()
                        .items_center()
                        .child(self.usage_page(cx)),
                )
                .child(subpage_header(cx)),
        }
    }
}

impl Focusable for ProfileView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

fn subpage_header(cx: &mut Context<ProfileView>) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .right_0()
        .h(px(94.))
        .occlude()
        .bg(theme::color(theme::PAGE))
        .px(px(20.))
        .pt(px(44.))
        .flex()
        .items_center()
        .justify_center()
        .child(
            chrome("profile-back")
                .absolute()
                .left(px(20.))
                .size(px(43.))
                .rounded_full()
                .cursor_pointer()
                .on_click(cx.listener(|this, _, _, cx| {
                    this.page = Page::Profile;
                    cx.notify();
                }))
                .child(icon("chevron-left", 21., theme::TEXT)),
        )
        .child(
            div()
                .text_size(px(20.))
                .font_weight(FontWeight::SEMIBOLD)
                .child("Usage"),
        )
}

fn usage_title(usage: &UsageSnapshot) -> &'static str {
    match usage.period.as_deref() {
        Some("day") => "Daily usage limit",
        Some("month") => "Monthly usage limit",
        Some("year") => "Yearly usage limit",
        Some("week") => "Weekly usage limit",
        _ => "Usage limit",
    }
}

fn usage_limit_card(usage: Option<&UsageSnapshot>) -> Div {
    let title = usage.map(usage_title).unwrap_or("Usage limit");
    let percent = usage.and_then(UsageSnapshot::used_percent);
    let percent_label = percent
        .map(|percent| format!("{percent}% used"))
        .unwrap_or_else(|| "Unavailable".to_owned());
    let reset = usage
        .and_then(|usage| usage.reset_countdown(Utc::now()))
        .map(|duration| format!("{duration} until the limit resets"))
        .unwrap_or_else(|| "Reset time unavailable".to_owned());
    card()
        .child(
            div()
                .flex()
                .items_center()
                .justify_between()
                .child(label(title))
                .child(mutual(percent_label)),
        )
        .child(progress(percent))
        .child(
            div()
                .text_size(px(14.))
                .text_color(theme::color(theme::TEXT_MUTED))
                .child(reset),
        )
}

fn reset_ticket_card(tickets: &TicketCounts) -> Div {
    card()
        .child(
            div()
                .flex()
                .items_center()
                .child(label("Reset tickets"))
                .child(div().flex_1())
                .child(ticket_holding(theme::LINK, count_text(tickets.full)))
                .child(ticket_holding(0xc547ef, count_text(tickets.half))),
        )
        .child(divider())
        .child(ticket_action(
            theme::LINK,
            "Full reset ticket",
            "Resets usage to 0%",
            tickets.full,
        ))
        .child(ticket_action(
            0xc547ef,
            "Half reset ticket",
            "Lowers usage by 50%",
            tickets.half,
        ))
        .child(divider())
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .flex_1()
                        .text_color(theme::color(theme::TEXT_MUTED))
                        .child("Buy tickets"),
                )
                .child(availability()),
        )
}

fn ticket_action(
    color: u32,
    title: &'static str,
    subtitle: &'static str,
    count: Option<i64>,
) -> Div {
    div()
        .py(px(3.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(icon("ticket", 17., color))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(label(title))
                .child(mutual(subtitle)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(2.))
                .child(mutual(format!("{} available", count_text(count))))
                .child(availability()),
        )
}

fn auto_use_card(usage: Option<&UsageSnapshot>) -> Div {
    let auto = usage
        .map(|usage| if usage.ticket_auto_use { "On" } else { "Off" })
        .unwrap_or("—");
    let order = usage
        .and_then(|usage| usage.ticket_order.as_deref())
        .map(|order| match order {
            "half_first" => "Half tickets first",
            "full_first" => "Full tickets first",
            _ => "Server preference",
        })
        .unwrap_or("—");
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(group_title("Auto-use settings"))
        .child(
            card()
                .child(readonly_value("Use reset tickets automatically", auto))
                .child(divider())
                .child(readonly_value("Ticket order", order))
                .child(
                    div()
                        .pt(px(5.))
                        .text_size(px(12.))
                        .text_color(theme::color(theme::TEXT_FAINT))
                        .child("Changes are not available on desktop yet."),
                ),
        )
}

fn history_section(history: Option<&Vec<TicketHistoryItem>>) -> Div {
    let body = match history {
        None => card().child(mutual("Ticket history is unavailable.")),
        Some(items) if items.is_empty() => card().child(mutual("No ticket history yet.")),
        Some(items) => {
            let mut list = card().gap(px(0.));
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    list = list.child(divider());
                }
                list = list.child(history_row(item));
            }
            list
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(group_title("Ticket history"))
        .child(body)
}

fn history_row(item: &TicketHistoryItem) -> Div {
    let when = item
        .granted_text
        .clone()
        .or_else(|| item.granted_at_epoch_ms.map(format_epoch))
        .unwrap_or_else(|| "Date unavailable".to_owned());
    let color = match item.ticket_fraction_pct {
        Some(100) => theme::LINK,
        Some(_) => 0xc547ef,
        None => theme::TEXT_FAINT,
    };
    div()
        .py(px(5.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(label(item.label.clone()))
                .child(mutual(when)),
        )
        .child(icon("ticket", 17., color))
        .child(item.count.to_string())
}

fn format_epoch(epoch_ms: i64) -> String {
    DateTime::from_timestamp_millis(epoch_ms)
        .map(|date| {
            date.with_timezone(&chrono::Local)
                .format("%b %-d, %Y %-I:%M %p")
                .to_string()
        })
        .unwrap_or_else(|| "Date unavailable".to_owned())
}

fn active_row(
    id: &'static str,
    icon_name: &'static str,
    title: &'static str,
    url: &'static str,
    cx: &mut Context<ProfileView>,
) -> Stateful<Div> {
    div()
        .id(id)
        .py(px(7.))
        .flex()
        .items_center()
        .gap(px(12.))
        .cursor_pointer()
        .on_click(cx.listener(move |_, _, _, cx| cx.emit(ProfileEvent::Link(url.to_owned()))))
        .child(icon(icon_name, 19., theme::TEXT))
        .child(div().flex_1().child(label(title)))
        .child(icon("chevron-right", 18., theme::TEXT_FAINT))
}

fn disabled_row(icon_name: &'static str, title: &'static str) -> Div {
    div()
        .py(px(7.))
        .flex()
        .items_center()
        .gap(px(12.))
        .child(icon(icon_name, 19., theme::TEXT_MUTED))
        .child(div().flex_1().child(label(title)))
        .child(availability())
}

fn settings_group(title: SharedString, rows: &[(&'static str, &'static str)]) -> Div {
    let mut body = card();
    for (index, &(icon_name, label)) in rows.iter().enumerate() {
        if index > 0 {
            body = body.child(divider().ml(px(42.)));
        }
        body = body.child(disabled_row(icon_name, label));
    }
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(group_title(title))
        .child(body)
}

fn account_card(display_name: &str) -> Div {
    card()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(12.))
        .child(icon("user-round", 21., theme::TEXT_MUTED))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(label(if display_name.trim().is_empty() {
                    "Account name unavailable".to_owned()
                } else {
                    display_name.to_owned()
                }))
                .child(mutual("My account · Editing unavailable on desktop")),
        )
}

fn connection_card(profile: &ProfileData) -> Div {
    let (count, enabled) = profile
        .connections
        .as_ref()
        .map(|connections| {
            (
                connections
                    .enabled_count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "—".to_owned()),
                connections.enabled.as_slice(),
            )
        })
        .unwrap_or_else(|| ("—".to_owned(), &[]));
    let mut tiles = div().flex().items_center().gap(px(6.));
    for connection in enabled.iter().take(3) {
        let initial = connection
            .initial
            .clone()
            .or_else(|| connection.label.chars().next().map(|ch| ch.to_string()))
            .unwrap_or_else(|| "·".to_owned());
        tiles = tiles.child(
            div()
                .size(px(36.))
                .rounded(px(8.))
                .bg(theme::color(theme::SURFACE))
                .flex()
                .items_center()
                .justify_center()
                .font_weight(FontWeight::SEMIBOLD)
                .child(initial),
        );
    }
    tiles = tiles.child(
        div()
            .size(px(36.))
            .rounded(px(8.))
            .bg(theme::color(theme::SURFACE))
            .flex()
            .items_center()
            .justify_center()
            .child(icon("plus", 18., theme::TEXT_FAINT)),
    );
    card()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(label("Connections"))
                .child(mutual(count)),
        )
        .child(tiles)
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme::color(theme::TEXT_FAINT))
                .child("Connection management is not available on desktop yet."),
        )
}

fn status_card(reason: &str, retry: bool, cx: &mut Context<ProfileView>) -> Div {
    card().child(mutual(reason.to_owned())).when(retry, |card| {
        card.child(
            div()
                .id("reload-profile")
                .text_color(theme::color(theme::LINK))
                .cursor_pointer()
                .on_click(cx.listener(|_, _, _, cx| cx.emit(ProfileEvent::Reload)))
                .child("Retry"),
        )
    })
}

fn card() -> Div {
    div()
        .w_full()
        .rounded(px(24.))
        .bg(theme::color(theme::CARD))
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(12.))
}

fn section_title(title: &'static str) -> Div {
    div()
        .text_size(px(20.))
        .line_height(px(26.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(title)
}

fn group_title(title: impl Into<SharedString>) -> Div {
    div()
        .px(px(20.))
        .text_size(px(14.))
        .text_color(theme::color(theme::TEXT_FAINT))
        .child(title.into())
}

fn label(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(theme::LABEL)
        .line_height(theme::LABEL_LINE)
        .font_weight(FontWeight::MEDIUM)
        .child(text.into())
}

fn mutual(text: impl Into<SharedString>) -> Div {
    div()
        .text_size(px(14.))
        .line_height(px(18.))
        .text_color(theme::color(theme::TEXT_MUTED))
        .child(text.into())
}

fn divider() -> Div {
    div().h(px(1.)).bg(theme::color(theme::HAIRLINE))
}

fn progress(percent: Option<u8>) -> Div {
    let fraction = f32::from(percent.unwrap_or(0)) / 100.;
    div()
        .w_full()
        .h(px(6.))
        .rounded_full()
        .overflow_hidden()
        .bg(theme::alpha(0xffffff, 0.08))
        .child(
            div()
                .h_full()
                .w(relative(fraction))
                .rounded_full()
                .bg(theme::color(theme::TEXT)),
        )
}

fn ticket_holding(color: u32, count: String) -> Div {
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .child(icon("ticket", 16., color))
        .child(count)
}

fn count_text(count: Option<i64>) -> String {
    count
        .map(|count| count.to_string())
        .unwrap_or_else(|| "—".to_owned())
}

fn availability() -> Div {
    div()
        .text_size(px(11.))
        .text_color(theme::color(theme::TEXT_FAINT))
        .child("Not available")
}

fn readonly_value(label_text: &'static str, value: &'static str) -> Div {
    div()
        .py(px(2.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(div().flex_1().child(label(label_text)))
        .child(mutual(value))
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc};

    use gpui::{
        Modifiers, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext,
        point, size,
    };

    use super::*;

    fn view(cx: &mut TestAppContext) -> (Entity<ProfileView>, &mut VisualTestContext) {
        let (view, cx) = cx.add_window_view(|_, cx| {
            let pictures = cx.new(|_| Pictures::default());
            ProfileView::new(Identity::default(), pictures, cx)
        });
        view.update(cx, |view, cx| {
            view.set_profile(Ok(ProfileData::default()), cx)
        });
        cx.simulate_resize(size(px(440.), px(900.)));
        cx.run_until_parked();
        (view, cx)
    }

    #[gpui::test]
    fn usage_card_opens_and_back_returns_to_profile(cx: &mut TestAppContext) {
        let (view, cx) = view(cx);
        cx.simulate_click(point(px(220.), px(240.)), Modifiers::none());
        view.read_with(cx, |view, _| assert_eq!(view.page, Page::Usage));

        cx.simulate_click(point(px(42.), px(65.)), Modifiers::none());
        view.read_with(cx, |view, _| assert_eq!(view.page, Page::Profile));
    }

    #[gpui::test]
    fn profile_scroll_reaches_the_active_contact_row(cx: &mut TestAppContext) {
        let (view, cx) = view(cx);
        let links = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            let links = links.clone();
            cx.subscribe(&view, move |_, event, _| {
                if let ProfileEvent::Link(target) = event {
                    links.borrow_mut().push(target.clone());
                }
            })
            .detach();
        });

        let contact_position = point(px(220.), px(620.));
        cx.simulate_click(contact_position, Modifiers::none());
        assert!(
            links.borrow().is_empty(),
            "contact begins below the viewport"
        );

        cx.simulate_event(ScrollWheelEvent {
            position: point(px(220.), px(450.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-5000.))),
            modifiers: Modifiers::none(),
            touch_phase: TouchPhase::Moved,
        });
        cx.run_until_parked();
        cx.simulate_click(contact_position, Modifiers::none());
        assert_eq!(&*links.borrow(), &[CONTACT_URL.to_owned()]);
    }
}
