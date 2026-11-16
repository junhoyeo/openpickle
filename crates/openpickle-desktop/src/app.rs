//! The root view: shows sign-in or the chat, and applies what the backend reports.

use std::time::{SystemTime, UNIX_EPOCH};

use chrono::Local;
use futures::{StreamExt, channel::mpsc::UnboundedReceiver};
use gpui::{
    Context, Entity, Focusable, FontWeight, ImageSource, SharedString, Subscription, Task, Window,
    div, prelude::*, px,
};
use tokio::sync::mpsc::UnboundedSender;

use crate::{
    convert,
    model::{Message, Part, UNCONFIRMED_SEQ, build_rows},
    protocol::{Command, Update},
    theme,
    ui::{
        chat::{ChatEvent, ChatView},
        login::{LoginEvent, LoginView},
        search::{SearchEvent, SearchView},
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
    /// How many messages were typed here, which numbers the ones not confirmed yet.
    sent: u64,
    activity: Option<Option<String>>,
    /// For each message, the index of its first row in the chat's list.
    row_starts: Vec<usize>,
    /// The search screen. It is kept when it closes, so that opening it again shows the same
    /// search: a result is opened to read around it, and then usually the next one.
    search: Option<Entity<SearchView>>,
    /// Whether the search screen is drawn, over the chat.
    searching: bool,
    _search_events: Option<Subscription>,
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
        // The backend takes the window to be behind until told otherwise, and the observer
        // only reports changes.
        commands
            .send(Command::Active(window.is_window_active()))
            .ok();
        let mut shell = Self {
            commands,
            screen: Screen::Starting,
            messages: Vec::new(),
            sent: 0,
            activity: None,
            row_starts: Vec::new(),
            search: None,
            searching: false,
            _search_events: None,
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
            Update::SignedOut => {
                self.messages.clear();
                self.forget_search();
                self.show_login(window, cx)
            }
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
                open_conversation(&mut self.messages, messages);
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
            Update::SendFailed { id, reason } => {
                mark_not_sent(&mut self.messages, &id, &reason);
                self.refresh_rows(cx);
            }
            Update::Activity(activity) => {
                self.activity = activity;
                self.refresh_rows(cx);
            }
            Update::Found { query, results } => {
                if let Some(search) = &self.search {
                    search.update(cx, |search, cx| search.found(&query, results, cx));
                }
            }
            Update::Failed(reason) => {
                self.forget_search();
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
        self._screen_events = Some(
            cx.subscribe_in(&chat, window, |this, _, event, window, cx| match event {
                ChatEvent::Send(text) => this.send(text, cx),
                ChatEvent::Search => this.open_search(window, cx),
            }),
        );
        window.focus(&chat.focus_handle(cx));
        self.screen = Screen::Chat(chat.clone());
        cx.notify();
        chat
    }

    fn open_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let search = match &self.search {
            Some(search) => search.clone(),
            None => {
                let search = cx.new(SearchView::new);
                self._search_events = Some(cx.subscribe_in(
                    &search,
                    window,
                    |this, _, event, window, cx| match event {
                        SearchEvent::Query(query) => {
                            this.commands.send(Command::Search(query.clone())).ok();
                        }
                        SearchEvent::Open(seq) => this.open_result(*seq, window, cx),
                        SearchEvent::Close => this.close_search(window, cx),
                    },
                ));
                self.search = Some(search.clone());
                search
            }
        };
        window.focus(&search.focus_handle(cx));
        self.searching = true;
        cx.notify();
    }

    fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.searching = false;
        if let Screen::Chat(chat) = &self.screen {
            window.focus(&chat.focus_handle(cx));
        }
        cx.notify();
    }

    /// Drops the search screen with what it found: the conversation it searched is gone.
    fn forget_search(&mut self) {
        self.search = None;
        self.searching = false;
        self._search_events = None;
    }

    /// Leaves the search screen for the message a result names. A message that is not on
    /// this computer (yet) cannot be shown, and the search stays open.
    fn open_result(&mut self, seq: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.messages.iter().position(|message| message.seq == seq) else {
            return;
        };
        let (Screen::Chat(chat), Some(&start)) = (&self.screen, self.row_starts.get(index)) else {
            return;
        };
        let rows = start..start + self.messages[index].parts.len();
        chat.update(cx, |chat, cx| chat.reveal(rows, window, cx));
        self.close_search(window, cx);
    }

    /// Shows the message at once and hands it to the backend, which may be busy for a while:
    /// connecting, or catching up after a long sleep.
    fn send(&mut self, text: &str, cx: &mut Context<Self>) {
        let id = uuid::Uuid::new_v4().to_string();
        self.sent += 1;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0., |elapsed| elapsed.as_secs_f64());
        let message = convert::outgoing(id.clone(), text, UNCONFIRMED_SEQ + self.sent, now);
        upsert(&mut self.messages, message);
        let command = Command::Send {
            id: id.clone(),
            text: text.to_owned(),
        };
        if self.commands.send(command).is_err() {
            mark_not_sent(&mut self.messages, &id, "Not sent. OpenPickle has stopped.");
        }
        self.refresh_rows(cx);
    }

    fn refresh_rows(&mut self, cx: &mut Context<Self>) {
        if let Screen::Chat(chat) = &self.screen {
            let (rows, starts) = build_rows(
                &self.messages,
                self.activity.as_ref().map(|label| label.as_deref()),
                Local::now(),
            );
            self.row_starts = starts;
            chat.update(cx, |chat, cx| chat.set_rows(rows, cx));
        }
    }
}

