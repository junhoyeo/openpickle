//! What the UI and the backend say to each other. The backend owns the network and the disk and
//! runs on its own thread; the UI only ever sees these messages.

use crate::phone::CallSummary;
use crate::profile::ProfileData;
use openpickle_api::PhoneCallTranscript;
use std::path::PathBuf;

use crate::model::{Attachment, Explore, Message, RoomInfo, SearchResult, Social};

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
    /// Open/refresh one transcript, or stop watching when None. Closing never hangs up.
    WatchPhoneCall(Option<String>),
    /// Explicitly confirmed request to end the currently watched live call.
    EndPhoneCall { call_id: String, action_id: String },
    /// These offers were on screen.
    OffersShown(Vec<String>),
    /// List the pictures and files of the conversation, reading back to its first message.
    LoadArchive,
    /// Show this chat room instead: a friend's Pickle, or the character's again when empty.
    OpenRoom(String),
    /// Load these pictures of the Archive: the attachments themselves, by their paths.
    LoadAttachments(Vec<String>),
    /// Download the thumbnail (or first-page preview) of one attachment in the open room.
    PreviewAttachment(String),
    /// Search the messages for this text.
    Search(String),
    /// Whether the chat can be read now: the window is in front and shows the chat. Only
    /// then do new messages count as read.
    Active(bool),
    /// The app is quitting: stop after the network call in flight, if there is one.
    Quit,
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
    /// What a search for `query` found, or why it could not be done.
    Found {
        query: String,
        results: Result<Vec<SearchResult>, String>,
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
    /// The backend stopped over something on this computer and cannot go on. The text says
    /// what happened.
    Failed(String),
}
