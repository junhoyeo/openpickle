//! Sign-in with SMS or browser-based Kakao OAuth.

use gpui::{
    App, Context, Div, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, SharedString,
    Stateful, Subscription, Window, div, prelude::*, px,
};

use crate::{
    theme,
    ui::input::{InputEvent, TextInput},
};

pub enum LoginEvent {
    StartKakao(u64),
    CancelKakao,
    /// The user asked for a code to be texted to this number (E.164).
    RequestCode(String),
    /// The user typed the code they received.
    VerifyCode(String),
}

enum Step {
    Kakao,
    Phone,
    /// Waiting for the code texted to this number.
    Code(SharedString),
}

pub struct LoginView {
    step: Step,
    phone: Entity<TextInput>,
    code: Entity<TextInput>,
    busy: bool,
    kakao_attempt: u64,
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
            InputEvent::Selected => {}
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
            kakao_attempt: 0,
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
        if matches!(self.step, Step::Kakao) {
            self.step = Step::Phone;
        }
        self.busy = false;
        self.error = Some(message.into());
        cx.notify();
    }

    pub fn is_kakao_attempt(&self, attempt: u64) -> bool {
        matches!(self.step, Step::Kakao) && self.kakao_attempt == attempt
    }

    fn start_kakao(&mut self, cx: &mut Context<Self>) {
        if self.busy || !matches!(self.step, Step::Phone) {
            return;
        }
        self.kakao_attempt += 1;
        self.step = Step::Kakao;
        self.busy = true;
        self.error = None;
        cx.emit(LoginEvent::StartKakao(self.kakao_attempt));
        cx.notify();
    }

    fn submit(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        match &self.step {
            Step::Kakao => return,
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
        if matches!(self.step, Step::Kakao) {
            cx.emit(LoginEvent::CancelKakao);
        }
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
            Step::Phone | Step::Kakao => self.phone.focus_handle(cx),
            Step::Code(_) => self.code.focus_handle(cx),
        }
    }
}

impl Render for LoginView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let waiting = matches!(self.step, Step::Kakao);
        let (prompt, input, action): (SharedString, _, _) = match &self.step {
            Step::Kakao => (
                "Complete sign-in in your browser…".into(),
                field("phone", &self.phone),
                "Waiting for Kakao…",
            ),
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
            .debug_selector(|| "submit".into())
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
            .when(!waiting, |form| form.child(input).child(button));
        if matches!(self.step, Step::Phone) {
            form = form.child(
                div()
                    .id("kakao")
                    .debug_selector(|| "kakao".into())
                    .w_full()
                    .h(px(50.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded_full()
                    .bg(theme::color(theme::CHROME))
                    .border_1()
                    .border_color(theme::color(theme::HAIRLINE))
                    .font_weight(FontWeight::MEDIUM)
                    .when(self.busy, |button| button.opacity(0.5))
                    .when(!self.busy, |button| button.cursor_pointer())
                    .child("Continue with Kakao")
                    .on_click(cx.listener(|this, _, _, cx| this.start_kakao(cx))),
            );
        }
        if let Some(error) = &self.error {
            form = form.child(
                div()
                    .text_color(theme::color(theme::DANGER))
                    .child(error.clone()),
            );
        }
        if matches!(self.step, Step::Code(_) | Step::Kakao) {
            form = form.child(
                div()
                    .id("another")
                    .debug_selector(|| "another".into())
                    .pt(px(4.))
                    .cursor_pointer()
                    .text_color(theme::color(theme::LINK))
                    .child(if waiting {
                        "Cancel"
                    } else {
                        "Use another number"
                    })
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
            .font_family(theme::FONT)
            .text_size(theme::BODY)
            .line_height(theme::BODY_LINE)
            .child(form)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn kakao_cancel_and_failure_leave_sms_usable(cx: &mut gpui::TestAppContext) {
        use std::{cell::RefCell, rc::Rc};
        let (view, cx) = cx.add_window_view(|window, cx| {
            window.activate_window();
            LoginView::new(cx)
        });
        cx.simulate_resize(gpui::size(px(440.), px(700.)));
        let events = Rc::new(RefCell::new(Vec::new()));
        let observed = events.clone();
        cx.update(|_, cx| {
            cx.subscribe(&view, move |_, event, _| {
                observed.borrow_mut().push(match event {
                    LoginEvent::StartKakao(_) => "kakao".to_owned(),
                    LoginEvent::CancelKakao => "cancel".to_owned(),
                    LoginEvent::RequestCode(phone) => phone.clone(),
                    LoginEvent::VerifyCode(code) => code.clone(),
                });
            })
            .detach();
        });
        let click = |cx: &mut gpui::VisualTestContext, selector| {
            cx.update(|window, cx| window.draw(cx).clear());
            let bounds = cx.debug_bounds(selector).expect(selector);
            cx.simulate_click(bounds.center(), gpui::Modifiers::none());
        };
        click(cx, "kakao");
        cx.update(|window, cx| window.draw(cx).clear());
        view.read_with(cx, |view, _| assert!(view.is_kakao_attempt(1)));
        click(cx, "another");
        click(cx, "kakao");
        view.update(cx, |view, cx| {
            assert!(!view.is_kakao_attempt(1));
            assert!(view.is_kakao_attempt(2));
            view.failed("Denied", cx);
            view.phone
                .update(cx, |input, cx| input.set_text("010-1234-5678", cx));
        });
        click(cx, "submit");
        cx.update(|window, cx| {
            view.update(cx, |view, cx| {
                view.code_requested(window, cx);
                view.code
                    .update(cx, |input, cx| input.set_text("123456", cx));
            });
        });
        click(cx, "submit");
        assert_eq!(
            &*events.borrow(),
            &["kakao", "cancel", "kakao", "+821012345678", "123456"]
        );
    }

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
