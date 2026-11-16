//! The backend thread: signs in, keeps the conversation in step with the server, and tells the
//! UI what changed. It is the only code that touches the network or the disk.
//!
//! Launch order is what makes the app feel instant: the cached conversation goes to the UI
//! first, and only then does the network get asked for what is new.

use std::{collections::HashMap, path::PathBuf, pin::Pin, time::Duration};

use futures::{Stream, StreamExt, channel::mpsc::UnboundedSender as Updates};
use openpickle_api::{
    CardState, ClientOptions, Credentials, Error, HistoryItem, LoginSession, LoginSessionRefresh,
    PickleClient, PickleEvent, SendMessageOptions,
    events::{ChatEvent, RoomEvent},
};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::mpsc::{UnboundedReceiver as Commands, UnboundedSender, unbounded_channel},
    time::Instant,
};

use crate::{
    convert,
    model::Message,
    protocol::{Command, Update},
    store::{Session, Store},
};

type EventStream = Pin<Box<dyn Stream<Item = openpickle_api::Result<PickleEvent>> + Send>>;

/// Why the backend could not start.
#[derive(Debug)]
pub enum StartError {
    /// Another instance of the app is running on this account.
    AlreadyRunning,
    /// The app's data folder cannot be used.
    Store(std::io::Error),
}

/// The ends of the backend that the app holds.
pub struct Backend {
    pub commands: UnboundedSender<Command>,
    pub updates: futures::channel::mpsc::UnboundedReceiver<Update>,
    pub stop: Stop,
}

/// Lets the app wait for the backend when it quits.
pub struct Stop {
    commands: UnboundedSender<Command>,
    /// Closed when the backend thread has finished.
    stopped: std::sync::mpsc::Receiver<()>,
}

impl Stop {
    /// How long quitting waits for a network call in flight.
    const WAIT: Duration = Duration::from_secs(3);

    /// Tells the backend to stop and waits for it, but not for long. The backend stops
    /// between network calls. That matters for one of them: a login session refresh spends
    /// the stored token, so quitting before the new one is saved signs the app out.
    pub fn stop(&self) {
        self.commands.send(Command::Quit).ok();
        // An error is the answer: the thread closed its end, or the wait ran out.
        self.stopped.recv_timeout(Self::WAIT).ok();
    }
}

/// Starts the backend thread.
pub fn start() -> Result<Backend, StartError> {
    let store = Store::open().map_err(StartError::Store)?;
    let claim = store
        .claim()
        .map_err(StartError::Store)?
        .ok_or(StartError::AlreadyRunning)?;
    let (commands, commands_rx) = unbounded_channel();
    let (updates, updates_rx) = futures::channel::mpsc::unbounded();
    let (finished, stopped) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("backend".into())
        .spawn(move || {
            // Held until the process ends.
            let _claim = claim;
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("tokio runtime");
            runtime.block_on(run(store, commands_rx, updates));
            drop(finished);
        })
        .map_err(StartError::Store)?;
    Ok(Backend {
        stop: Stop {
            commands: commands.clone(),
            stopped,
        },
        commands,
        updates: updates_rx,
    })
}

/// Why the backend stopped doing what it was doing.
enum Ended {
    /// The server no longer knows the login session.
    SignedOut,
    /// The UI is gone.
    Closed,
    /// Something on this computer failed. It says nothing about the login session, which
    /// stays where it is.
    Failed(String),
}

