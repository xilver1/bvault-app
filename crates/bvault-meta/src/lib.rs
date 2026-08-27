//! rekordbox-meta: the relational model — the raw-file lookup, playlists,
//! membership, and submission batches — plus the queries over them. Owned by the
//! gateway; invisible to the worker.
//!
//! There is deliberately **no** analysis lookup table and **no** stored progress.
//! Whether a hash is analyzed is derived from the artifact store's presence, and
//! a batch's progress is computed on read from the store + the jobs dead-letter.
//! This crate stores intent (what playlists/batches exist, where raw audio
//! lives), never done-ness.

mod model;
mod queries;

pub use model::{Batch, Playlist, SearchResult, Track, User};
pub use queries::Meta;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),

    /// A registration collided with an existing (case-insensitive) username.
    /// Surfaced as its own variant so the gateway can answer 409 instead of a
    /// generic 500 on this expected, user-caused condition.
    #[error("username already taken")]
    UsernameTaken,
}

pub type Result<T> = std::result::Result<T, Error>;
