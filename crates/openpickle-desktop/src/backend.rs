//! The backend thread: signs in, keeps the conversation in step with the server, and tells the
//! UI what changed. It is the only code that touches the network or the disk.
//!
//! Launch order is what makes the app feel instant: the cached conversation goes to the UI
//! first, and only then does the network get asked for what is new.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::{Stream, StreamExt, channel::mpsc::UnboundedSender as Updates};
use openpickle_api::{
    ALL_SOCIAL_SCOPES, CardState, ClientOptions, Credentials, Error, ExploreSectionsScreen,
    HistoryItem, LoginSession, LoginSessionRefresh, MessageContext, PickleClient, PickleEvent,
    SendMessageOptions, SocialSnapshot,
    events::{ChatEvent, RoomEvent},
};
use serde::{Deserialize, Serialize};
use tokio::{
    sync::mpsc::{UnboundedReceiver as Commands, UnboundedSender, unbounded_channel},
    time::Instant,
};

use crate::{
    convert,
    model::{Attachment, Message},
    phone::CallSummary,
    profile::ProfileData,
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

#[derive(Debug)]
enum OpenRoomError {
    Session(Error),
    Visible(String),
}

impl OpenRoomError {
    fn explain(&self) -> String {
        match self {
            Self::Session(error) => explain(error),
            Self::Visible(reason) => reason.clone(),
        }
    }
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
            Command::Send { .. }
            | Command::Search(_)
            | Command::OpenRoom(_)
            | Command::LoadSocial
            | Command::LoadExplore
            | Command::LoadProfile
            | Command::LoadPhoneCalls
            | Command::ResolvePhoneTask(_)
            | Command::LoadHandoffCard(_)
            | Command::WatchHandoff(_)
            | Command::DownloadAttachment(_)
            | Command::WatchPhoneCall(_)
            | Command::EndPhoneCall { .. }
            | Command::OffersShown(_)
            | Command::LoadArchive
            | Command::LoadAttachments(_)
            | Command::PreviewAttachment(_)
            | Command::Active(_) => continue,
        };
        updates.unbounded_send(Update::SignInFailed(failure)).ok();
    }
    Err(Ended::Closed)
}