async fn run(store: Store, mut commands: Commands<Command>, updates: Updates<Update>) {
    let mut signed_in = store.session().map(|session| (session, None));
    let ended = loop {
        let (session, access_token) = match signed_in.take() {
            Some(signed_in) => signed_in,
            None => {
                updates.unbounded_send(Update::SignedOut).ok();
                match sign_in(&store, &mut commands, &updates).await {
                    Ok((session, access_token)) => (session, Some(access_token)),
                    Err(ended) => break ended,
                }
            }
        };
        match converse(&store, session, access_token, &mut commands, &updates).await {
            // Without this, the next person to sign in would see the old conversations.
            Ended::SignedOut => {
                if let Err(error) = store.sign_out() {
                    break Ended::Failed(format!(
                        "Pickle ended this login session, but its data on this computer could \
                         not be cleared: {error}"
                    ));
                }
            }
            ended => break ended,
        }
    };
    if let Ended::Failed(reason) = ended {
        updates.unbounded_send(Update::Failed(reason)).ok();
    }
}

/// What to say when the HTTP client cannot be set up.
fn cannot_start(error: &Error) -> Ended {
    Ended::Failed(format!(
        "Could not set up the connection to Pickle: {error}. You are still signed in."
    ))
}

/// Runs the sign-in screen until a login session exists. Returns it with its first access token.
async fn sign_in(
    store: &Store,
    commands: &mut Commands<Command>,
    updates: &Updates<Update>,
) -> Result<(Session, String), Ended> {
    let client =
        PickleClient::new(ClientOptions::default()).map_err(|error| cannot_start(&error))?;
    let mut pending_id = None;
    while let Some(command) = commands.recv().await {
        let failure = match command {
            Command::RequestCode(phone) => {
                match client.request_phone_code(&phone, None, None).await {
                    Ok(requested) => {
                        pending_id = Some(requested.pending_id);
                        updates.unbounded_send(Update::CodeSent).ok();
                        continue;
                    }
                    Err(error) => explain(&error),
                }
            }
            Command::VerifyCode(code) => match &pending_id {
                Some(pending_id) => match client.verify_phone_code(pending_id, &code).await {
                    Ok(issued) => {
                        let session = Session {
                            login_session_token: issued.login_session_token,
                            // iOS reports an upper-case UUID as its device id.
                            device_id: uuid::Uuid::new_v4().to_string().to_uppercase(),
                        };
                        match store.save_session(&session) {
                            Ok(()) => return Ok((session, issued.login_session_jwt)),
                            Err(error) => {
                                format!("Signed in, but the session could not be saved: {error}")
                            }
                        }
                    }
                    Err(error) => explain(&error),
                },
                None => "Ask for a code first.".to_owned(),
            },
            Command::Quit => break,
            Command::Send { .. } | Command::Active(_) => continue,
        };
        updates.unbounded_send(Update::SignInFailed(failure)).ok();
    }
    Err(Ended::Closed)
}

/// What to tell the user when a sign-in call fails.
fn explain(error: &Error) -> String {
    match error.api() {
        Some(api) => match &api.detail {
            Some(detail) => format!("Pickle answered: {detail}"),
            None => format!("Pickle answered with status {}", api.status),
        },
        None => "Could not reach Pickle. Check your connection and try again.".to_owned(),
    }
}

/// What to show under a message that could not be sent.
fn not_sent(error: &Error) -> String {
    match error {
        Error::Rotate(error) => {
            format!("Not sent. The login session could not be saved on this computer: {error}")
        }
        Error::Api(api) => match &api.detail {
            Some(detail) => format!("Not sent. Pickle answered: {detail}"),
            None => format!("Not sent. Pickle answered with status {}", api.status),
        },
        _ => "Not sent. Check your connection and send it again.".to_owned(),
    }
}

fn session_is_gone(error: &Error) -> bool {
    error
        .api()
        .is_some_and(|api| api.status == 401 && api.detail.as_deref() == Some("session_not_found"))
}

/// One line of a room's history log.
#[derive(Serialize, Deserialize)]
#[serde(tag = "t", content = "v")]
enum Record {
    #[serde(rename = "item")]
    Item(Box<HistoryItem>),
    #[serde(rename = "card")]
    Card(CardState),
    /// How far the log follows the server: the `next_after` of the last page written.
    #[serde(rename = "synced")]
    Synced(i64),
}

