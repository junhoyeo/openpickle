//! What the UI and the backend say to each other. The backend owns the network and the disk and
//! runs on its own thread; the UI only ever sees these messages.

use std::path::PathBuf;

use crate::model::Message;

/// UI to backend.
#[derive(Debug)]
pub enum Command {
    /// Text a sign-in code to this number (E.164).
    RequestCode(String),
    /// Sign in with the texted code.
    VerifyCode(String),
    /// Send a message to the open conversation.
    Send(String),
    /// The window came to the front (`true`) or went behind (`false`).
    Active(bool),
}

/// Backend to UI.
#[derive(Debug)]
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
    Upsert(Vec<Message>),
    /// Whether the character is working on a reply, and on what when it says.
    Activity(Option<Option<String>>),
    /// The backend stopped over something on this computer and cannot go on. The text says
    /// what happened.
    Failed(String),
}