/// What to tell the user when a sign-in call or a search fails.
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

    /// The pictures and files of every message, newest first.
    fn attachments(&self) -> Vec<Attachment> {
        let mut items: Vec<&HistoryItem> = self.items.values().collect();
        items.sort_by_key(|item| std::cmp::Reverse(item.seq));
        items.into_iter().flat_map(convert::attachments).collect()
    }

    /// The oldest message on this computer, which is where reading back starts.
    fn first_seq(&self) -> Option<i64> {
        self.items.values().map(|item| item.seq).min()
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

    /// Takes in a message as the event stream announces it: its text, without the attachments,
    /// the request and the rest that `history/delta` brings. A message that is known already
    /// keeps those, and an announcement older than what is known changes nothing. Returns the
    /// message when it changed.
    fn hear(&mut self, heard: HistoryItem) -> Option<Message> {
        let id = heard.id.clone();
        match self.items.get_mut(&id) {
            Some(known) if heard.seq < known.seq => return None,
            Some(known) => {
                known.role = heard.role;
                known.text = heard.text;
                known.at = heard.at;
                known.seq = heard.seq;
                if heard.title.is_some() {
                    known.title = heard.title;
                }
                if heard.path.is_some() {
                    known.path = heard.path;
                }
            }
            None => {
                self.items.insert(id.clone(), heard);
            }
        }
        self.items.get(&id).map(|item| self.message(item))
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

/// The capture repeatedly fetches the open call while it is live (median about 3.75 s).
/// This interval is for display refresh only, never an automatic call action.
const PHONE_REFRESH: Duration = Duration::from_secs(4);

fn valid_call_id(id: &str) -> bool {
    // IDs are opaque server values. The API client encodes the entire value as one path
    // segment; restricting it to hexadecimal/ASCII would reject valid future records.
    !id.trim().is_empty() && !id.chars().any(char::is_control)
}

fn valid_handoff_path(path: &str) -> bool {
    matches!(
        crate::content::LinkTarget::parse(&format!("handoff:{path}")),
        Some(crate::content::LinkTarget::PhoneTask(_))
    )
}

/// What the handoff list has about a task in the chat.
enum TaskCard {
    /// The task placed this call.
    Call(String),
    /// The task is not a call, or has not placed its call yet.
    Task(Box<openpickle_api::HandoffItem>),
    /// The list does not have the task: it has only just started, or it is long past.
    Unlisted,
}

fn task_card(handoffs: &openpickle_api::HandoffsScreen, path: &str) -> TaskCard {
    let item = handoffs
        .active_items
        .iter()
        .chain(&handoffs.recent_items)
        .chain(&handoffs.items)
        .find(|item| format!("handoffs/{}", item.name) == path);
    match item {
        None => TaskCard::Unlisted,
        Some(item) => match item.phone_call_id.clone().filter(|id| valid_call_id(id)) {
            Some(call_id) => TaskCard::Call(call_id),
            None => TaskCard::Task(Box::new(item.clone())),
        },
    }
}

/// The call of the task at this handoff path, when the list has that task and it has a call.
fn task_call_id(handoffs: &openpickle_api::HandoffsScreen, path: &str) -> Option<String> {
    match task_card(handoffs, path) {
        TaskCard::Call(call_id) => Some(call_id),
        _ => None,
    }
}

/// The title of the task the character works on now, which the chat's header shows.
fn active_task_title(handoffs: &openpickle_api::HandoffsScreen) -> Option<String> {
    handoffs
        .active_items
        .iter()
        .find(|item| item.state == "running")
        .map(|item| item.title.clone())
}

/// The task whose progress is open, and when it is read again.
struct HandoffWatch {
    path: String,
    next_refresh: Option<Instant>,
}

struct PhoneWatch {
    id: String,
    latest: Option<openpickle_api::PhoneCallTranscript>,
    next_refresh: Option<Instant>,
    end_requested: bool,
}

impl PhoneWatch {
    fn new(id: String) -> Self {
        Self {
            id,
            latest: None,
            next_refresh: None,
            end_requested: false,
        }
    }

    fn can_end(&self, call_id: &str) -> bool {
        !self.end_requested
            && self.id == call_id
            && self
                .latest
                .as_ref()
                .is_some_and(|call| call.call_id == call_id && call.live && !call.hangup_requested)
    }
}

/// The cards of the tasks in the chat: which of them show a call that is still live or a task
/// that still runs, and so are read again, and the handoff list the cards drawn together were
/// found in.
#[derive(Default)]
struct HandoffCards {
    /// Handoff path and call id of each live call a card shows.
    live: Vec<(String, String)>,
    /// Tasks that still run or that the handoff list does not have yet, and until when a task
    /// it does not have is still asked about.
    waiting: Vec<(String, Instant)>,
    next_refresh: Option<Instant>,
    handoffs: Option<(Instant, openpickle_api::HandoffsScreen)>,
    /// Whether the list had a running task when it was last read. While it has one it is read
    /// again, also when no card waits for it: the chat's header has to learn that the task
    /// ended.
    active: bool,
}

/// How long one reading of the handoff list serves the cards that are drawn together.
const CARD_HANDOFFS_FRESH: Duration = Duration::from_secs(10);
/// How long a card waits for a task the handoff list does not have: a task that has just
/// started is listed within seconds, one from long ago never is.
const CARD_UNLISTED_WAIT: Duration = Duration::from_secs(30);

struct Live {
    client: PickleClient,
    store: Store,
    updates: Updates<Update>,
    conversation: Conversation,
    identity: Identity,
    profile: Option<ProfileData>,
    handoff_watch: Option<HandoffWatch>,
    phone_watch: Option<PhoneWatch>,
    handoff_cards: HandoffCards,
    /// Where downloaded attachments go: the user's Downloads folder.
    download_dir: PathBuf,
    /// Whether the window is in front; only then does reading count as "seen".
    active: bool,
    /// Whether the history on this computer reaches back to the first message.
    history_complete: bool,
    /// The room with the character, which the app starts in.
    character_room: String,
    /// Another room the window switched to. While it is set, the stream's chat events for
    /// the character's room must not be mixed into it.
    room_override: Option<String>,
    /// The rooms as last listed by `sync`; this is the authority for room links and send access.
    last_rooms: Vec<openpickle_api::Room>,
    /// Becomes false before this account ends, so detached picture tasks cannot recreate a
    /// signed-out account's cache.
    account_active: Arc<AtomicBool>,
    /// The cache files that were shown to the window in this run.
    shown_from_disk: HashSet<&'static str>,
    pictures: PictureFetches,
    marked_read: i64,
}

fn save_refresh_if_active(
    store: &Store,
    account_active: &AtomicBool,
    device_id: &str,
    refresh: &LoginSessionRefresh,
) -> std::io::Result<()> {
    if !account_active.load(Ordering::Acquire) {
        return Ok(());
    }
    store.save_session(&Session {
        login_session_token: refresh.login_session_token.clone(),
        device_id: device_id.to_owned(),
    })
}

async fn converse(
    store: &Store,
    session: Session,
    access_token: Option<String>,
    commands: &mut Commands<Command>,
    updates: &Updates<Update>,
) -> Ended {
    let device_id = session.device_id.clone();
    let account_active = Arc::new(AtomicBool::new(true));
    let rotate_store = store.clone();
    let rotate_device = device_id.clone();
    let rotate_active = account_active.clone();
    let mut login = LoginSession::new(
        session.login_session_token,
        move |refresh: &LoginSessionRefresh| {
            save_refresh_if_active(&rotate_store, &rotate_active, &rotate_device, refresh)
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
        profile: None,
        handoff_watch: None,
        phone_watch: None,
        handoff_cards: HandoffCards::default(),
        download_dir: download_dir(),
        // The window says when it comes to the front.
        active: false,
        history_complete: false,
        character_room: String::new(),
        room_override: None,
        last_rooms: Vec::new(),
        account_active: account_active.clone(),
        shown_from_disk: HashSet::new(),
        pictures: PictureFetches::new(account_active),
        marked_read: 0,
    };
    live.send_startup_conversation();

    let mut events: Option<EventStream> = None;
    let mut connect_at = Instant::now();
    let mut opened_at = Instant::now();
    let mut backoff = Backoff::default();
    loop {
        let watch_deadline = live
            .phone_watch
            .as_ref()
            .and_then(|watch| watch.next_refresh);
        let cards_deadline = live.handoff_cards.next_refresh;
        let handoff_deadline = live
            .handoff_watch
            .as_ref()
            .and_then(|watch| watch.next_refresh);
        let phone_deadline = watch_deadline
            .into_iter()
            .chain(cards_deadline)
            .chain(handoff_deadline)
            .min();
        tokio::select! {
            command = commands.recv() => match command {
                None | Some(Command::Quit) => {
                    // Background reads can have started a rotation. Keep the store alive until
                    // it finishes, and stop their queued clones from starting another one.
                    let _ = live.client.close_login_session().await;
                    return Ended::Closed;
                }
                Some(Command::Send { room_id, id, text, offer }) => {
                    let sync_failure = if live.last_rooms.is_empty() {
                        match live.find_room().await {
                            Ok(()) => None,
                            Err(error) if session_is_gone(&error) => return Ended::SignedOut,
                            Err(error) => Some(not_sent(&error)),
                        }
                    } else {
                        None
                    };
                    let failure = if sync_failure.is_some() {
                        sync_failure
                    } else if let Some(reason) = live.send_block_reason(&room_id) {
                        Some(reason.to_owned())
                    } else {
                        match live.send_message(&room_id, id.clone(), &text, offer).await {
                            Ok(()) => None,
                            Err(error) if session_is_gone(&error) => return Ended::SignedOut,
                            Err(error) => Some(not_sent(&error)),
                        }
                    };
                    if let Some(reason) = failure {
                        live.send(Update::SendFailed { room_id, id, reason });
                    }
                }
                Some(Command::Search(query)) => {
                    let results = match live.client.search_messages(&query).await {
                        Ok(found) => Ok(found
                            .items
                            .into_iter()
                            .map(|item| convert::search_result(item, &live.conversation.room_id))
                            .collect()),
                        Err(error) if session_is_gone(&error) => return Ended::SignedOut,
                        Err(error) => Err(explain(&error)),
                    };
                    live.send(Update::Found { query, results });
                }
                Some(Command::LoadSocial) => {
                    if let Some(cached) = live.cached_once::<SocialSnapshot>("social.json") {
                        live.show_social(&cached);
                    }
                    match live.client.get_social_snapshot(&ALL_SOCIAL_SCOPES).await {
                        Ok(snapshot) => {
                            live.store.cache("social.json", &snapshot).ok();
                            live.show_social(&snapshot);
                        }
                        Err(error) if session_is_gone(&error) => return Ended::SignedOut,
                        Err(error) => live.send(Update::Social(Err(explain(&error)))),
                    }
                }
                Some(Command::ResolvePhoneTask(path)) => {
                    if let Err(error) = live.resolve_phone_task(path).await
                        && session_is_gone(&error) { return Ended::SignedOut; }
                }
                Some(Command::LoadHandoffCard(path)) => {
                    if let Err(error) = live.load_handoff_card(path).await
                        && session_is_gone(&error) { return Ended::SignedOut; }
                }
                Some(Command::WatchHandoff(path)) => {
                    live.handoff_watch = path.map(|path| HandoffWatch {
                        path,
                        next_refresh: None,
                    });
                    if live.handoff_watch.is_some()
                        && let Err(error) = live.refresh_handoff().await
                        && session_is_gone(&error) {
                        return Ended::SignedOut;
                    }
                }
                Some(Command::LoadPhoneCalls) => {
                    if let Some(cached) = live.cached_once::<Vec<CallSummary>>("phone-calls.json") {
                        live.send(Update::PhoneCalls(Ok(cached)));
                    }
                    if let Err(error) = live.load_phone_calls().await {
                        if session_is_gone(&error) { return Ended::SignedOut; }
                        live.send(Update::PhoneCalls(Err(explain(&error))));
                    }
                }
                Some(Command::WatchPhoneCall(call_id)) => {
                    live.phone_watch = match call_id {
                        Some(id) if valid_call_id(&id) => Some(PhoneWatch::new(id)),
                        Some(id) => {
                            live.send(Update::PhoneCall { call_id: id,
                                result: Err("This call link is not valid.".into()) });
                            None
                        }
                        None => None,
                    };
                    if live.phone_watch.is_some()
                        && let Err(error) = live.refresh_phone_call().await
                        && session_is_gone(&error) {
                        return Ended::SignedOut;
                    }
                }
                Some(Command::EndPhoneCall { call_id, action_id }) => {
                    if let Err(error) = live.end_phone_call(call_id, action_id).await
                        && session_is_gone(&error) {
                        return Ended::SignedOut;
                    }
                }
                Some(Command::LoadProfile) => {
                    if let Some(cached) = live.cached_once::<ProfileData>("profile-view.json") {
                        live.profile = Some(cached.clone());
                        live.send(Update::Profile(Ok(cached)));
                    }
                    if let Err(error) = live.load_profile().await {
                        if session_is_gone(&error) { return Ended::SignedOut; }
                        live.send(Update::Profile(Err(explain(&error))));
                    }
                }
                Some(Command::LoadExplore) => {
                    if let Some(cached) = live.cached_once::<ExploreSectionsScreen>("explore.json") {
                        live.show_explore(cached);
                    }
                    match live.client.get_explore_sections().await {
                        Ok(screen) => {
                            live.store.cache("explore.json", &screen).ok();
                            live.show_explore(screen);
                        }
                        Err(error) if session_is_gone(&error) => return Ended::SignedOut,
                        Err(error) => live.send(Update::Explore(Err(explain(&error)))),
                    }
                }
                Some(Command::OpenRoom(room_id)) => {
                    if let Err(error) = live.open_room_and_report(room_id).await
                        && session_is_gone(&error)
                    {
                        return Ended::SignedOut;
                    }
                }
                Some(Command::LoadArchive) => {
                    if let Err(error) = live.read_archive().await {
                        if session_is_gone(&error) {
                            return Ended::SignedOut;
                        }
                        // What was read is shown; the rest is read on the next opening.
                        live.show_archive(false);
                    }
                }
                Some(Command::LoadAttachments(paths)) => live.fetch_attachments(paths),
                Some(Command::PreviewAttachment(path)) => {
                    if let Err(error) = live.preview_attachment(path).await
                        && session_is_gone(&error)
                    {
                        return Ended::SignedOut;
                    }
                }
                Some(Command::DownloadAttachment(path)) => live.download_attachment(path),
                Some(Command::OffersShown(offers)) => {
                    // Nothing on screen depends on this report, so a failure is not shown.
                    if let Err(error) = live.client.report_offer_impressions(&offers).await
                        && session_is_gone(&error)
                    {
                        return Ended::SignedOut;
                    }
                }
                Some(Command::Active(active)) => {
                    live.active = active;
                    live.mark_read().await;
                }
                Some(Command::RequestCode(_) | Command::VerifyCode(_)) => {}
            },
            _ = tokio::time::sleep_until(phone_deadline.unwrap_or_else(Instant::now)), if phone_deadline.is_some() => {
                let now = Instant::now();
                if watch_deadline.is_some_and(|at| at <= now)
                    && let Err(error) = live.refresh_phone_call().await
                    && session_is_gone(&error) {
                    return Ended::SignedOut;
                }
                if cards_deadline.is_some_and(|at| at <= now)
                    && let Err(error) = live.refresh_handoff_cards().await
                    && session_is_gone(&error) {
                    return Ended::SignedOut;
                }
                if handoff_deadline.is_some_and(|at| at <= now)
                    && let Err(error) = live.refresh_handoff().await
                    && session_is_gone(&error) {
                    return Ended::SignedOut;
                }
            },
            event = next_event(&mut events), if events.is_some() => match event {
                Some(Ok(PickleEvent::Retry(delay))) => backoff.server_asks(delay),
                Some(Ok(event)) => {
                    if let Err(error) = live.handle(event).await
                        && session_is_gone(&error)
                    {
                        return Ended::SignedOut;
                    }
                }
                // The stream ended or broke: catch up and open it again after a wait.
                Some(Err(_)) | None => {
                    events = None;
                    backoff.stream_ended(opened_at.elapsed());
                    connect_at = Instant::now() + backoff.next();
                }
            },
            _ = tokio::time::sleep_until(connect_at), if events.is_none() => match live.connect().await {
                Ok(stream) => {
                    events = Some(stream);
                    opened_at = Instant::now();
                }
                Err(error) if session_is_gone(&error) => return Ended::SignedOut,
                Err(_) => connect_at = Instant::now() + backoff.next(),
            },
        }
    }
}

/// Reads one room's delta page by page. A visible conversation can publish each applied page,
/// while a staged room switch can wait until the whole catch-up succeeds before publishing.
async fn catch_up_conversation<F>(
    client: &PickleClient,
    conversation: &mut Conversation,
    mut page_applied: F,
) -> openpickle_api::Result<()>
where
    F: FnMut(Vec<Message>),
{
    if conversation.room_id.is_empty() {
        return Ok(());
    }
    loop {
        let delta = client
            .get_history_delta(&conversation.room_id, conversation.synced, None)
            .await?;
        if delta.next_after > conversation.synced
            && let Some(log) = &mut conversation.log
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

        let advanced = delta.next_after > conversation.synced;
        conversation.synced = delta.next_after.max(conversation.synced);
        let changed = conversation.apply(delta.items, delta.card_states);
        if !changed.is_empty() {
            page_applied(changed);
        }
        if !delta.has_more || !advanced {
            break;
        }
    }
    Ok(())
}

/// How long to wait before opening the event stream again. The wait doubles with every
/// attempt and starts over only when a stream has stayed open for a while: a stream that
/// opens and ends at once is as much a failure as one that does not open.
struct Backoff {
    /// The wait to start over with. The server may name it.
    first: Duration,
    wait: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(2),
            wait: Duration::from_secs(2),
        }
    }
}

impl Backoff {
    const LONGEST: Duration = Duration::from_secs(30);
    /// A stream that was open this long was a working connection.
    const SETTLED: Duration = Duration::from_secs(30);

    /// The wait before the next attempt.
    fn next(&mut self) -> Duration {
        let wait = self.wait;
        self.wait = (wait * 2).min(Self::LONGEST).max(self.first);
        wait
    }

    fn stream_ended(&mut self, open_for: Duration) {
        if open_for >= Self::SETTLED {
            self.wait = self.first;
        }
    }

    fn server_asks(&mut self, wait: Duration) {
        self.first = wait;
        self.wait = self.wait.max(wait);
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

    /// The disk can paint cached messages before the room roster has established membership,
    /// but it cannot grant send access.
    fn send_startup_conversation(&self) {
        self.send(Update::Conversation {
            room_id: self.conversation.room_id.clone(),
            is_character: true,
            can_send: false,
            name: self.display_name(),
            avatar: self.avatar_path(),
            messages: self.conversation.messages(),
        });
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

    /// Refreshes the authoritative room roster without changing or republishing the visible
    /// conversation. Room-opening uses this while its target is still staged.
    async fn refresh_rooms(&mut self) -> openpickle_api::Result<Option<String>> {
        let rooms = self.client.sync().await?;
        let character_room = rooms
            .rooms
            .iter()
            .find(|room| room.kind == "character")
            .map(|room| room.room_id.clone());
        self.show_rooms(&rooms);
        if let Some(character_room) = &character_room {
            self.character_room = character_room.clone();
            // This cache is intentionally always the primary room. A launch must never reopen a
            // peer room before `sync` can validate that the current account may see it.
            self.store.cache("room.json", character_room).ok();
        }
        Ok(character_room)
    }

    /// Asks the server which room is the one with the character. When it is not the open one
    /// (the first launch, or another account), opens it with what the disk has for it.
    async fn find_room(&mut self) -> openpickle_api::Result<()> {
        if let Some(character_room) = self.refresh_rooms().await? {
            let wanted = self
                .room_override
                .as_ref()
                .filter(|wanted| self.known_room(wanted).is_some())
                .cloned()
                .unwrap_or_else(|| character_room.clone());
            self.room_override = (wanted != character_room).then(|| wanted.clone());
            if wanted != self.conversation.room_id {
                self.conversation = Conversation::load(&self.store, &wanted);
                self.reset_room_state();
            }
            // `sync` is the authority for membership and permissions. Publish it even when the
            // cached room was already open, so startup cannot leave the composer disabled or
            // accidentally preserve stale write access.
            self.send_update_for(&wanted);
        }
        Ok(())
    }

    /// Reads `history/delta` from where the log ends, page by page.
    async fn catch_up(&mut self) -> openpickle_api::Result<()> {
        let room_id = self.conversation.room_id.clone();
        let updates = self.updates.clone();
        catch_up_conversation(&self.client, &mut self.conversation, |messages| {
            updates
                .unbounded_send(Update::Upsert {
                    room_id: room_id.clone(),
                    messages,
                })
                .ok();
        })
        .await?;
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
            if self.conversation.room_id == self.character_room {
                self.send(Update::Identity {
                    name: self.display_name(),
                    avatar: self.avatar_path(),
                });
            }
        }
    }

    async fn handle(&mut self, event: PickleEvent) -> openpickle_api::Result<()> {
        match event {
            PickleEvent::Usage(usage) => {
                if let Some(data) = &mut self.profile {
                    let before = data.clone();
                    if data.apply_usage_event(&usage) && *data != before {
                        let changed = data.clone();
                        self.store.cache("profile-view.json", &changed).ok();
                        self.send(Update::Profile(Ok(changed)));
                    }
                }
            }
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
                // Chat events carry no room id and belong only to the primary character room.
                // While a peer room is open, its messages arrive through scoped `room` events.
                if self.conversation.room_id == self.character_room
                    && let Ok(item) = serde_json::from_value::<HistoryItem>(
                        serde_json::json!({ "role": role, "id": id, "text": text, "at": at, "seq": seq }),
                    )
                    && let Some(message) = self.conversation.hear(item)
                {
                    self.send(Update::Upsert {
                        room_id: self.character_room.clone(),
                        messages: vec![message],
                    });
                }
            }
            PickleEvent::Chat(ChatEvent::Subagent {
                role,
                id,
                title,
                path,
                at,
                seq,
                ..
            }) => {
                if self.conversation.room_id == self.character_room
                    && let Ok(item) = serde_json::from_value::<HistoryItem>(serde_json::json!({
                        "role":role,"id":id,"title":title,"path":path,"at":at,"seq":seq
                    }))
                    && let Some(message) = self.conversation.hear(item)
                {
                    self.send(Update::Upsert {
                        room_id: self.character_room.clone(),
                        messages: vec![message],
                    });
                }
            }
            PickleEvent::Chat(ChatEvent::Pending { on }) => {
                if self.conversation.room_id == self.character_room {
                    self.send(Update::Activity {
                        room_id: self.character_room.clone(),
                        activity: on.then_some(None),
                    });
                }
            }
            PickleEvent::Chat(ChatEvent::Activity { state, label, .. }) => {
                if self.conversation.room_id == self.character_room {
                    self.send(Update::Activity {
                        room_id: self.character_room.clone(),
                        activity: Some(label.filter(|_| state == "tool")),
                    });
                }
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
    async fn send_message(
        &mut self,
        room_id: &str,
        id: String,
        text: &str,
        offer: Option<String>,
    ) -> openpickle_api::Result<()> {
        let options = SendMessageOptions {
            client_msg_id: Some(id),
            context: offer.map(MessageContext::offer),
            ..Default::default()
        };
        self.client
            .send_message(room_id, text, options)
            .await
            .map(drop)
    }

    /// Lists the conversation's pictures and files. The history on this computer usually
    /// starts where the app was first opened; the messages before it are read page by page,
    /// logged like any other, and the list is shown again after each page.
    async fn read_archive(&mut self) -> openpickle_api::Result<()> {
        if self.conversation.room_id.is_empty() {
            self.find_room().await?;
        }
        self.show_archive(self.history_complete);
        while !self.history_complete {
            let before = self.conversation.first_seq().unwrap_or(i64::MAX);
            let page = self
                .client
                .get_history_page(&self.conversation.room_id, before, ARCHIVE_PAGE)
                .await?;
            // A page that reaches back no further than what is known ends the reading.
            let older = page.messages.iter().any(|item| item.seq < before);
            self.history_complete = !page.has_more || !older;
            if let Some(log) = &mut self.conversation.log {
                let mut records: Vec<Record> = page
                    .messages
                    .iter()
                    .cloned()
                    .map(|item| Record::Item(Box::new(item)))
                    .collect();
                records.extend(page.card_states.iter().cloned().map(Record::Card));
                log.append(&records);
            }
            let changed = self.conversation.apply(page.messages, page.card_states);
            if !changed.is_empty() {
                self.send(Update::Upsert {
                    room_id: self.conversation.room_id.clone(),
                    messages: changed,
                });
            }
            self.show_archive(self.history_complete);
        }
        Ok(())
    }

    fn show_archive(&mut self, complete: bool) {
        self.send(Update::Archive {
            room_id: self.conversation.room_id.clone(),
            attachments: self.conversation.attachments(),
            complete,
        });
    }

    /// Downloads the thumbnails of the Archive's attachments that are not on disk yet. The
    /// picture cache keys a thumbnail by the attachment's path.
    fn fetch_attachments(&mut self, paths: Vec<String>) {
        self.pictures.forget_failed();
        let allowed: HashSet<String> = self
            .conversation
            .attachments()
            .into_iter()
            .map(|attachment| attachment.path)
            .collect();
        let sources = paths
            .into_iter()
            .filter(|path| allowed.contains(path))
            .map(|path| Source::Thumbnail(self.client.clone(), path));
        let ready = self.pictures.fetch(sources, &self.store, &self.updates);
        if !ready.is_empty() {
            self.send(Update::Pictures(ready));
        }
    }

    async fn preview_attachment(&mut self, path: String) -> openpickle_api::Result<()> {
        let belongs_to_room = self
            .conversation
            .attachments()
            .iter()
            .any(|attachment| attachment.path == path);
        if !belongs_to_room {
            self.send(Update::AttachmentPreview {
                path,
                result: Err("This attachment does not belong to the open conversation.".to_owned()),
            });
            return Ok(());
        }

        let file = picture_file(&format!("thumbnail:{path}"));
        let cached = self.store.cache_path(&file);
        if cached.exists() {
            self.send(Update::AttachmentPreview {
                path,
                result: Ok(cached),
            });
            return Ok(());
        }

        match self.client.get_attachment_thumbnail(&path).await {
            Ok(bytes) => {
                let result = if self.account_active.load(Ordering::Acquire) {
                    self.store
                        .cache_bytes(&file, &bytes)
                        .map(|()| cached)
                        .map_err(|error| format!("Could not save the attachment preview: {error}"))
                } else {
                    Err("The account was signed out before the preview finished.".to_owned())
                };
                self.send(Update::AttachmentPreview { path, result });
                Ok(())
            }
            Err(error) => {
                self.send(Update::AttachmentPreview {
                    path,
                    result: Err(explain(&error)),
                });
                Err(error)
            }
        }
    }

    /// Downloads an attachment of the open conversation, the file itself, into the Downloads
    /// folder. A file may take minutes, so it is fetched beside the conversation, which goes
    /// on meanwhile.
    fn download_attachment(&mut self, path: String) {
        let attachments = self.conversation.attachments();
        let Some(attachment) = attachments.iter().find(|item| item.path == path) else {
            self.send(Update::AttachmentDownloaded {
                path,
                result: Err("This attachment does not belong to the open conversation.".to_owned()),
            });
            return;
        };
        let name = attachment.name.clone();
        let client = self.client.clone();
        let updates = self.updates.clone();
        let account_active = self.account_active.clone();
        let folder = self.download_dir.clone();
        tokio::spawn(async move {
            let result = match client.get_attachment(&path).await {
                Ok(_) if !account_active.load(Ordering::Acquire) => {
                    Err("The account was signed out before the download finished.".to_owned())
                }
                // Writing a large file takes its time too, and the backend has one thread: the
                // write goes to a thread of its own.
                Ok(bytes) => {
                    tokio::task::spawn_blocking(move || save_download(&folder, &name, &bytes))
                        .await
                        .unwrap_or_else(|stopped| Err(std::io::Error::other(stopped)))
                        .map_err(|error| format!("Could not save the file: {error}"))
                }
                Err(error) => Err(explain(&error)),
            };
            updates
                .unbounded_send(Update::AttachmentDownloaded { path, result })
                .ok();
        });
    }

    /// What the disk has under `name`, the first time it is asked for in this run. The window
    /// keeps what it was shown, so later reads only bring the server's answer.
    fn cached_once<T: serde::de::DeserializeOwned>(&mut self, name: &'static str) -> Option<T> {
        self.shown_from_disk
            .insert(name)
            .then(|| self.store.cached(name))
            .flatten()
    }

    async fn resolve_phone_task(&mut self, path: String) -> openpickle_api::Result<()> {
        if !matches!(
            crate::content::LinkTarget::parse(&format!("handoff:{path}")),
            Some(crate::content::LinkTarget::PhoneTask(_))
        ) {
            self.send(Update::PhoneTask {
                path,
                result: Err("This task link is not valid.".into()),
            });
            return Ok(());
        }
        match self.client.get_handoffs().await {
            Ok(handoffs) => {
                let call_id = task_call_id(&handoffs, &path);
                self.send(Update::PhoneTask {
                    path,
                    result: call_id
                        .ok_or_else(|| "No phone transcript is available for this task.".into()),
                });
                Ok(())
            }
            Err(error) => {
                self.send(Update::PhoneTask {
                    path,
                    result: Err(explain(&error)),
                });
                Err(error)
            }
        }
    }

    /// Reads what the handoff list has about a task in the chat, for its card. The task is
    /// asked about again with the others if it still runs, because its card shows what it is
    /// doing and because it may yet place a call; if the list does not have it yet; and if
    /// the reading failed, which must not leave the task without its card.
    async fn load_handoff_card(&mut self, path: String) -> openpickle_api::Result<()> {
        if !valid_handoff_path(&path) {
            return Ok(());
        }
        let read = self.read_handoff_card(&path).await;
        let waiting = &mut self.handoff_cards.waiting;
        if !matches!(read, Ok(false)) && !waiting.iter().any(|(waiting, _)| *waiting == path) {
            waiting.push((path, Instant::now() + CARD_UNLISTED_WAIT));
        }
        self.schedule_handoff_cards();
        read.map(|_| ())
    }

    /// Has the cards read again in a while, if there is something to read: a live call, a
    /// task that is waited for, or a task the character works on.
    fn schedule_handoff_cards(&mut self) {
        let cards = &mut self.handoff_cards;
        if !cards.live.is_empty() || !cards.waiting.is_empty() || cards.active {
            cards
                .next_refresh
                .get_or_insert_with(|| Instant::now() + PHONE_REFRESH);
        }
    }

    /// Reads the handoff list for the cards, and tells the window which task the character
    /// works on.
    async fn read_handoffs(&mut self) -> openpickle_api::Result<()> {
        let handoffs = self.client.get_handoffs().await?;
        let active = active_task_title(&handoffs);
        self.handoff_cards.active = active.is_some();
        self.send(Update::ActiveTask(active));
        self.handoff_cards.handoffs = Some((Instant::now(), handoffs));
        Ok(())
    }

    /// Shows the card of a task: its call, or the task itself. Says whether the task is to be
    /// asked about again.
    async fn read_handoff_card(&mut self, path: &str) -> openpickle_api::Result<bool> {
        let fresh = self
            .handoff_cards
            .handoffs
            .as_ref()
            .is_some_and(|(read_at, _)| read_at.elapsed() < CARD_HANDOFFS_FRESH);
        if !fresh {
            self.read_handoffs().await?;
        }
        let card = self
            .handoff_cards
            .handoffs
            .as_ref()
            .map_or(TaskCard::Unlisted, |(_, handoffs)| {
                task_card(handoffs, path)
            });
        match card {
            // A call that is live is read again as a call, not through the list.
            TaskCard::Call(call_id) => {
                self.read_phone_card(path.to_owned(), call_id).await?;
                Ok(false)
            }
            TaskCard::Task(task) => Ok(self.show_task_card(path, *task)),
            TaskCard::Unlisted => Ok(true),
        }
    }

    /// Shows a task that is not a call in its card, and says whether the task still runs.
    fn show_task_card(&mut self, path: &str, task: openpickle_api::HandoffItem) -> bool {
        let running = task.state == "running";
        self.send(Update::HandoffCard {
            path: path.to_owned(),
            card: crate::protocol::HandoffCard::Task(task),
        });
        running
    }

    async fn read_phone_card(
        &mut self,
        path: String,
        call_id: String,
    ) -> openpickle_api::Result<()> {
        let call = self.client.get_phone_call(&call_id).await?;
        if call.call_id != call_id {
            return Ok(());
        }
        let cards = &mut self.handoff_cards;
        cards.live.retain(|(shown, _)| *shown != path);
        if call.live {
            cards.live.push((path.clone(), call_id));
        }
        self.send(Update::HandoffCard {
            path,
            card: crate::protocol::HandoffCard::Phone(call),
        });
        Ok(())
    }

    /// Reads the live calls of the cards again, and the handoff list for the tasks that are
    /// waited for and for the task the character works on. What has ended is read no more.
    async fn refresh_handoff_cards(&mut self) -> openpickle_api::Result<()> {
        self.handoff_cards.next_refresh = None;
        for (path, call_id) in std::mem::take(&mut self.handoff_cards.live) {
            match self.read_phone_card(path.clone(), call_id.clone()).await {
                Ok(()) => {}
                Err(error) if session_is_gone(&error) => return Err(error),
                // One failed reading does not end the card: it is tried again with the others.
                Err(_) => self.handoff_cards.live.push((path, call_id)),
            }
        }
        let waiting = std::mem::take(&mut self.handoff_cards.waiting);
        if !waiting.is_empty() || self.handoff_cards.active {
            let listed = match self.read_handoffs().await {
                Ok(()) => true,
                Err(error) if session_is_gone(&error) => return Err(error),
                Err(_) => false,
            };
            for (path, until) in waiting {
                let card = self
                    .handoff_cards
                    .handoffs
                    .as_ref()
                    .filter(|_| listed)
                    .map(|(_, handoffs)| task_card(handoffs, &path));
                match card {
                    Some(TaskCard::Call(call_id)) => {
                        match self.read_phone_card(path.clone(), call_id).await {
                            Ok(()) => {}
                            Err(error) if session_is_gone(&error) => return Err(error),
                            Err(_) => self.handoff_cards.waiting.push((path, until)),
                        }
                    }
                    Some(TaskCard::Task(task)) => {
                        if self.show_task_card(&path, *task) {
                            self.handoff_cards.waiting.push((path, until));
                        }
                    }
                    Some(TaskCard::Unlisted) if Instant::now() >= until => {}
                    // Not listed yet, or the list could not be read.
                    _ => self.handoff_cards.waiting.push((path, until)),
                }
            }
        }
        self.schedule_handoff_cards();
        Ok(())
    }

    /// Reads the progress of the task whose sheet is open. A task that still runs is read
    /// again, and so is one whose reading failed.
    async fn refresh_handoff(&mut self) -> openpickle_api::Result<()> {
        let Some(path) = self.handoff_watch.as_ref().map(|watch| watch.path.clone()) else {
            return Ok(());
        };
        let Some(name) = path
            .strip_prefix("handoffs/")
            .filter(|_| valid_handoff_path(&path))
        else {
            self.send(Update::Handoff {
                path,
                result: Err("This task link is not valid.".into()),
            });
            self.handoff_watch = None;
            return Ok(());
        };
        let result = self.client.get_handoff(name).await;
        match result {
            Ok(handoff) => {
                let running = handoff.state == "running";
                self.send(Update::Handoff {
                    path: path.clone(),
                    result: Ok(handoff),
                });
                if let Some(watch) = &mut self.handoff_watch
                    && watch.path == path
                {
                    watch.next_refresh = running.then(|| Instant::now() + PHONE_REFRESH);
                }
                Ok(())
            }
            Err(error) => {
                self.send(Update::Handoff {
                    path,
                    result: Err(explain(&error)),
                });
                if let Some(watch) = &mut self.handoff_watch {
                    watch.next_refresh = Some(Instant::now() + PHONE_REFRESH);
                }
                Err(error)
            }
        }
    }

    async fn load_phone_calls(&mut self) -> openpickle_api::Result<()> {
        let (handoffs, extras) = tokio::try_join!(
            self.client.get_handoffs(),
            self.client.get_extra_conversations(),
        )?;
        let calls = CallSummary::from_screens(&handoffs, &extras);
        self.store.cache("phone-calls.json", &calls).ok();
        self.send(Update::PhoneCalls(Ok(calls)));
        Ok(())
    }

    async fn refresh_phone_call(&mut self) -> openpickle_api::Result<()> {
        let Some(call_id) = self.phone_watch.as_ref().map(|watch| watch.id.clone()) else {
            return Ok(());
        };
        match self.client.get_phone_call(&call_id).await {
            Ok(call) if call.call_id == call_id => {
                if let Some(watch) = &mut self.phone_watch {
                    watch.next_refresh = call.live.then(|| Instant::now() + PHONE_REFRESH);
                    watch.latest = Some(call.clone());
                }
                self.send(Update::PhoneCall {
                    call_id,
                    result: Ok(call),
                });
                Ok(())
            }
            Ok(_) => {
                self.phone_watch = None;
                self.send(Update::PhoneCall {
                    call_id,
                    result: Err("Pickle returned a different call. Open the call again.".into()),
                });
                Ok(())
            }
            Err(error) => {
                if let Some(watch) = &mut self.phone_watch {
                    // A failed first read needs an explicit retry; an already known live call
                    // continues refreshing while its view remains open.
                    watch.next_refresh = watch
                        .latest
                        .as_ref()
                        .filter(|call| call.live)
                        .map(|_| Instant::now() + PHONE_REFRESH);
                }
                self.send(Update::PhoneCall {
                    call_id,
                    result: Err(explain(&error)),
                });
                Err(error)
            }
        }
    }

    async fn end_phone_call(
        &mut self,
        call_id: String,
        action_id: String,
    ) -> openpickle_api::Result<()> {
        let can_end = self
            .phone_watch
            .as_ref()
            .is_some_and(|watch| watch.can_end(&call_id));
        if !can_end || uuid::Uuid::parse_str(&action_id).is_err() {
            self.send(Update::PhoneCallEnded {
                call_id,
                result: Err("This call is not available to end. Refresh its status first.".into()),
            });
            return Ok(());
        }
        let options = openpickle_api::EndPhoneCallOptions {
            action_id: Some(action_id),
            in_call: false,
        };
        match self.client.end_phone_call(&call_id, options).await {
            Ok(reply) if reply.status == "ok" => {
                if let Some(watch) = &mut self.phone_watch {
                    watch.end_requested = true;
                    if let Some(call) = &mut watch.latest {
                        call.hangup_requested = true;
                    }
                    watch.next_refresh = Some(Instant::now());
                }
                self.send(Update::PhoneCallEnded {
                    call_id,
                    result: Ok(()),
                });
                Ok(())
            }
            Ok(_) => {
                self.send(Update::PhoneCallEnded {
                    call_id,
                    result: Err("Pickle did not accept the end-call request.".into()),
                });
                Ok(())
            }
            Err(error) => {
                self.send(Update::PhoneCallEnded {
                    call_id,
                    result: Err(explain(&error)),
                });
                Err(error)
            }
        }
    }

    /// Reads only the captured profile/bootstrap/catalog routes. Session tokens and the
    /// rest of the bootstrap response never enter the UI protocol or disk cache.
    async fn load_profile(&mut self) -> openpickle_api::Result<()> {
        let (account, session, catalog) = tokio::join!(
            self.client.get_profile(None),
            self.client.start_session(),
            self.client.get_screen("connection-catalog", &[]),
        );
        let account = account?;
        let mut data = ProfileData::from_profile(&account);
        let session = session?;
        data.apply_session(&session);
        drop(session);
        match catalog {
            Ok(catalog) => {
                data.set_connections_json(&catalog).ok();
            }
            Err(error) if session_is_gone(&error) => return Err(error),
            Err(_) => {} // Unknown connection count, not zero.
        }
        // The supplied captures contain no ticket-history endpoint. Leave it unavailable.
        self.store.cache("profile-view.json", &data).ok();
        self.profile = Some(data.clone());
        self.send(Update::Profile(Ok(data)));
        Ok(())
    }

    fn show_social(&mut self, snapshot: &SocialSnapshot) {
        let (social, pictures) = convert::social(snapshot, chrono::Utc::now());
        self.send(Update::Social(Ok(social)));
        self.fetch_pictures(pictures);
    }

    fn show_explore(&mut self, screen: ExploreSectionsScreen) {
        let (explore, pictures) = convert::explore(screen);
        self.send(Update::Explore(Ok(explore)));
        self.fetch_pictures(pictures);
    }

    /// Tells the UI where the pictures at these addresses are, now for those in the cache and
    /// later, one by one, for those that have to be downloaded first.
    /// Tells the window which other chat rooms exist, and shows their characters' pictures.
    fn show_rooms(&mut self, rooms: &openpickle_api::SyncResponse) {
        let (infos, pictures) = convert::rooms(&rooms.rooms);
        self.last_rooms = rooms.rooms.clone();
        self.send(Update::Rooms(infos));
        self.fetch_pictures(pictures);
    }

    fn known_room(&self, room_id: &str) -> Option<&openpickle_api::Room> {
        self.last_rooms.iter().find(|room| room.room_id == room_id)
    }

    fn send_block_reason(&self, room_id: &str) -> Option<&'static str> {
        if room_id != self.conversation.room_id {
            Some("Not sent. This conversation is no longer open.")
        } else if !self.known_room(room_id).is_some_and(|room| !room.observer) {
            Some("Not sent. This room is read-only.")
        } else {
            None
        }
    }

    fn reset_room_state(&mut self) {
        self.history_complete = false;
        self.marked_read = 0;
    }

    async fn open_room_and_report(&mut self, requested: String) -> openpickle_api::Result<()> {
        match self.open_room(&requested).await {
            Ok(()) => Ok(()),
            Err(error) => {
                let reason = error.explain();
                let session_error = match error {
                    OpenRoomError::Session(error) => Some(error),
                    OpenRoomError::Visible(_) => None,
                };
                self.send(Update::RoomOpenFailed {
                    room_id: requested,
                    reason,
                });
                session_error.map_or(Ok(()), Err)
            }
        }
    }

    /// Opens a room only when the last `sync` says this account can see it. The target is
    /// caught up before it replaces the visible conversation, so a failed request leaves the
    /// old conversation intact.
    async fn open_room(&mut self, requested: &str) -> Result<(), OpenRoomError> {
        let mut refreshed = false;
        if self.last_rooms.is_empty() || self.character_room.is_empty() {
            self.refresh_rooms().await.map_err(OpenRoomError::Session)?;
            refreshed = true;
        }
        let mut wanted = if requested.is_empty() {
            self.character_room.clone()
        } else {
            requested.to_owned()
        };
        if (wanted.is_empty() || self.known_room(&wanted).is_none()) && !refreshed {
            // A link can name a room created after the last event or reconnect. Refresh the
            // authoritative roster once before deciding that the account cannot open it.
            self.refresh_rooms().await.map_err(OpenRoomError::Session)?;
            wanted = if requested.is_empty() {
                self.character_room.clone()
            } else {
                requested.to_owned()
            };
        }
        if wanted.is_empty() || self.known_room(&wanted).is_none() {
            return Err(OpenRoomError::Visible(
                "This chat room is not available for this account.".to_owned(),
            ));
        }
        if wanted == self.conversation.room_id {
            // Opening a room is also a UI handshake: a no-op still acknowledges the request so
            // the window can clear its pending switch state and restore the composer.
            self.send_update_for(&wanted);
            return Ok(());
        }

        let mut target = Conversation::load(&self.store, &wanted);
        catch_up_conversation(&self.client, &mut target, |_| {})
            .await
            .map_err(OpenRoomError::Session)?;

        self.room_override = (wanted != self.character_room).then(|| wanted.clone());
        self.conversation = target;
        self.reset_room_state();
        self.send_update_for(&wanted);
        self.mark_read().await;
        Ok(())
    }

    /// The conversation update for a validated room: the character room carries the primary
    /// identity; a peer room carries the title and permissions from `sync`.
    fn send_update_for(&self, room_id: &str) {
        let room = self.known_room(room_id);
        let is_character = room.is_some_and(|room| room.kind == "character");
        let can_send = room.is_some_and(|room| !room.observer);
        let (name, avatar) = if is_character {
            (self.display_name(), self.avatar_path())
        } else {
            (
                room.map(|room| {
                    if room.title.trim().is_empty() {
                        room.peer
                            .as_ref()
                            .map(|peer| peer.display_name.clone())
                            .unwrap_or_default()
                    } else {
                        room.title.clone()
                    }
                })
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| self.display_name()),
                None,
            )
        };
        self.send(Update::Conversation {
            room_id: room_id.to_owned(),
            is_character,
            can_send,
            name,
            avatar,
            messages: self.conversation.messages(),
        });
    }

    fn fetch_pictures(&mut self, urls: Vec<String>) {
        self.pictures.forget_failed();
        let ready = self.pictures.fetch(
            urls.into_iter().map(Source::Url),
            &self.store,
            &self.updates,
        );
        if !ready.is_empty() {
            self.send(Update::Pictures(ready));
        }
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

impl Drop for Live {
    fn drop(&mut self) {
        self.account_active.store(false, Ordering::Release);
    }
}

/// Where a picture comes from.
enum Source {
    /// An address the server named, signed or not.
    Url(String),
    /// The thumbnail of a message's attachment, by the attachment's path.
    Thumbnail(PickleClient, String),
}

impl Source {
    /// The picture's name in the cache, the same for every answer that names it.
    fn key(&self) -> String {
        match self {
            Self::Url(url) => convert::picture_key(url).to_owned(),
            // Attachment paths have the day and the name, but no host: they cannot be taken
            // for an address.
            Self::Thumbnail(_, path) => format!("thumbnail:{path}"),
        }
    }
}

/// The pictures the window was told about, or will be when their download is done. A
/// download that fails is forgotten, so the next answer that names the picture tries again.
struct PictureFetches {
    known: HashSet<String>,
    downloads: Arc<Downloads>,
    failed: UnboundedSender<String>,
    failures: tokio::sync::mpsc::UnboundedReceiver<String>,
}

impl PictureFetches {
    fn new(account_active: Arc<AtomicBool>) -> Self {
        let (failed, failures) = unbounded_channel();
        Self {
            known: HashSet::new(),
            downloads: Arc::new(Downloads::new(account_active)),
            failed,
            failures,
        }
    }

    /// Starts the downloads of the pictures that are new, and returns those already cached.
    fn fetch(
        &mut self,
        sources: impl IntoIterator<Item = Source>,
        store: &Store,
        updates: &Updates<Update>,
    ) -> Vec<(String, PathBuf)> {
        let mut ready = Vec::new();
        for source in sources {
            let key = source.key();
            if !self.known.insert(key.clone()) {
                continue;
            }
            let file = picture_file(&key);
            let path = store.cache_path(&file);
            if path.exists() {
                ready.push((key, path));
                continue;
            }
            let (store, updates, downloads, failed) = (
                store.clone(),
                updates.clone(),
                self.downloads.clone(),
                self.failed.clone(),
            );
            tokio::spawn(async move {
                let Ok(_slot) = downloads.slots.acquire().await else {
                    return;
                };
                if !downloads.account_active.load(Ordering::Acquire) {
                    return;
                }
                let bytes = match source {
                    Source::Url(url) => downloads.get(&url).await,
                    Source::Thumbnail(client, path) => client
                        .get_attachment_thumbnail(&path)
                        .await
                        .ok()
                        .map(|bytes| bytes.to_vec()),
                };
                if !downloads.account_active.load(Ordering::Acquire) {
                    return;
                }
                match bytes {
                    Some(bytes) if store.cache_bytes(&file, &bytes).is_ok() => {
                        updates
                            .unbounded_send(Update::Pictures(vec![(key, path)]))
                            .ok();
                    }
                    _ => {
                        failed.send(key).ok();
                    }
                }
            });
        }
        ready
    }

    /// Forgets the pictures whose download failed since the last call.
    fn forget_failed(&mut self) {
        while let Ok(key) = self.failures.try_recv() {
            self.known.remove(&key);
        }
    }
}

/// How many messages the Archive reads back at a time, as the app does.
const ARCHIVE_PAGE: u32 = 250;

/// Downloads pictures, a few at a time.
struct Downloads {
    http: reqwest::Client,
    slots: tokio::sync::Semaphore,
    account_active: Arc<AtomicBool>,
}

impl Downloads {
    fn new(account_active: Arc<AtomicBool>) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            slots: tokio::sync::Semaphore::new(6),
            account_active,
        }
    }

    async fn get(&self, url: &str) -> Option<Vec<u8>> {
        let response = self
            .http
            .get(url)
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        Some(response.bytes().await.ok()?.to_vec())
    }
}

/// The cache file of the picture with this key. The extension tells GPUI how to decode it.
/// The user's Downloads folder. A system that names none gets one in the home folder.
fn download_dir() -> PathBuf {
    dirs::download_dir()
        .or_else(|| dirs::home_dir().map(|home| home.join("Downloads")))
        .unwrap_or_else(std::env::temp_dir)
}

/// Writes a downloaded file into `folder` under its own name, or under the next free one
/// ("report (1).pdf") when a file of that name is there. Only the last part of the name is
/// used and what a file name may not hold is replaced, so that a name from the server cannot
/// lead out of the folder.
fn save_download(folder: &Path, name: &str, bytes: &[u8]) -> std::io::Result<PathBuf> {
    use std::io::Write;

    let name: String = Path::new(name)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("attachment")
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let (stem, extension) = match name.rfind('.') {
        Some(dot) if dot > 0 => name.split_at(dot),
        _ => (name.as_str(), ""),
    };
    std::fs::create_dir_all(folder)?;
    for count in 0.. {
        let file = if count == 0 {
            folder.join(&name)
        } else {
            folder.join(format!("{stem} ({count}){extension}"))
        };
        // Creating the file fails when it exists: two downloads cannot take one name.
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
        {
            Ok(mut created) => {
                if let Err(error) = created.write_all(bytes) {
                    std::fs::remove_file(&file).ok();
                    return Err(error);
                }
                return Ok(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    unreachable!("the count has no end")
}

fn picture_file(key: &str) -> String {
    // FNV-1a: the name has to be the same in every run, which the standard hasher does not
    // promise.
    let hash = key.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    let extension = if key.starts_with("thumbnail:") {
        // The attachment endpoint always returns an image preview, including for PDFs.
        "jpg"
    } else {
        key.rsplit('/')
            .next()
            .and_then(|name| name.rsplit_once('.'))
            .map(|(_, extension)| extension)
            .filter(|extension| {
                extension.len() <= 4 && extension.chars().all(char::is_alphanumeric)
            })
            .unwrap_or("png")
    };
    format!("picture-{hash:016x}.{extension}")
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
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::Mutex,
        thread,
    };

    struct HistoryServer {
        base_url: String,
        address: std::net::SocketAddr,
        requests: Arc<Mutex<Vec<String>>>,
        rooms: Arc<Mutex<Vec<openpickle_api::Room>>>,
        fail_sync_once: Arc<AtomicBool>,
        fail_handoffs_once: Arc<AtomicBool>,
        tasks_ended: Arc<AtomicBool>,
        two_page_history: Arc<AtomicBool>,
        fail_after_once: Arc<Mutex<Option<i64>>>,
        active: Arc<AtomicBool>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl HistoryServer {
        fn start() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let rooms = Arc::new(Mutex::new(vec![
                fixture_room("primary", "character", false),
                fixture_room("peer", "character_pair", false),
                fixture_room("observer", "character_pair", true),
            ]));
            let fail_sync_once = Arc::new(AtomicBool::new(false));
            let fail_handoffs_once = Arc::new(AtomicBool::new(false));
            let tasks_ended = Arc::new(AtomicBool::new(false));
            let two_page_history = Arc::new(AtomicBool::new(false));
            let fail_after_once = Arc::new(Mutex::new(None));
            let active = Arc::new(AtomicBool::new(true));
            let server_requests = requests.clone();
            let server_rooms = rooms.clone();
            let server_fail_sync_once = fail_sync_once.clone();
            let server_fail_handoffs_once = fail_handoffs_once.clone();
            let server_tasks_ended = tasks_ended.clone();
            let server_two_page_history = two_page_history.clone();
            let server_fail_after_once = fail_after_once.clone();
            let server_active = active.clone();
            let thread = thread::spawn(move || {
                while let Ok((mut stream, _)) = listener.accept() {
                    if !server_active.load(Ordering::Acquire) {
                        break;
                    }
                    let mut bytes = [0_u8; 8192];
                    let read = stream.read(&mut bytes).unwrap_or(0);
                    let request = String::from_utf8_lossy(&bytes[..read]);
                    let target = request
                        .lines()
                        .next()
                        .and_then(|line| line.split_whitespace().nth(1))
                        .unwrap_or("/")
                        .to_owned();
                    server_requests.lock().unwrap().push(target.clone());
                    let (status, body) = if target.starts_with("/api/sync") {
                        if server_fail_sync_once.swap(false, Ordering::AcqRel) {
                            ("500 Internal Server Error", "{}".to_owned())
                        } else {
                            (
                                "200 OK",
                                serde_json::json!({
                                    "rooms": server_rooms.lock().unwrap().clone(),
                                    "rev": 1,
                                })
                                .to_string(),
                            )
                        }
                    } else if target.starts_with("/api/screens/handoff?name=") {
                        let done = target.ends_with("done-task");
                        (
                            "200 OK",
                            serde_json::json!({
                                "kind":"handoff",
                                "state": if done { "done" } else { "running" },
                                "title":"Synthetic task",
                                "requested_at_epoch_ms":1.0,
                                "latest_log_text":"Reading a public guide",
                                "latest_log_kind":"web",
                                "handoff_estimate_pct":50,
                                "plan_blocks":[{"type":"prose","markdown":"## Plan\nRead the guide."}],
                                "log_entries":[],
                                "log_nodes":[{"kind":"future_kind","text":"Checking an option"}]
                            })
                            .to_string(),
                        )
                    } else if target == "/api/screens/handoffs"
                        && server_fail_handoffs_once.swap(false, Ordering::AcqRel)
                    {
                        ("500 Internal Server Error", "{}".to_owned())
                    } else if target == "/api/screens/handoffs" {
                        // A task that has just started places its call a moment later: the
                        // list has its call from the second reading on.
                        let readings = server_requests
                            .lock()
                            .unwrap()
                            .iter()
                            .filter(|path| *path == "/api/screens/handoffs")
                            .count();
                        let mut starting = serde_json::json!({
                            "name":"starting-task","title":"Starting call","state":"running",
                            "requested_at_epoch_ms":2.0,"time_text":"Now"
                        });
                        if readings > 1 {
                            starting["phone_call_id"] = "completed-call".into();
                        }
                        let active = if server_tasks_ended.load(Ordering::Acquire) {
                            vec![]
                        } else {
                            vec![starting]
                        };
                        ("200 OK", serde_json::json!({
                            "kind":"handoffs","items":[{
                                "name":"sample-task","title":"Sample call","state":"running",
                                "requested_at_epoch_ms":1.0,"time_text":"Now","phone_call_id":"sample-call"
                            }, {
                                "name":"finished-task","title":"Not a call","state":"done",
                                "subtitle":"Completed summary","requested_at_epoch_ms":1.0,
                                "finished_at_epoch_ms":2.0,"time_text":"Now"
                            }],"handoff_estimate_pct":0.0,"active_items":active,"recent_items":[],
                            "active_count":1,"recent_count":0,"sections":[]
                        }).to_string())
                    } else if target.starts_with("/api/attachments/") {
                        ("200 OK", "the file as it was made".to_owned())
                    } else if let Some(id) = target.strip_prefix("/api/phone-calls/") {
                        let call = phone_snapshot(id, id != "completed-call", false);
                        ("200 OK", serde_json::to_string(&call).unwrap())
                    } else if target == "/api/actions" {
                        ("200 OK", "{\"status\":\"ok\"}".to_owned())
                    } else {
                        let after = target
                            .split('?')
                            .nth(1)
                            .and_then(|query| {
                                query.split('&').find_map(|pair| {
                                    pair.strip_prefix("after_seq=")?.parse::<i64>().ok()
                                })
                            })
                            .unwrap_or(0);
                        let should_fail = {
                            let mut fail_after = server_fail_after_once.lock().unwrap();
                            if *fail_after == Some(after) {
                                *fail_after = None;
                                true
                            } else {
                                false
                            }
                        };
                        if should_fail {
                            ("500 Internal Server Error", "{}".to_owned())
                        } else {
                            let seq = after + 1;
                            (
                                "200 OK",
                                serde_json::json!({
                                    "v": 1,
                                    "items": [{
                                        "role": "assistant",
                                        "id": format!("fixture-{seq}"),
                                        "text": "fixture",
                                        "at": 1.0,
                                        "seq": seq
                                    }],
                                    "reactions": [],
                                    "card_states": [],
                                    "next_after": seq,
                                    "has_more": server_two_page_history.load(Ordering::Acquire)
                                        && after == 0,
                                    "head_seq": seq,
                                    "log_epoch": "fixture",
                                    "projection_version": 1,
                                    "read_seq": 0
                                })
                                .to_string(),
                            )
                        }
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    stream.write_all(response.as_bytes()).ok();
                }
            });
            Self {
                base_url: format!("http://{address}"),
                address,
                requests,
                rooms,
                fail_sync_once,
                fail_handoffs_once,
                tasks_ended,
                two_page_history,
                fail_after_once,
                active,
                thread: Some(thread),
            }
        }

        fn fail_next_sync(&self) {
            self.fail_sync_once.store(true, Ordering::Release);
        }

        fn fail_next_handoffs(&self) {
            self.fail_handoffs_once.store(true, Ordering::Release);
        }

        /// From now on the handoff list has no task that runs.
        fn end_tasks(&self) {
            self.tasks_ended.store(true, Ordering::Release);
        }

        fn fail_second_history_page_once(&self) {
            self.two_page_history.store(true, Ordering::Release);
            *self.fail_after_once.lock().unwrap() = Some(1);
        }

        fn set_rooms(&self, rooms: Vec<openpickle_api::Room>) {
            *self.rooms.lock().unwrap() = rooms;
        }
    }

    impl Drop for HistoryServer {
        fn drop(&mut self) {
            self.active.store(false, Ordering::Release);
            TcpStream::connect(self.address).ok();
            if let Some(thread) = self.thread.take() {
                thread.join().ok();
            }
        }
    }

    fn fixture_room(room_id: &str, kind: &str, observer: bool) -> openpickle_api::Room {
        serde_json::from_value(serde_json::json!({
            "room_id": room_id,
            "kind": kind,
            "title": format!("{kind} fixture"),
            "peer": null,
            "member_count": 1,
            "last_seq": 0,
            "read_seq": 0,
            "role": "member",
            "last_event_at": "2026-01-01T00:00:00Z",
            "observer": observer,
            "unread": 0,
            "muted": false,
            "pinned_at": null,
            "peer_read_seq": null,
            "peer_read_at": null,
            "peer_joined_at": null,
            "request_id": "",
            "request_role": "",
            "extra_conversation_unread": false,
            "log_epoch": "fixture",
            "projection_version": 1
        }))
        .unwrap()
    }

    fn fixture_live(
        name: &str,
        server: &HistoryServer,
    ) -> (Live, futures::channel::mpsc::UnboundedReceiver<Update>) {
        let root = std::env::temp_dir().join(format!(
            "openpickle-test-room-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::at(root).unwrap();
        store.cache("room.json", &"primary").unwrap();
        let client = PickleClient::new(ClientOptions {
            credentials: Credentials::AccessToken("fixture".to_owned()),
            base_url: Some(server.base_url.clone()),
            ..Default::default()
        })
        .unwrap();
        let (updates, received) = futures::channel::mpsc::unbounded();
        let account_active = Arc::new(AtomicBool::new(true));
        let live = Live {
            client,
            store: store.clone(),
            updates,
            conversation: Conversation::load(&store, "primary"),
            identity: Identity::default(),
            profile: None,
            handoff_watch: None,
            phone_watch: None,
            handoff_cards: HandoffCards::default(),
            download_dir: store.cache_path("downloads"),
            active: false,
            history_complete: false,
            character_room: "primary".to_owned(),
            room_override: None,
            last_rooms: vec![
                fixture_room("primary", "character", false),
                fixture_room("peer", "character_pair", false),
                fixture_room("observer", "character_pair", true),
            ],
            account_active: account_active.clone(),
            shown_from_disk: HashSet::new(),
            pictures: PictureFetches::new(account_active),
            marked_read: 0,
        };
        (live, received)
    }

    fn drain(received: &mut futures::channel::mpsc::UnboundedReceiver<Update>) {
        while received.try_recv().is_ok() {}
    }

    #[tokio::test]
    async fn visible_catch_up_publishes_each_page_before_a_later_page_fails() {
        let server = HistoryServer::start();
        server.fail_second_history_page_once();
        let (mut live, mut received) = fixture_live("partial-visible", &server);

        assert!(live.catch_up().await.is_err());
        assert_eq!(live.conversation.synced, 1);
        assert!(matches!(
            received.next().await,
            Some(Update::Upsert { room_id, messages })
                if room_id == "primary" && messages.iter().any(|message| message.seq == 1)
        ));

        live.catch_up().await.unwrap();
        assert_eq!(live.conversation.synced, 2);
        assert!(matches!(
            received.next().await,
            Some(Update::Upsert { room_id, messages })
                if room_id == "primary" && messages.iter().any(|message| message.seq == 2)
        ));
        let requests = server.requests.lock().unwrap();
        assert_eq!(
            requests
                .iter()
                .filter(|target| target.contains("after_seq=1"))
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn failed_staged_room_catch_up_keeps_the_visible_room_unpublished() {
        let server = HistoryServer::start();
        server.fail_second_history_page_once();
        let (mut live, mut received) = fixture_live("partial-staged", &server);

        assert!(matches!(
            live.open_room("peer").await,
            Err(OpenRoomError::Session(_))
        ));
        assert_eq!(live.conversation.room_id, "primary");
        assert!(received.try_recv().is_err());

        live.open_room("peer").await.unwrap();
        assert_eq!(live.conversation.room_id, "peer");
        assert!(matches!(
            received.next().await,
            Some(Update::Conversation { room_id, messages, .. })
                if room_id == "peer"
                    && messages.iter().any(|message| message.seq == 1)
                    && messages.iter().any(|message| message.seq == 2)
        ));
    }

    #[tokio::test]
    async fn startup_waits_for_sync_before_granting_send_access() {
        let server = HistoryServer::start();
        let (mut empty, mut empty_updates) = fixture_live("startup-empty", &server);
        empty.conversation = Conversation::default();
        empty.character_room.clear();
        empty.last_rooms.clear();

        empty.send_startup_conversation();
        assert!(matches!(
            empty_updates.next().await,
            Some(Update::Conversation { room_id, can_send: false, .. }) if room_id.is_empty()
        ));
        empty.find_room().await.unwrap();
        assert_eq!(empty.conversation.room_id, "primary");
        assert_eq!(
            empty.send_block_reason(""),
            Some("Not sent. This conversation is no longer open.")
        );
        assert!(matches!(empty_updates.next().await, Some(Update::Rooms(_))));
        assert!(matches!(
            empty_updates.next().await,
            Some(Update::Conversation { room_id, can_send: true, .. }) if room_id == "primary"
        ));

        // A cached primary id is not enough to grant access either. The first successful sync
        // must publish permissions even though it does not change the id.
        let (mut cached, mut cached_updates) = fixture_live("startup-cached", &server);
        cached.character_room.clear();
        cached.last_rooms.clear();
        cached.send_startup_conversation();
        assert!(matches!(
            cached_updates.next().await,
            Some(Update::Conversation { room_id, can_send: false, .. }) if room_id == "primary"
        ));
        cached.find_room().await.unwrap();
        assert!(matches!(
            cached_updates.next().await,
            Some(Update::Rooms(_))
        ));
        assert!(matches!(
            cached_updates.next().await,
            Some(Update::Conversation { room_id, can_send: true, .. }) if room_id == "primary"
        ));
    }

    #[tokio::test]
    async fn failed_startup_sync_does_not_grant_send_access() {
        let server = HistoryServer::start();
        server.fail_next_sync();
        let (mut live, mut received) = fixture_live("startup-sync-failure", &server);
        live.character_room.clear();
        live.last_rooms.clear();

        live.send_startup_conversation();
        assert!(live.find_room().await.is_err());
        assert!(matches!(
            received.next().await,
            Some(Update::Conversation {
                can_send: false,
                ..
            })
        ));
        assert!(received.try_recv().is_err());
        assert!(live.last_rooms.is_empty());
    }

    #[tokio::test]
    async fn opening_an_unknown_target_refreshes_the_authoritative_roster_once() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("refresh-room", &server);
        server.set_rooms(vec![
            fixture_room("primary", "character", false),
            fixture_room("peer", "character_pair", false),
            fixture_room("new-peer", "character_pair", false),
        ]);

        live.open_room("new-peer").await.unwrap();
        assert_eq!(live.conversation.room_id, "new-peer");
        let conversations: Vec<String> = std::iter::from_fn(|| received.try_recv().ok())
            .filter_map(|update| match update {
                Update::Conversation { room_id, .. } => Some(room_id),
                _ => None,
            })
            .collect();
        assert_eq!(conversations, vec!["new-peer".to_owned()]);
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .any(|target| target.starts_with("/api/sync"))
        );

        drain(&mut received);
        live.open_room_and_report("forged".to_owned())
            .await
            .unwrap();
        assert_eq!(live.conversation.room_id, "new-peer");
        let mut rejected = false;
        let mut conversation_emitted = false;
        while let Ok(update) = received.try_recv() {
            match update {
                Update::RoomOpenFailed { room_id, .. } if room_id == "forged" => rejected = true,
                Update::Conversation { .. } => conversation_emitted = true,
                _ => {}
            }
        }
        assert!(rejected);
        assert!(!conversation_emitted);
    }

    #[tokio::test]
    async fn room_switches_isolate_events_permissions_and_state() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("isolation", &server);

        live.history_complete = true;
        live.marked_read = 41;
        live.open_room("peer").await.unwrap();
        assert_eq!(live.conversation.room_id, "peer");
        assert_eq!(live.room_override.as_deref(), Some("peer"));
        assert!(!live.history_complete);
        assert_eq!(live.marked_read, 0);
        assert_eq!(
            live.store.cached::<String>("room.json").as_deref(),
            Some("primary")
        );
        assert!(matches!(
            received.next().await,
            Some(Update::Conversation {
                room_id,
                is_character: false,
                can_send: true,
                ..
            }) if room_id == "peer"
        ));

        // Unscoped chat and activity belong to the primary character room, never this peer.
        live.handle(PickleEvent::Chat(ChatEvent::User {
            role: "user".to_owned(),
            id: "unscoped".to_owned(),
            text: "fixture".to_owned(),
            at: 2.0,
            seq: 99,
            channel: "fixture".to_owned(),
            prev_seq: None,
        }))
        .await
        .unwrap();
        live.handle(PickleEvent::Chat(ChatEvent::Pending { on: true }))
            .await
            .unwrap();
        assert!(!live.conversation.items.contains_key("unscoped"));
        assert!(received.try_recv().is_err());

        // A scoped room event catches up the room named by the event.
        let mut peer = fixture_room("peer", "character_pair", false);
        peer.last_seq = 2;
        live.handle(PickleEvent::Room(Box::new(RoomEvent::Advanced {
            room_id: "peer".to_owned(),
            last_seq: 2,
            room: peer,
        })))
        .await
        .unwrap();
        assert_eq!(live.conversation.synced, 2);
        assert!(matches!(
            received.next().await,
            Some(Update::Upsert { room_id, messages })
                if room_id == "peer" && messages.iter().any(|message| message.seq == 2)
        ));

        // Empty OpenRoom returns to the character conversation and resets per-room state.
        live.history_complete = true;
        live.marked_read = 87;
        live.open_room("").await.unwrap();
        assert_eq!(live.conversation.room_id, "primary");
        assert!(live.room_override.is_none());
        assert!(!live.history_complete);
        assert_eq!(live.marked_read, 0);
        assert_eq!(
            live.store.cached::<String>("room.json").as_deref(),
            Some("primary")
        );
        assert!(matches!(
            received.next().await,
            Some(Update::Conversation {
                room_id,
                is_character: true,
                can_send: true,
                ..
            }) if room_id == "primary"
        ));
        live.open_room("").await.unwrap();
        assert!(matches!(
            received.next().await,
            Some(Update::Conversation { room_id, .. }) if room_id == "primary"
        ));

        // A stale queued send cannot move to the newly open room; observers cannot send.
        assert_eq!(
            live.send_block_reason("peer"),
            Some("Not sent. This conversation is no longer open.")
        );
        live.conversation = Conversation::load(&live.store, "observer");
        assert_eq!(
            live.send_block_reason("observer"),
            Some("Not sent. This room is read-only.")
        );

        // Unknown deep-link targets are rejected visibly without changing the open room.
        live.conversation = Conversation::load(&live.store, "primary");
        drain(&mut received);
        live.open_room_and_report("unknown".to_owned())
            .await
            .unwrap();
        assert_eq!(live.conversation.room_id, "primary");
        let mut rejected = false;
        while let Ok(update) = received.try_recv() {
            if matches!(update, Update::RoomOpenFailed { room_id, .. } if room_id == "unknown") {
                rejected = true;
            }
        }
        assert!(rejected);

        let requests = server.requests.lock().unwrap();
        assert!(
            requests
                .iter()
                .any(|target| target.contains("/rooms/peer/history/delta"))
        );
        assert!(requests.iter().any(|target| target.contains("after_seq=1")));
        assert!(
            requests
                .iter()
                .any(|target| target.contains("/rooms/primary/history/delta"))
        );
    }

    #[test]
    fn a_download_keeps_its_name_or_takes_the_next_free_one_and_stays_in_its_folder() {
        let folder =
            std::env::temp_dir().join(format!("openpickle-test-downloads-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        let first = save_download(&folder, "보고서 최종.pdf", b"one").unwrap();
        let second = save_download(&folder, "보고서 최종.pdf", b"two").unwrap();
        assert_eq!(first, folder.join("보고서 최종.pdf"));
        assert_eq!(second, folder.join("보고서 최종 (1).pdf"));
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        // A name from the server cannot lead out of the folder. A name without an extension
        // is counted without one.
        let notes = save_download(&folder, "../../notes", b"x").unwrap();
        assert_eq!(notes, folder.join("notes"));
        assert_eq!(
            save_download(&folder, "notes", b"y").unwrap(),
            folder.join("notes (1)")
        );
        assert_eq!(
            save_download(&folder, "..", b"z").unwrap(),
            folder.join("attachment")
        );
        std::fs::remove_dir_all(&folder).ok();
    }

    #[tokio::test]
    async fn an_attachment_of_the_open_conversation_is_downloaded_under_its_own_name() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("download", &server);
        let item: HistoryItem = serde_json::from_value(serde_json::json!({
            "role": "assistant", "id": "m", "text": "<bubble>여기 있어요</bubble>", "at": 10.0, "seq": 7,
            "inline_files": {"downloads/계획서.md": "attachments/2026-11-05/계획서.md"},
            "file_meta": {"attachments/2026-11-05/계획서.md": {"name": "계획서.md", "size": 23}},
        }))
        .unwrap();
        live.conversation.apply(vec![item], vec![]);

        // A path that no message of the conversation carries is not asked for.
        live.download_attachment("attachments/2026-11-05/other.pdf".into());
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::AttachmentDownloaded { result: Err(_), .. }
        ));
        assert!(server.requests.lock().unwrap().is_empty());

        live.download_attachment("attachments/2026-11-05/계획서.md".into());
        let Some(Update::AttachmentDownloaded { path, result }) = received.next().await else {
            panic!("the download reports how it ended");
        };
        assert_eq!(path, "attachments/2026-11-05/계획서.md");
        let file = result.unwrap();
        assert_eq!(file, live.download_dir.join("계획서.md"));
        assert_eq!(std::fs::read(&file).unwrap(), b"the file as it was made");
        assert_eq!(
            *server.requests.lock().unwrap(),
            ["/api/attachments/attachments/2026-11-05/%EA%B3%84%ED%9A%8D%EC%84%9C.md"]
        );
    }

    #[test]
    fn attachment_thumbnails_are_always_cached_as_jpeg() {
        assert!(picture_file("thumbnail:attachments/fixture/document.pdf").ends_with(".jpg"));
        assert!(picture_file("thumbnail:attachments/fixture/picture.png").ends_with(".jpg"));
    }

    #[test]
    fn a_late_session_rotation_cannot_recreate_a_signed_out_account() {
        let root = std::env::temp_dir().join(format!(
            "openpickle-test-late-rotation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::at(root).unwrap();
        store
            .save_session(&Session {
                login_session_token: "fixture-before".to_owned(),
                device_id: "fixture-device".to_owned(),
            })
            .unwrap();
        store.sign_out().unwrap();
        let active = AtomicBool::new(false);
        let refresh = LoginSessionRefresh {
            login_session_jwt: "fixture-access".to_owned(),
            expires_at: "fixture-expiry".to_owned(),
            login_session_token: "fixture-after".to_owned(),
        };

        save_refresh_if_active(&store, &active, "fixture-device", &refresh).unwrap();
        assert!(store.session().is_none());
    }

    #[tokio::test]
    async fn a_picture_that_could_not_be_downloaded_is_tried_again() {
        let root =
            std::env::temp_dir().join(format!("openpickle-test-pictures-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let store = Store::at(root).unwrap();
        let (updates, _received) = futures::channel::mpsc::unbounded();
        // A port that refuses the connection: the download fails at once.
        let address = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let url = format!("http://{address}/icon.png?sig=1");
        let mut pictures = PictureFetches::new(Arc::new(AtomicBool::new(true)));

        assert!(
            pictures
                .fetch([Source::Url(url.clone())], &store, &updates)
                .is_empty()
        );
        // Asked again while the download runs, the picture is not fetched twice.
        assert!(
            pictures
                .fetch([Source::Url(url.clone())], &store, &updates)
                .is_empty()
        );
        let key = convert::picture_key(&url).to_owned();
        for _ in 0..200 {
            pictures.forget_failed();
            if !pictures.known.contains(&key) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        assert!(
            !pictures.known.contains(&key),
            "the failure was not reported"
        );
        // The next answer that names it starts a new download.
        pictures.fetch([Source::Url(url)], &store, &updates);
        assert!(pictures.known.contains(&key));
    }

    #[test]
    fn a_picture_keeps_its_file_name_and_its_kind() {
        let icon = picture_file("https://img.example/explore/icons/0a1b.png");
        assert_eq!(
            icon,
            picture_file("https://img.example/explore/icons/0a1b.png")
        );
        assert!(icon.starts_with("picture-") && icon.ends_with(".png"));
        assert_ne!(
            icon,
            picture_file("https://img.example/explore/icons/0a1c.png")
        );
        assert!(picture_file("https://img.example/avatars/u1/face.JPG").ends_with(".JPG"));
        // An address without a usable extension is taken to be a PNG.
        assert!(picture_file("https://img.example/avatars/u1").ends_with(".png"));
    }

    #[test]
    fn the_wait_to_reconnect_grows_until_a_stream_stays_open() {
        let seconds = Duration::from_secs;
        let mut backoff = Backoff::default();
        backoff.server_asks(Duration::from_millis(1250));

        // The stream opens and ends at once, again and again.
        let waits: Vec<Duration> = (0..6)
            .map(|_| {
                backoff.stream_ended(seconds(1));
                backoff.next()
            })
            .collect();
        assert_eq!(waits, [2, 4, 8, 16, 30, 30].map(seconds));

        // After a stream that stayed open, the wait starts over with what the server asked for.
        backoff.stream_ended(seconds(120));
        assert_eq!(backoff.next(), Duration::from_millis(1250));
        assert_eq!(backoff.next(), Duration::from_millis(2500));
    }

    #[test]
    fn a_message_announced_again_keeps_what_the_history_said_about_it() {
        let item = |value: serde_json::Value| serde_json::from_value::<HistoryItem>(value).unwrap();
        let mut conversation = Conversation::default();
        // The catch-up read the message with its attachment. The event that announced it was
        // still waiting in the stream, and is handled afterwards.
        conversation.apply(
            vec![item(serde_json::json!({
                "role": "user", "id": "m", "text": "사진 보낼게", "at": 10.0, "seq": 7,
                "attachments": ["attachments/2026-11-16/a.png"],
            }))],
            Vec::new(),
        );
        let announced = serde_json::json!({ "role": "user", "id": "m", "text": "사진 보낼게", "at": 10.0, "seq": 7 });
        assert!(conversation.hear(item(announced)).is_some());
        assert_eq!(
            conversation.items["m"]
                .attachments
                .as_deref()
                .map(<[String]>::len),
            Some(1)
        );

        // A newer announcement changes the text and keeps the rest.
        let edited = serde_json::json!({ "role": "user", "id": "m", "text": "사진 두 장", "at": 10.0, "seq": 9 });
        conversation.hear(item(edited));
        assert_eq!(conversation.items["m"].text.as_deref(), Some("사진 두 장"));
        assert_eq!(
            conversation.items["m"]
                .attachments
                .as_deref()
                .map(<[String]>::len),
            Some(1)
        );

        // One that is older than what is known is ignored.
        let stale = serde_json::json!({ "role": "user", "id": "m", "text": "사진 보낼게", "at": 10.0, "seq": 7 });
        assert!(conversation.hear(item(stale)).is_none());
        assert_eq!(conversation.items["m"].seq, 9);

        // A message heard for the first time is taken as announced.
        let new = serde_json::json!({ "role": "assistant", "id": "n", "text": "<bubble>네!</bubble>", "at": 11.0, "seq": 10 });
        assert_eq!(
            conversation.hear(item(new)).map(|message| message.seq),
            Some(10)
        );
    }

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

    fn phone_snapshot(
        call_id: &str,
        live: bool,
        requested: bool,
    ) -> openpickle_api::PhoneCallTranscript {
        serde_json::from_value(serde_json::json!({
            "kind":"phone_call", "call_id":call_id, "title":"Sample call",
            "subtitle_lead":"", "counterpart_label":"Library", "subtitle_tail":"",
            "started_label":"Today", "live":live, "hangup_requested":requested,
            "footer":"Sample status", "lines":[], "started_at_epoch_ms":1000.0
        }))
        .unwrap()
    }

    #[test]
    fn a_phone_end_requires_the_current_verified_live_call() {
        let mut watch = PhoneWatch::new("sample-call".into());
        assert!(
            !watch.can_end("sample-call"),
            "a cached or unverified call is not enough"
        );
        watch.latest = Some(phone_snapshot("sample-call", true, false));
        assert!(watch.can_end("sample-call"));
        assert!(!watch.can_end("other-call"));
        watch.latest = Some(phone_snapshot("sample-call", true, true));
        assert!(!watch.can_end("sample-call"));
        watch.latest = Some(phone_snapshot("sample-call", false, false));
        assert!(!watch.can_end("sample-call"));
        watch.latest = Some(phone_snapshot("sample-call", true, false));
        watch.end_requested = true;
        assert!(
            !watch.can_end("sample-call"),
            "a lagging poll cannot undo an accepted end request"
        );
    }

    #[test]
    fn phone_ids_preserve_opaque_server_values() {
        assert!(valid_call_id("a1b2c3d4e5f6"));
        assert!(valid_call_id("sample-call_1"));
        assert!(valid_call_id("통화/one?record=two"));
        assert!(!valid_call_id(""));
        assert!(!valid_call_id("  "));
        assert!(!valid_call_id("call\nother"));
    }

    #[tokio::test]
    async fn ending_a_phone_call_needs_a_verified_snapshot_and_sends_one_action() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("phone-actions", &server);
        let id = "sample-call".to_owned();
        let action_id = uuid::Uuid::new_v4().to_string();
        live.phone_watch = Some(PhoneWatch::new(id.clone()));
        live.end_phone_call(id.clone(), action_id.clone())
            .await
            .unwrap();
        assert!(
            server.requests.lock().unwrap().is_empty(),
            "opening a view is not permission to end"
        );
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::PhoneCallEnded { result: Err(_), .. }
        ));

        live.refresh_phone_call().await.unwrap();
        assert!(live.phone_watch.as_ref().unwrap().next_refresh.is_some());
        live.end_phone_call(id.clone(), action_id.clone())
            .await
            .unwrap();
        live.end_phone_call(id, action_id).await.unwrap();
        let requests = server.requests.lock().unwrap().clone();
        assert_eq!(
            requests
                .iter()
                .filter(|path| path.as_str() == "/api/actions")
                .count(),
            1
        );
        // Closing the view prevents refresh; it sends no hangup action.
        live.phone_watch = None;
        live.refresh_phone_call().await.unwrap();
        assert_eq!(*server.requests.lock().unwrap(), requests);

        live.phone_watch = Some(PhoneWatch::new("completed-call".into()));
        live.refresh_phone_call().await.unwrap();
        assert!(live.phone_watch.as_ref().unwrap().next_refresh.is_none());
        live.end_phone_call("completed-call".into(), uuid::Uuid::new_v4().to_string())
            .await
            .unwrap();
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|path| path.as_str() == "/api/actions")
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn handoff_cards_share_the_list_and_refresh_only_while_running() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("handoff-card", &server);
        live.load_handoff_card("handoffs/sample-task".into())
            .await
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::ActiveTask(Some(_))
        ));
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::HandoffCard { path, card: crate::protocol::HandoffCard::Phone(call) }
                if path == "handoffs/sample-task" && call.call_id == "sample-call" && call.live
        ));
        assert!(live.handoff_cards.next_refresh.is_some());

        // A finished task without a call keeps the list's own summary, while an invalid path
        // produces no card and makes no request.
        live.load_handoff_card("handoffs/finished-task".into())
            .await
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::HandoffCard { path, card: crate::protocol::HandoffCard::Task(item) }
                if path == "handoffs/finished-task"
                    && item.title == "Not a call"
                    && item.subtitle.as_deref() == Some("Completed summary")
        ));
        live.load_handoff_card("handoffs/../sample-task".into())
            .await
            .unwrap();
        assert!(received.try_recv().is_err());
        // One reading of the handoff list served the cards drawn together.
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|path| *path == "/api/screens/handoffs")
                .count(),
            1
        );
        live.refresh_handoff_cards().await.unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::HandoffCard { path, .. } if path == "handoffs/sample-task"
        ));
        assert!(server.requests.lock().unwrap().iter().all(|path| {
            path == "/api/screens/handoffs" || path == "/api/phone-calls/sample-call"
        }));
    }

    #[tokio::test]
    async fn the_running_task_is_followed_to_its_end_when_no_card_waits_for_it() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("handoff-active", &server);
        // The card of a task that has ended needs no further reading, but the list has
        // another task that runs.
        live.load_handoff_card("handoffs/finished-task".into())
            .await
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::ActiveTask(Some(title)) if title == "Starting call"
        ));
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::HandoffCard { .. }
        ));
        assert!(live.handoff_cards.waiting.is_empty());
        assert!(live.handoff_cards.next_refresh.is_some());

        // The list is read again for that task, and when it has ended, no more.
        server.end_tasks();
        live.refresh_handoff_cards().await.unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::ActiveTask(None)
        ));
        assert!(received.try_recv().is_err());
        assert!(live.handoff_cards.next_refresh.is_none());
    }

    #[tokio::test]
    async fn a_card_whose_first_reading_failed_is_read_again() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("handoff-card-retry", &server);
        server.fail_next_handoffs();
        assert!(
            live.load_handoff_card("handoffs/sample-task".into())
                .await
                .is_err()
        );
        assert!(received.try_recv().is_err());
        assert_eq!(live.handoff_cards.waiting.len(), 1);
        assert!(live.handoff_cards.next_refresh.is_some());

        // The next reading succeeds: the task gets its card and is waited for no more.
        live.refresh_handoff_cards().await.unwrap();
        let updates: Vec<Update> = std::iter::from_fn(|| received.try_recv().ok()).collect();
        assert!(updates.iter().any(|update| matches!(
            update,
            Update::HandoffCard { path, card: crate::protocol::HandoffCard::Phone(call) }
                if path == "handoffs/sample-task" && call.call_id == "sample-call"
        )));
        assert!(live.handoff_cards.waiting.is_empty());
    }

    #[tokio::test]
    async fn a_running_task_card_refreshes_and_changes_to_a_call_card() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("handoff-card-waits", &server);
        live.load_handoff_card("handoffs/starting-task".into())
            .await
            .unwrap();
        // Each reading of the list also says which task the character works on.
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::ActiveTask(Some(title)) if title == "Starting call"
        ));
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::HandoffCard { path, card: crate::protocol::HandoffCard::Task(item) }
                if path == "handoffs/starting-task" && item.state == "running"
        ));
        assert_eq!(live.handoff_cards.waiting.len(), 1);
        assert!(live.handoff_cards.next_refresh.is_some());

        // The next list reading has the call. The same card changes kind and stops waiting.
        live.refresh_handoff_cards().await.unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::ActiveTask(Some(_))
        ));
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::HandoffCard { path, card: crate::protocol::HandoffCard::Phone(call) }
                if path == "handoffs/starting-task" && call.call_id == "completed-call" && !call.live
        ));
        // The card is waited for no more. The list is still read, for the task it has as
        // running.
        assert!(live.handoff_cards.waiting.is_empty());

        // A task the list does not have is asked about for a while, then no more.
        live.load_handoff_card("handoffs/unknown-task".into())
            .await
            .unwrap();
        assert_eq!(live.handoff_cards.waiting.len(), 1);
        live.refresh_handoff_cards().await.unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::ActiveTask(Some(_))
        ));
        assert_eq!(live.handoff_cards.waiting.len(), 1);
        live.handoff_cards.waiting[0].1 = Instant::now();
        live.refresh_handoff_cards().await.unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::ActiveTask(Some(_))
        ));
        assert!(live.handoff_cards.waiting.is_empty());
        assert!(received.try_recv().is_err());
        assert!(server.requests.lock().unwrap().iter().all(|path| {
            path == "/api/screens/handoffs" || path == "/api/phone-calls/completed-call"
        }));
    }

    #[tokio::test]
    async fn an_open_running_handoff_refreshes_and_a_finished_one_does_not() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("handoff-progress", &server);
        live.handoff_watch = Some(HandoffWatch {
            path: "handoffs/sample task".into(),
            next_refresh: None,
        });
        live.refresh_handoff().await.unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::Handoff { path, result: Ok(handoff) }
                if path == "handoffs/sample task"
                    && handoff.log_nodes[0].kind == "future_kind"
        ));
        assert!(live.handoff_watch.as_ref().unwrap().next_refresh.is_some());
        assert!(
            server
                .requests
                .lock()
                .unwrap()
                .contains(&"/api/screens/handoff?name=sample%20task".to_owned())
        );

        live.handoff_watch = Some(HandoffWatch {
            path: "handoffs/done-task".into(),
            next_refresh: None,
        });
        live.refresh_handoff().await.unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::Handoff { result: Ok(_), .. }
        ));
        assert!(live.handoff_watch.as_ref().unwrap().next_refresh.is_none());
    }

    #[tokio::test]
    async fn a_phone_task_resolves_from_the_exact_authorized_handoff_without_actions() {
        let server = HistoryServer::start();
        let (mut live, mut received) = fixture_live("phone-handoff", &server);
        live.resolve_phone_task("handoffs/sample-task".into())
            .await
            .unwrap();
        assert!(
            matches!(received.try_recv().unwrap(),Update::PhoneTask { path,result:Ok(id) }
            if path=="handoffs/sample-task" && id=="sample-call")
        );
        live.resolve_phone_task("handoffs/sample".into())
            .await
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::PhoneTask { result: Err(_), .. }
        ));
        let before = server.requests.lock().unwrap().len();
        live.resolve_phone_task("handoffs/../sample-task".into())
            .await
            .unwrap();
        assert!(matches!(
            received.try_recv().unwrap(),
            Update::PhoneTask { result: Err(_), .. }
        ));
        let requests = server.requests.lock().unwrap();
        assert_eq!(requests.len(), before);
        assert!(requests.iter().all(|path| path == "/api/screens/handoffs"));
    }
}
