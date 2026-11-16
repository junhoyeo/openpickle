//! Client for the Pickle character API (`https://character-api-prod.picklemem.com`).
//!
//! The wire contract is documented in `ts/README.md`; the TypeScript client in `ts/` is the
//! reference this crate follows.

pub mod events;
pub mod types;

pub use events::{PickleEvent, SseFrame, SseParser};
pub use types::*;