/// The name and picture last seen for the character.
#[derive(Default, Serialize, Deserialize)]
struct Identity {
    name: String,
    /// The picture's address without its signature.
    avatar: String,
    /// File name of the downloaded picture in the cache.
    avatar_file: Option<String>,
}

/// A room's history log on disk. After a failed write it stops for the rest of the run: a later
/// page would record a cursor past the missing one, and the next launch, which reads on from
/// the last cursor on disk, would never fetch the gap.
struct HistoryLog {
    store: Store,
    name: String,
    broken: bool,
}

impl HistoryLog {
    fn new(store: &Store, room_id: &str) -> Self {
        Self {
            store: store.clone(),
            name: format!("room-{room_id}.jsonl"),
            broken: false,
        }
    }

    fn append(&mut self, records: &[Record]) {
        if !self.broken {
            self.broken = self.store.append_log(&self.name, records).is_err();
        }
    }
}

/// The open room as the server described it.
#[derive(Default)]
struct Conversation {
    room_id: String,
    items: HashMap<String, HistoryItem>,
    card_states: HashMap<String, CardState>,
    /// Sequence number up to which `history/delta` has been read.
    synced: i64,
    /// `None` until the room is known.
    log: Option<HistoryLog>,
}

impl Conversation {
    fn load(store: &Store, room_id: &str) -> Self {
        let log = HistoryLog::new(store, room_id);
        let mut conversation = Self {
            room_id: room_id.to_owned(),
            ..Default::default()
        };
        for record in store.log::<Record>(&log.name) {
            match record {
                Record::Item(item) => {
                    conversation.items.insert(item.id.clone(), *item);
                }
                Record::Card(state) => {
                    conversation.card_states.insert(state.id.clone(), state);
                }
                Record::Synced(seq) => conversation.synced = seq,
            }
        }
        conversation.log = Some(log);
        conversation
    }

    fn message(&self, item: &HistoryItem) -> Message {
        convert::message(item, self.card_states.get(&item.id))
    }

    /// Every message, oldest first.
    fn messages(&self) -> Vec<Message> {
        let mut messages: Vec<Message> =
            self.items.values().map(|item| self.message(item)).collect();
        messages.sort_by_key(|message| message.seq);
        messages
    }

    /// Takes in items and card states and returns the messages they changed.
    fn apply(&mut self, items: Vec<HistoryItem>, card_states: Vec<CardState>) -> Vec<Message> {
        let mut changed: Vec<String> = Vec::with_capacity(items.len() + card_states.len());
        for state in card_states {
            changed.push(state.id.clone());
            self.card_states.insert(state.id.clone(), state);
        }
        for item in items {
            changed.push(item.id.clone());
            self.items.insert(item.id.clone(), item);
        }
        changed.sort();
        changed.dedup();
        changed
            .iter()
            .filter_map(|id| self.items.get(id))
            .map(|item| self.message(item))
            .collect()
    }

    fn head_seq(&self) -> i64 {
        self.items
            .values()
            .map(|item| item.seq)
            .max()
            .unwrap_or(0)
            .max(self.synced)
    }
}

struct Live {
    client: PickleClient,
    store: Store,
    updates: Updates<Update>,
    conversation: Conversation,
    identity: Identity,
    /// Whether the window is in front; only then does reading count as "seen".
    active: bool,
    marked_read: i64,
}

