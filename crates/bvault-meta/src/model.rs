//! Relational metadata types. `Serialize` is derived because the gateway returns
//! these straight over HTTP.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
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

/// A registered user, minus anything secret. The `password_hash` never leaves
/// the query layer — credential checks happen inside [`crate::Meta`], and this
/// type is what the gateway is allowed to serialize out.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct User {
    pub id: Uuid,
    pub username: String,
    pub created_at: DateTime<Utc>,
}

/// A playlist header. Membership (the hash set) is fetched separately.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
pub struct Playlist {
    pub id: Uuid,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub user_id: Option<Uuid>,
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
    pub user_id: Option<Uuid>,
}

/// A search result returned from the yt-dlp discovery service.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub title: String,
    pub url: String,
    pub duration_secs: Option<u32>,   // Option: yt-dlp returns null for live streams
    pub uploader: String,
    pub thumbnail: Option<String>,    // for future Android UI
    pub video_id: String,
}