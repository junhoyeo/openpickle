//! What the UI and the backend say to each other. The backend owns the network and the disk and
//! runs on its own thread; the UI only ever sees these messages.

use crate::phone::CallSummary;
use crate::profile::ProfileData;
use openpickle_api::{HandoffItem, HandoffScreen, PhoneCallTranscript};
use std::path::PathBuf;

use crate::model::{Attachment, Explore, Message, RoomInfo, SearchResult, Social};

/// What the card of a task in the chat shows, once the handoff list has been asked about the
/// task: the task itself, or the call it placed.
#[derive(Clone)]
pub enum HandoffCard {
    Task(HandoffItem),
    Phone(PhoneCallTranscript),
}

/// An intent belongs to one room and one generation of the search screen's requests.
pub struct SearchRequest {
    pub room_id: String,
    pub id: u64,
}

/// UI to backend. Neither side of the protocol can be printed with `{:?}`: a command may hold
/// a phone number or a sign-in code, and both hold message text.
pub enum Command {
    /// Text a sign-in code to this number (E.164).
    RequestCode(String),
    /// Sign in with the texted code.
    VerifyCode(String),
    /// Send a message to the open conversation. The UI shows it already, under this id, which
    /// the server keeps for its copy. `offer` names the Explore offer the message asks for.
    Send {
        room_id: String,
        id: String,
        text: String,
        offer: Option<String>,
    },
    /// Read the friends and the invite link: what was cached first, then the server's answer.
    LoadSocial,
    /// Read the Explore tab's offers, in the same two steps.
    LoadExplore,
    /// Load the account profile and read-only usage snapshot.
    LoadProfile,
    /// List calls placed by Pickle; this does not start a call.
    LoadPhoneCalls,
    /// Resolve a captured subagent handoff path through the fresh authorized handoff list.
    ResolvePhoneTask(String),
    /// Read what the card of the task at this handoff path shows. A task that runs, or its
    /// live call, is read again until it ends.
    LoadHandoffCard(String),
    /// Read the progress of one task, and again while it runs; with None, stop reading.
    WatchHandoff(Option<String>),
    /// Open/refresh one transcript, or stop watching when None. Closing never hangs up.
    WatchPhoneCall(Option<String>),
    /// Explicitly confirmed request to end the currently watched live call.
    EndPhoneCall { call_id: String, action_id: String },
    /// These offers were on screen.
    OffersShown(Vec<String>),
    /// Read the visible room's Archive; `None` cancels its in-flight backward page.
    WatchArchive(Option<String>),
    /// Show this chat room instead: a friend's Pickle, or the character's again when empty.
    OpenRoom(String),
    /// Load these pictures of the Archive: the attachments themselves, by their paths.
    LoadAttachments(Vec<String>),
    /// Download the thumbnail (or first-page preview) of one attachment in the open room.
    PreviewAttachment(String),
    /// Download one attachment of the open room, the file itself, into the Downloads folder.
    DownloadAttachment(String),
    /// Search the messages for this text in the open character room.
    Search {
        request: SearchRequest,
        query: String,
    },
    /// Resolve a search hit that is not in the timeline yet.
    OpenSearchResult { request: SearchRequest, seq: u64 },
    /// Discard queued search work and stop publishing its in-flight result.
    CancelSearch,
    /// Whether the chat can be read now: the window is in front and shows the chat. Only
    /// then do new messages count as read.
    Active(bool),
    /// Forget the session and cached account data on this computer. Does not call the server.
    SignOut,
    /// The app is quitting: stop after the network call in flight, if there is one.
    Quit,
}

#[derive(PartialEq, Eq)]
pub struct RoomMetadata {
    pub room_id: String,
    pub is_character: bool,
    pub can_send: bool,
    pub name: String,
    pub avatar: Option<PathBuf>,
}

/// Backend to UI.
pub enum Update {
    /// There is no login session; show the sign-in screen.
    SignedOut,
    /// The code was texted.
    CodeSent,
    /// Requesting or verifying the code failed; the text is shown to the user.
    SignInFailed(String),
    /// Signed in. Carries the conversation as far as it is known, which at launch is whatever
    /// the disk cache holds.
    Conversation {
        room_id: String,
        is_character: bool,
        can_send: bool,
        name: String,
        avatar: Option<PathBuf>,
        messages: Vec<Message>,
    },
    /// The open room's roster metadata changed without replacing its conversation.
    RoomMetadata(RoomMetadata),
    /// The character's name or picture changed (or was just downloaded).
    Identity {
        name: String,
        avatar: Option<PathBuf>,
    },
    /// New or changed messages, matched to known ones by id.
    Upsert {
        room_id: String,
        messages: Vec<Message>,
    },
    /// The message the UI sent under this id did not reach the server. The text says why.
    SendFailed {
        room_id: String,
        id: String,
        reason: String,
    },
    /// What this search found, or why it could not be done.
    Found {
        request: SearchRequest,
        results: Result<Vec<SearchResult>, String>,
    },
    /// The context to apply before revealing an uncached hit, or why it could not be opened.
    SearchResultOpened {
        request: SearchRequest,
        seq: u64,
        result: Result<Vec<Message>, String>,
    },
    /// The friends and the invite link, or why they could not be read.
    Social(Result<Social, String>),
    /// The Explore tab's offers, or why they could not be read.
    Explore(Result<Explore, String>),
    /// Cache-safe profile and usage fields, never the complete session response.
    Profile(Result<ProfileData, String>),
    /// Phone-call summaries for the list and drawer.
    PhoneCalls(Result<Vec<CallSummary>, String>),
    PhoneTask {
        path: String,
        result: Result<String, String>,
    },
    /// What the card of the task at this handoff path shows.
    HandoffCard { path: String, card: HandoffCard },
    /// The title of the task the character works on now, or that there is none.
    ActiveTask(Option<String>),
    /// A reading of the progress of the task at this handoff path, or why there is none.
    Handoff {
        path: String,
        result: Result<HandoffScreen, String>,
    },
    /// A snapshot or failure of the requested call, never an unscoped update.
    PhoneCall {
        call_id: String,
        result: Result<PhoneCallTranscript, String>,
    },
    /// The confirmed end request completed or failed.
    PhoneCallEnded {
        call_id: String,
        result: Result<(), String>,
    },
    /// Pictures that are on disk now, by their key.
    Pictures(Vec<(String, PathBuf)>),
    /// The chat rooms other than the character's, newest activity first.
    Rooms(Vec<RoomInfo>),
    /// The pictures and files of the conversation, newest first. `complete` says whether the
    /// whole history was read; a list that is not complete grows as older pages arrive.
    Archive {
        room_id: String,
        attachments: Vec<Attachment>,
        complete: bool,
    },
    /// Whether the character is working on a reply, and on what when it says.
    Activity {
        room_id: String,
        activity: Option<Option<String>>,
    },
    /// Switching rooms failed. The current conversation remains open.
    RoomOpenFailed { room_id: String, reason: String },
    /// A requested attachment thumbnail (or first-page preview) is ready or failed.
    AttachmentPreview {
        path: String,
        result: Result<PathBuf, String>,
    },
    /// An attachment was downloaded into this file, or why it was not.
    AttachmentDownloaded {
        path: String,
        result: Result<PathBuf, String>,
    },
    /// The backend stopped over something on this computer and cannot go on. The text says
    /// what happened.
    Failed(String),
}
