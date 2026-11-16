//! The root view: shows sign-in or the chat, and applies what the backend reports.

use chrono::Local;
use futures::{StreamExt, channel::mpsc::UnboundedReceiver};
use gpui::{
    Context, Entity, Focusable, FontWeight, ImageSource, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    model::{Message, build_rows},
    protocol::{Command, Update},
    theme,
    ui::{
        chat::{ChatEvent, ChatView},
        login::{LoginEvent, LoginView},
    },
};

enum Screen {
    /// Nothing is known yet; the backend answers within the first frames.
    Starting,
    Login(Entity<LoginView>),
    Chat(Entity<ChatView>),
    /// The backend stopped; the text says why.
    Failed(SharedString),
}

pub struct Shell {
    commands: UnboundedSender<Command>,
    screen: Screen,
    /// The open conversation, oldest first.
    messages: Vec<Message>,
    activity: Option<Option<String>>,
    _screen_events: Option<Subscription>,
    _activation: Subscription,
    _updates: Task<()>,
}

impl Shell {
    pub fn new(
        commands: UnboundedSender<Command>,
        mut updates: UnboundedReceiver<Update>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let activation = cx.observe_window_activation(window, |this, window, _| {
            this.commands
                .send(Command::Active(window.is_window_active()))
                .ok();
        });
        let mut shell = Self {
            commands,
            screen: Screen::Starting,
            messages: Vec::new(),
            activity: None,
            _screen_events: None,
            _activation: activation,
            _updates: Task::ready(()),
        };
        // The backend started before the window did, so the cached conversation is usually
        // waiting already. Applying it here puts it in the very first frame.
        while let Ok(update) = updates.try_recv() {
            shell.apply(update, window, cx);
        }
        shell._updates = cx.spawn_in(window, async move |this, cx| {
            while let Some(update) = updates.next().await {
                let applied = this.update_in(cx, |this, window, cx| this.apply(update, window, cx));
                if applied.is_err() {
                    break;
                }
            }
        });
        shell
    }

    fn apply(&mut self, update: Update, window: &mut Window, cx: &mut Context<Self>) {
        match update {
            Update::SignedOut => self.show_login(window, cx),
            Update::CodeSent => {
                if let Screen::Login(login) = &self.screen {
                    login.update(cx, |login, cx| login.code_requested(window, cx));
                }
            }
            Update::SignInFailed(reason) => {
                if let Screen::Login(login) = &self.screen {
                    login.update(cx, |login, cx| login.failed(reason, cx));
                }
            }
            Update::Conversation {
                name,
                avatar,
                messages,
            } => {
                self.messages = messages;
                self.activity = None;
                let chat = self.show_chat(window, cx);
                chat.update(cx, |chat, cx| {
                    chat.set_identity(name, avatar.map(ImageSource::from), cx)
                });
                self.refresh_rows(cx);
            }
            Update::Identity { name, avatar } => {
                if let Screen::Chat(chat) = &self.screen {
                    chat.update(cx, |chat, cx| {
                        chat.set_identity(name, avatar.map(ImageSource::from), cx)
                    });
                }
            }
            Update::Upsert(messages) => {
                for message in messages {
                    upsert(&mut self.messages, message);
                }
                self.refresh_rows(cx);
            }
            Update::Activity(activity) => {
                self.activity = activity;
                self.refresh_rows(cx);
            }
            Update::Failed(reason) => {
                self._screen_events = None;
                self.screen = Screen::Failed(reason.into());
                cx.notify();
            }
        }
    }

    fn show_login(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let login = cx.new(LoginView::new);
        self._screen_events = Some(cx.subscribe(&login, |this, _, event, _| {
            let command = match event {
                LoginEvent::RequestCode(phone) => Command::RequestCode(phone.clone()),
                LoginEvent::VerifyCode(code) => Command::VerifyCode(code.clone()),
            };
            this.commands.send(command).ok();
        }));
        window.focus(&login.focus_handle(cx));
        self.screen = Screen::Login(login);
        cx.notify();
    }

    fn show_chat(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Entity<ChatView> {
        if let Screen::Chat(chat) = &self.screen {
            return chat.clone();
        }
        let chat = cx.new(ChatView::new);
        self._screen_events = Some(cx.subscribe(&chat, |this, _, ChatEvent::Send(text), _| {
            this.commands.send(Command::Send(text.clone())).ok();
        }));
        window.focus(&chat.focus_handle(cx));
        self.screen = Screen::Chat(chat.clone());
        cx.notify();
        chat
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        if let Screen::Chat(chat) = &self.screen {
            let rows = build_rows(
                &self.messages,
                self.activity.as_ref().map(|label| label.as_deref()),
                Local::now(),
            );
            chat.update(cx, |chat, cx| chat.set_rows(rows, cx));
        }
    }
}

/// Replaces the message with the same id, or inserts a new one, keeping the list ordered by
/// sequence number. Messages still being sent carry the highest numbers, so they stay last.
fn upsert(messages: &mut Vec<Message>, message: Message) {
    if let Some(index) = messages.iter().rposition(|known| known.id == message.id) {
        if messages[index].seq == message.seq {
            messages[index] = message;
            return;
        }
        messages.remove(index);
    }
    let index = messages.partition_point(|known| known.seq <= message.seq);
    messages.insert(index, message);
}

impl Render for Shell {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let screen = div().size_full().bg(theme::color(theme::PAGE));
        match &self.screen {
            Screen::Starting => screen,
            Screen::Login(login) => screen.child(login.clone()),
            Screen::Chat(chat) => screen.child(chat.clone()),
            Screen::Failed(reason) => screen
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .px(px(36.))
                .font_family(theme::FONT)
                .text_size(theme::BODY)
                .line_height(theme::BODY_LINE)
                .text_center()
                .text_color(theme::color(theme::TEXT_MUTED))
                .child(
                    div()
                        .text_size(theme::LABEL)
                        .line_height(theme::LABEL_LINE)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme::color(theme::TEXT))
                        .child("OpenPickle had to stop"),
                )
                .child(reason.clone())
                .child("Quit and open the app again."),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Side;

    fn message(id: &str, seq: u64) -> Message {
        Message {
            id: id.into(),
            seq,
            at: seq as f64,
            side: Side::Mine,
            parts: vec![],
        }
    }

    #[test]
    fn upsert_orders_by_seq_and_confirms_pending_messages() {
        let mut messages = vec![message("a", 1), message("c", 5)];
        upsert(&mut messages, message("b", 3));
        upsert(&mut messages, message("sent", u64::MAX));
        upsert(&mut messages, message("d", 7));
        assert_eq!(
            messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "c", "d", "sent"]
        );
        // The server confirms the pending message with its real sequence number.
        upsert(&mut messages, message("sent", 6));
        assert_eq!(
            messages
                .iter()
                .map(|m| (m.id.as_str(), m.seq))
                .collect::<Vec<_>>(),
            [("a", 1), ("b", 3), ("c", 5), ("sent", 6), ("d", 7)]
        );
    }
}