/// Shows another conversation, or the same one read again. Messages that are still on their
/// way to the server belong to neither yet, so they stay.
fn open_conversation(messages: &mut Vec<Message>, conversation: Vec<Message>) {
    let unconfirmed: Vec<Message> = messages
        .drain(..)
        .filter(|message| message.seq > UNCONFIRMED_SEQ)
        .collect();
    *messages = conversation;
    for message in unconfirmed {
        if !messages.iter().any(|known| known.id == message.id) {
            upsert(messages, message);
        }
    }
}

/// Says under a message that it did not reach the server. A message the server has confirmed
/// in the meantime did reach it, and stays as it is.
fn mark_not_sent(messages: &mut [Message], id: &str, reason: &str) {
    if let Some(message) = messages
        .iter_mut()
        .rfind(|message| message.id == id && message.seq > UNCONFIRMED_SEQ)
    {
        message.parts.push(Part::Task(reason.into()));
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
        let screen = div().size_full().relative().bg(theme::color(theme::PAGE));
        match &self.screen {
            Screen::Starting => screen,
            Screen::Login(login) => screen.child(login.clone()),
            Screen::Chat(chat) => screen
                .child(chat.clone())
                .children(self.search.clone().filter(|_| self.searching)),
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
    use crate::model::{SearchResult, Side};

    fn message(id: &str, seq: u64) -> Message {
        Message {
            id: id.into(),
            seq,
            at: seq as f64,
            side: Side::Mine,
            parts: vec![],
        }
    }

    #[gpui::test]
    fn a_typed_message_shows_at_once_and_goes_to_the_backend(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let conversation = Update::Conversation {
            name: "Pickle".into(),
            avatar: None,
            messages: vec![message("a", 1)],
        };
        updates.unbounded_send(conversation).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));

        cx.simulate_input("안녕");
        cx.simulate_keystrokes("enter");
        // Nothing answers the command, as when the backend is busy. The message shows anyway.
        let (id, text) = std::iter::from_fn(|| sent.try_recv().ok())
            .find_map(|command| match command {
                Command::Send { id, text } => Some((id, text)),
                _ => None,
            })
            .expect("the message goes to the backend");
        assert_eq!(text, "안녕");
        shell.read_with(cx, |shell, _| {
            let shown = shell.messages.last().unwrap();
            assert_eq!((&shown.id, shown.seq > UNCONFIRMED_SEQ), (&id, true));
            assert_eq!(shown.parts.len(), 1);
        });

        let failed = Update::SendFailed {
            id,
            reason: "Not sent.".into(),
        };
        updates.unbounded_send(failed).unwrap();
        cx.run_until_parked();
        shell.read_with(cx, |shell, _| {
            let parts = &shell.messages.last().unwrap().parts;
            assert_eq!(parts.last(), Some(&Part::Task("Not sent.".into())));
        });
    }

    #[gpui::test]
    fn a_search_result_opens_its_message_and_closes_the_search(cx: &mut gpui::TestAppContext) {
        cx.update(crate::ui::bind_keys);
        let (commands, mut sent) = tokio::sync::mpsc::unbounded_channel();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let part = Part::Task("내일 비 온대요".into());
        let conversation = Update::Conversation {
            name: "Pickle".into(),
            avatar: None,
            messages: vec![Message {
                parts: vec![part],
                ..message("a", 1)
            }],
        };
        updates.unbounded_send(conversation).unwrap();
        let (shell, cx) =
            cx.add_window_view(|window, cx| Shell::new(commands, received, window, cx));

        cx.simulate_resize(gpui::size(gpui::px(440.), gpui::px(900.)));

        cx.simulate_keystrokes("cmd-f");
        cx.simulate_input("비");
        cx.simulate_keystrokes("enter");
        let asked = std::iter::from_fn(|| sent.try_recv().ok()).find_map(|command| match command {
            Command::Search(query) => Some(query),
            _ => None,
        });
        assert_eq!(asked.as_deref(), Some("비"));

        let found = |seq| SearchResult {
            seq: Some(seq),
            label: "Chat".into(),
            date: "Nov 16".into(),
            snippet: "내일 비 온대요".into(),
            matches: vec![0..6, 7..10],
        };
        // The first result is a message this computer does not have; the second one it has.
        let results = Update::Found {
            query: "비".into(),
            results: Ok(vec![found(99), found(1)]),
        };
        updates.unbounded_send(results).unwrap();
        cx.run_until_parked();
        let first = gpui::point(gpui::px(120.), gpui::px(75.));
        cx.simulate_click(first, gpui::Modifiers::none());
        shell.read_with(cx, |shell, _| assert!(shell.searching));
        cx.simulate_click(
            first + gpui::point(gpui::px(0.), gpui::px(70.)),
            gpui::Modifiers::none(),
        );
        shell.read_with(cx, |shell, _| assert!(!shell.searching));

        // Escape closes the search too.
        cx.simulate_keystrokes("cmd-f");
        shell.read_with(cx, |shell, _| assert!(shell.searching));
        cx.simulate_keystrokes("escape");
        shell.read_with(cx, |shell, _| assert!(!shell.searching));
    }

    #[test]
    fn a_message_on_its_way_survives_the_conversation_arriving_and_is_then_confirmed() {
        // Typed while the backend was still connecting.
        let mut messages = vec![message("typed", UNCONFIRMED_SEQ + 1)];
        open_conversation(&mut messages, vec![message("a", 1), message("b", 2)]);
        assert_eq!(
            messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["a", "b", "typed"]
        );

        // The server's copy takes its place. A failure reported after that is stale.
        upsert(&mut messages, message("typed", 3));
        mark_not_sent(&mut messages, "typed", "Not sent.");
        assert_eq!(messages.last(), Some(&message("typed", 3)));

        // Reading the conversation again does not bring a confirmed message back as pending.
        let mut messages = vec![message("typed", UNCONFIRMED_SEQ + 1)];
        open_conversation(&mut messages, vec![message("a", 1), message("typed", 2)]);
        assert_eq!(messages, [message("a", 1), message("typed", 2)]);
    }

    #[test]
    fn a_message_that_was_not_sent_says_so() {
        let mut messages = vec![message("a", 1), message("typed", UNCONFIRMED_SEQ + 1)];
        mark_not_sent(&mut messages, "typed", "Not sent. Check your connection.");
        assert_eq!(
            messages[1].parts,
            [Part::Task("Not sent. Check your connection.".into())]
        );
        assert!(messages[0].parts.is_empty());
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
