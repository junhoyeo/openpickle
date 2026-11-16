//! Client for the Pickle character API (`https://character-api-prod.picklemem.com`).
//!
//! The wire contract is documented in `ts/README.md`; the TypeScript client in `ts/` is the
//! reference this crate follows.

mod client;
pub mod events;
pub mod types;

pub use client::{
    AccessTokenClaims, Auth, ClientOptions, ClientProfile, Credentials, DEFAULT_BASE_URL,
    LoginSession, PickleClient, RequestOptions, RotateCallback, SendMessageOptions, TokenProvider,
    decode_access_token, random_nonce,
};
pub use events::{PickleEvent, SseFrame, SseParser};
pub use reqwest::Method;
pub use types::*;

/// A non-2xx response.
#[derive(Clone, Debug)]
pub struct ApiError {
    pub method: Method,
    pub path: String,
    pub status: u16,
    /// The server's reason code (`{"detail": "..."}`), when it sent one.
    pub detail: Option<String>,
    /// The response body as received.
    pub body: bytes::Bytes,
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {} failed with {}",
            self.method, self.path, self.status
        )?;
        if let Some(detail) = &self.detail {
            write!(f, ": {detail}")?;
        }
        Ok(())
    }
}

impl std::error::Error for ApiError {}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Api(Box<ApiError>),
    #[error("this client is signed out; give it credentials first")]
    SignedOut,
    #[error("HTTP request failed: {0}")]
    Http(#[from] reqwest::Error),
    #[error("{method} {path}: unexpected response body: {source}")]
    Decode {
        method: Method,
        path: String,
        source: serde_json::Error,
    },
    #[error("could not encode the request body: {0}")]
    Encode(serde_json::Error),
    #[error("not a JWT")]
    InvalidToken,
    #[error("value is not allowed in an HTTP header")]
    InvalidHeader,
    /// The rotation callback failed. The new refresh token was not stored, so the client does
    /// not use the access token it came with; the next request offers it to the callback again.
    #[error("storing the rotated login session token failed: {0}")]
    Rotate(std::io::Error),
    /// The task that refreshes the login session stopped before it finished.
    #[error("the login session refresh did not finish")]
    RefreshAborted,
}

impl Error {
    /// The API error, if the server answered with a non-2xx status.
    pub fn api(&self) -> Option<&ApiError> {
        match self {
            Self::Api(error) => Some(error),
            _ => None,
        }
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