async fn converse(
    store: &Store,
    session: Session,
    access_token: Option<String>,
    commands: &mut Commands<Command>,
    updates: &Updates<Update>,
) -> Ended {
    let device_id = session.device_id.clone();
    let rotate_store = store.clone();
    let rotate_device = device_id.clone();
    let mut login = LoginSession::new(
        session.login_session_token,
        move |refresh: &LoginSessionRefresh| {
            rotate_store.save_session(&Session {
                login_session_token: refresh.login_session_token.clone(),
                device_id: rotate_device.clone(),
            })
        },
    );
    if let Some(access_token) = access_token {
        login = login.with_access_token(access_token);
    }
    let options = ClientOptions {
        credentials: Credentials::LoginSession(login),
        device_id: Some(device_id),
        ..Default::default()
    };
    // A failure here is local: only the server can say that the login session is gone.
    let client = match PickleClient::new(options) {
        Ok(client) => client,
        Err(error) => return cannot_start(&error),
    };

    // First paint comes from the disk; the network is asked afterwards.
    let room_id: Option<String> = store.cached("room.json");
    let conversation = room_id
        .as_deref()
        .map(|room_id| Conversation::load(store, room_id))
        .unwrap_or_default();
    let identity: Identity = store.cached("identity.json").unwrap_or_default();
    let mut live = Live {
        client,
        store: store.clone(),
        updates: updates.clone(),
        conversation,
        identity,
        active: true,
        marked_read: 0,
    };
    live.send(Update::Conversation {
        name: live.display_name(),
        avatar: live.avatar_path(),
        messages: live.conversation.messages(),
    });

    let mut events: Option<EventStream> = None;
    let mut connect_at = Instant::now();
    let mut backoff = Duration::from_secs(2);
    loop {
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Quit) => return Ended::Closed,
                Some(Command::Send { id, text }) => {
                    if let Err(error) = live.send_message(id.clone(), &text).await {
                        live.send(Update::SendFailed { id, reason: not_sent(&error) });
                        if session_is_gone(&error) {
                            return Ended::SignedOut;
                        }
                    }
                }
                Some(Command::Active(active)) => {
                    live.active = active;
                    live.mark_read().await;
                }
                Some(Command::RequestCode(_) | Command::VerifyCode(_)) => {}
            },
            event = next_event(&mut events), if events.is_some() => match event {
                Some(Ok(PickleEvent::Retry(delay))) => backoff = delay,
                Some(Ok(event)) => {
                    if let Err(error) = live.handle(event).await
                        && session_is_gone(&error)
                    {
                        return Ended::SignedOut;
                    }
                }
                // The stream ended or broke: catch up and reopen it after the server's delay.
                Some(Err(_)) | None => {
                    events = None;
                    connect_at = Instant::now() + backoff;
                }
            },
            _ = tokio::time::sleep_until(connect_at), if events.is_none() => match live.connect().await {
                Ok(stream) => {
                    events = Some(stream);
                    backoff = Duration::from_secs(2);
                }
                Err(error) if session_is_gone(&error) => return Ended::SignedOut,
                Err(_) => {
                    connect_at = Instant::now() + backoff;
                    backoff = (backoff * 2).min(Duration::from_secs(30));
                }
            },
        }
    }
}

async fn next_event(
    events: &mut Option<EventStream>,
) -> Option<openpickle_api::Result<PickleEvent>> {
    match events {
        Some(events) => events.next().await,
        None => std::future::pending().await,
    }
}

impl Live {
    fn send(&self, update: Update) {
        self.updates.unbounded_send(update).ok();
    }

    fn display_name(&self) -> String {
        if self.identity.name.is_empty() {
            "Pickle".to_owned()
        } else {
            self.identity.name.clone()
        }
    }

    fn avatar_path(&self) -> Option<PathBuf> {
        let path = self.store.cache_path(self.identity.avatar_file.as_deref()?);
        path.exists().then_some(path)
    }

    /// Finds the room, reads what is new, and opens the event stream.
    async fn connect(&mut self) -> openpickle_api::Result<EventStream> {
        // Open the stream first so nothing that happens during the catch-up is missed.
        let events: EventStream = Box::pin(self.client.events().await?);
        self.find_room().await?;
        self.catch_up().await?;
        self.refresh_identity().await;
        Ok(events)
    }

