//! Relational metadata types. `Serialize` is derived because the gateway returns
//! these straight over HTTP.

use chrono::{DateTime, Utc};
use serde::Serialize;
use uuid::Uuid;

/// An ingested track — the raw-file lookup row: hash -> location, plus light
/// display metadata. Whether it is *analyzed* is not here (derived from the
/// artifact store).
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Track {
    /// Content hash (hex) — identity and the key everything else joins on.
    pub hash: String,
    /// Store-relative path to the raw audio in the music store.
    pub raw_location: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub added_at: DateTime<Utc>,
}

/// A playlist header. Membership (the hash set) is fetched separately.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Playlist {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// A submission batch — the durable record of an "analyze these playlists"
/// request. `hashes` is the full submitted member set (the progress
/// denominator); `done`/`failed` are derived by the gateway from the artifact
/// store and the jobs dead-letter, never stored here.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Batch {
    pub id: Uuid,
    pub name: Option<String>,
    pub hashes: Vec<String>,
    pub created_at: DateTime<Utc>,
}