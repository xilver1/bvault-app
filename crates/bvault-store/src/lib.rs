//! bvault-store: the two content stores the pipeline reads and writes.
//!
//! - [`RawStore`] — the mounted music library. Opens raw audio at a
//!   store-relative location the caller was handed (from the raw-file lookup
//!   table), with a traversal guard so a bad location can't escape the root.
//! - [`ArtifactStore`] — the content-addressable analysis store. A track's
//!   artifacts live at a path *derived from its content hash*, so
//!   **presence is the sole truth of "analyzed"**: there is nothing to record
//!   and no lookup table to keep in sync. Writes are atomic-by-marker and
//!   idempotent.
//!
//! Both roots come from config (12-factor); nothing here hardcodes a path. The
//! crate is intentionally free of any rekordbox-specific types — it moves opaque
//! named blobs, keyed by a hash string.

mod artifact;
mod raw;

pub use artifact::ArtifactStore;
pub use raw::RawStore;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// A resolved location pointed outside the store root.
    #[error("location escapes store root: {0}")]
    Traversal(String),

    #[error("not found: {0}")]
    NotFound(String),
}

pub type Result<T> = std::result::Result<T, Error>;