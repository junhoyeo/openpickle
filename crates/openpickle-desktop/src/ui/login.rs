//! Sign-in: a phone number, then the code Pickle texts to it.

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString,
    Stateful, Subscription, Window, div, prelude::*, px,
};

use crate::{
    theme,
    ui::input::{InputEvent, TextInput},
};

pub enum LoginEvent {
    /// The user asked for a code to be texted to this number (E.164).
    RequestCode(String),
    /// The user typed the code they received.
    VerifyCode(String),
}

enum Step {
    Phone,
    /// Waiting for the code texted to this number.
    Code(SharedString),
}

pub struct LoginView {
    step: Step,
    phone: Entity<TextInput>,
    code: Entity<TextInput>,
    busy: bool,
    error: Option<SharedString>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<LoginEvent> for LoginView {}

impl LoginView {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let phone = cx.new(|cx| TextInput::new("010-1234-5678", cx));
        let code = cx.new(|cx| TextInput::new("6-digit code", cx));
        let on_input = |this: &mut Self,
                        _: Entity<TextInput>,
                        event: &InputEvent,
                        cx: &mut Context<Self>| match event {
            InputEvent::Submitted(_) => this.submit(cx),
            InputEvent::Changed => {
                this.error = None;
                cx.notify();
            }
        };
        let subscriptions = vec![
            cx.subscribe(&phone, on_input),
            cx.subscribe(&code, on_input),
        ];
        Self {
            step: Step::Phone,
            phone,
            code,
            busy: false,
            error: None,
            _subscriptions: subscriptions,
        }
    }

    /// The code was texted; ask for it.
    pub fn code_requested(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(phone) = normalize_phone(self.phone.read(cx).text()) else {
            return;
        };
        self.step = Step::Code(phone.into());
        self.busy = false;
        self.error = None;
        window.focus(&self.code.focus_handle(cx));
        cx.notify();
    }

    /// The last request failed; show why and let the user try again.
    pub fn failed(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.busy = false;
        self.error = Some(message.into());
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match &self.step {
            Step::Phone => match normalize_phone(self.phone.read(cx).text()) {
                Some(phone) => {
                    self.busy = true;
                    self.error = None;
                    cx.emit(LoginEvent::RequestCode(phone));
                }
                None => {
                    self.error = Some("Enter a mobile number, for example 010-1234-5678.".into())
                }
            },
            Step::Code(_) => {
                let code: String = self
                    .code
                    .read(cx)
                    .text()
                    .chars()
                    .filter(char::is_ascii_digit)
                    .collect();
                if code.len() == 6 {
                    self.busy = true;
                    self.error = None;
                    cx.emit(LoginEvent::VerifyCode(code));
                } else {
                    self.error = Some("The code has 6 digits.".into());
                }
            }
        }
        cx.notify();
    }

    fn use_another_number(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.step = Step::Phone;
        self.busy = false;
        self.error = None;
        self.code.update(cx, |code, cx| code.set_text("", cx));
        window.focus(&self.phone.focus_handle(cx));
        cx.notify();
    }
}

/// Turns what people type into E.164. A number without a country code is taken as Korean.
pub fn normalize_phone(input: &str) -> Option<String> {
    let international = input.trim_start().starts_with('+');
    let digits: String = input.chars().filter(char::is_ascii_digit).collect();
    let number = if international {
        digits
    } else {
        format!("82{}", digits.strip_prefix('0')?)
    };
    (8..=15)
        .contains(&number.len())
        .then(|| format!("+{number}"))
}

fn field(id: &'static str, input: &Entity<TextInput>) -> Stateful<Div> {
    div()
        .id(id)
        .w_full()
        .h(px(53.))
        .px(px(20.))
        .flex()
        .items_center()
        .rounded_full()
        .bg(theme::alpha(theme::CHROME, 0.94))
        .border_1()
        .border_color(theme::color(theme::HAIRLINE))
        .child(input.clone())
}

impl Focusable for LoginView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match self.step {
            Step::Phone => self.phone.focus_handle(cx),
            Step::Code(_) => self.code.focus_handle(cx),
        }
    }
}

impl Render for LoginView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (prompt, input, action): (SharedString, _, _) = match &self.step {
            Step::Phone => (
                "Sign in with your phone number".into(),
                field("phone", &self.phone),
                "Text me a code",
            ),
            Step::Code(phone) => (
                format!("Enter the code sent to {phone}").into(),
                field("code", &self.code),
                "Sign in",
            ),
        };
        let button = div()
            .id("submit")
            .w_full()
            .h(px(50.))
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(theme::color(theme::TEXT))
            .text_color(theme::color(theme::CHROME))
            .font_weight(FontWeight::MEDIUM)
            .when(self.busy, |button| button.opacity(0.5))
            .when(!self.busy, |button| button.cursor_pointer())
            .child(if self.busy { "One moment…" } else { action })
            .on_click(cx.listener(|this, _, _, cx| this.submit(cx)));

        let mut form = div()
            .w_full()
            .max_w(px(340.))
            .px(px(24.))
            .flex()
            .flex_col()
            .gap(px(12.))
            .child(
                div()
                    .pb(px(4.))
                    .text_size(px(28.))
                    .line_height(px(34.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child("Pickle"),
            )
            .child(
                div()
                    .pb(px(8.))
                    .text_color(theme::color(theme::TEXT_MUTED))
                    .child(prompt),
            )
            .child(input)
            .child(button);
        if let Some(error) = &self.error {
            form = form.child(
                div()
                    .text_color(theme::color(theme::DANGER))
                    .child(error.clone()),
            );
        }
        if matches!(self.step, Step::Code(_)) {
            form = form.child(
                div()
                    .id("another")
                    .pt(px(4.))
                    .cursor_pointer()
                    .text_color(theme::color(theme::LINK))
                    .child("Use another number")
                    .on_click(
                        cx.listener(|this, _, window, cx| this.use_another_number(window, cx)),
                    ),
            );
        }
        div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme::color(theme::PAGE))
            .text_color(theme::color(theme::TEXT))
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .child(form)
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_phone;

    #[test]
    fn local_numbers_become_korean_e164() {
        assert_eq!(
            normalize_phone("010-1234-5678").as_deref(),
            Some("+821012345678")
        );
        assert_eq!(
            normalize_phone(" +1 (415) 555-0132").as_deref(),
            Some("+14155550132")
        );
        assert_eq!(normalize_phone("1234"), None);
        assert_eq!(normalize_phone("12345678"), None);
    }
}