    /// Asks the server which room is the one with the character. When it is not the open one
    /// (the first launch, or another account), opens it with what the disk has for it.
    async fn find_room(&mut self) -> openpickle_api::Result<()> {
        let rooms = self.client.sync().await?;
        if let Some(room) = rooms.rooms.iter().find(|room| room.kind == "character")
            && room.room_id != self.conversation.room_id
        {
            self.store.cache("room.json", &room.room_id).ok();
            self.conversation = Conversation::load(&self.store, &room.room_id);
            self.send(Update::Conversation {
                name: self.display_name(),
                avatar: self.avatar_path(),
                messages: self.conversation.messages(),
            });
        }
        Ok(())
    }

    /// Reads `history/delta` from where the log ends, page by page.
    async fn catch_up(&mut self) -> openpickle_api::Result<()> {
        if self.conversation.room_id.is_empty() {
            return Ok(());
        }
        loop {
            let delta = self
                .client
                .get_history_delta(&self.conversation.room_id, self.conversation.synced, None)
                .await?;
            // A page that brings nothing new leaves the log as it is.
            if delta.next_after > self.conversation.synced
                && let Some(log) = &mut self.conversation.log
            {
                let mut records: Vec<Record> = delta
                    .items
                    .iter()
                    .cloned()
                    .map(|item| Record::Item(Box::new(item)))
                    .collect();
                records.extend(delta.card_states.iter().cloned().map(Record::Card));
                records.push(Record::Synced(delta.next_after));
                log.append(&records);
            }

            let advanced = delta.next_after > self.conversation.synced;
            self.conversation.synced = delta.next_after.max(self.conversation.synced);
            let changed = self.conversation.apply(delta.items, delta.card_states);
            if !changed.is_empty() {
                self.send(Update::Upsert(changed));
            }
            if !delta.has_more || !advanced {
                break;
            }
        }
        self.mark_read().await;
        Ok(())
    }

    /// Fetches the character's name and picture and tells the UI when either changed.
    async fn refresh_identity(&mut self) {
        let Ok(character) = self.client.get_character().await else {
            return;
        };
        // The address is signed and changes on every call; the path names the picture.
        let picture = character
            .avatar_url
            .split('?')
            .next()
            .unwrap_or_default()
            .to_owned();
        let renamed = character.name != self.identity.name;
        let repainted = picture != self.identity.avatar || self.avatar_path().is_none();
        self.identity.name = character.name;
        if repainted
            && !picture.is_empty()
            && let Some(file) = download_avatar(&self.store, &character.avatar_url).await
        {
            self.identity.avatar_file = Some(file);
            self.identity.avatar = picture;
        }
        if renamed || repainted {
            self.store.cache("identity.json", &self.identity).ok();
            self.send(Update::Identity {
                name: self.display_name(),
                avatar: self.avatar_path(),
            });
        }
    }

    async fn handle(&mut self, event: PickleEvent) -> openpickle_api::Result<()> {
        match event {
            PickleEvent::Chat(ChatEvent::User {
                role,
                id,
                text,
                at,
                seq,
                ..
            })
            | PickleEvent::Chat(ChatEvent::Assistant {
                role,
                id,
                text,
                at,
                seq,
                ..
            }) => {
                // Shown at once; the next catch-up logs it with everything the event leaves out.
                if let Ok(item) = serde_json::from_value::<HistoryItem>(
                    serde_json::json!({ "role": role, "id": id, "text": text, "at": at, "seq": seq }),
                ) {
                    let changed = self.conversation.apply(vec![item], Vec::new());
                    self.send(Update::Upsert(changed));
                }
            }
            PickleEvent::Chat(ChatEvent::Pending { on }) => {
                self.send(Update::Activity(on.then_some(None)))
            }
            PickleEvent::Chat(ChatEvent::Activity { state, label, .. }) => {
                self.send(Update::Activity(Some(label.filter(|_| state == "tool"))));
            }
            PickleEvent::Room(room) => {
                let RoomEvent::Advanced {
                    room_id, last_seq, ..
                } = *room;
                if room_id == self.conversation.room_id && last_seq > self.conversation.synced {
                    self.catch_up().await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Sends a message the UI already shows. The server's copy, which carries the same id,
    /// replaces it when it arrives.
    async fn send_message(&mut self, id: String, text: &str) -> openpickle_api::Result<()> {
        // Right after a first sign-in the room may not be known yet.
        if self.conversation.room_id.is_empty() {
            self.find_room().await?;
        }
        let options = SendMessageOptions {
            client_msg_id: Some(id),
            in_call: false,
        };
        self.client
            .send_message(&self.conversation.room_id, text, options)
            .await
            .map(drop)
    }

    /// Tells the server how far the user has read, while the window is in front.
    async fn mark_read(&mut self) {
        let head = self.conversation.head_seq();
        if self.active
            && head > self.marked_read
            && !self.conversation.room_id.is_empty()
            && self
                .client
                .mark_read(&self.conversation.room_id, head)
                .await
                .is_ok()
        {
            self.marked_read = head;
        }
    }
}

/// Saves the picture at `url` in the cache and returns its file name.
async fn download_avatar(store: &Store, url: &str) -> Option<String> {
    let response = reqwest::get(url).await.ok()?.error_for_status().ok()?;
    let bytes = response.bytes().await.ok()?;
    // The extension tells GPUI how to decode the file.
    let extension = url
        .split('?')
        .next()?
        .rsplit('.')
        .next()
        .filter(|extension| extension.len() <= 4)
        .unwrap_or("png");
    let file = format!("avatar.{extension}");
    store.cache_bytes(&file, &bytes).ok()?;
    Some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn quit_stops_the_backend() {
        let root =
            std::env::temp_dir().join(format!("openpickle-test-quit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::at(root).unwrap();
        let (commands, commands_rx) = unbounded_channel();
        let (updates, mut updates_rx) = futures::channel::mpsc::unbounded();

        // Nobody is signed in, so the backend waits at the sign-in screen until it is told.
        commands.send(Command::Quit).unwrap();
        run(store, commands_rx, updates).await;
        assert!(matches!(updates_rx.next().await, Some(Update::SignedOut)));
        assert!(updates_rx.next().await.is_none());
    }

    #[test]
    fn only_the_server_can_say_that_the_login_session_is_gone() {
        let answered = |status, detail: &str| {
            Error::Api(Box::new(openpickle_api::ApiError {
                method: reqwest::Method::GET,
                path: "/api/sync".into(),
                status,
                detail: Some(detail.into()),
                body: Default::default(),
            }))
        };
        assert!(session_is_gone(&answered(401, "session_not_found")));
        // An expired access token is refreshed, and other answers are not about the session.
        assert!(!session_is_gone(&answered(401, "token_expired")));
        assert!(!session_is_gone(&answered(404, "session_not_found")));
        // Failures on this computer must never sign anyone out: that deletes the session.
        assert!(!session_is_gone(&Error::Rotate(std::io::Error::other(
            "disk full"
        ))));
        assert!(!session_is_gone(&Error::RefreshAborted));
        assert!(!session_is_gone(&Error::InvalidHeader));
    }

    #[test]
    fn a_failed_write_stops_the_log_instead_of_leaving_a_gap() {
        let root = std::env::temp_dir().join(format!(
            "openpickle-test-history-log-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::at(root).unwrap();
        let mut log = HistoryLog::new(&store, "room");

        log.append(&[Record::Synced(100)]);
        // A directory where the file should be makes the next write fail.
        let path = store.cache_path(&log.name);
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        log.append(&[Record::Synced(500)]);
        // The disk works again, but a cursor of 900 must not be recorded: the page up to 500
        // never reached the log.
        std::fs::remove_dir(&path).unwrap();
        log.append(&[Record::Synced(900)]);

        assert!(log.broken);
        assert!(store.log::<Record>(&log.name).is_empty());
    }
}
