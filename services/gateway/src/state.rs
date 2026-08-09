//! Shared application state. Every field is cheap to clone (a pool handle or an
//! `Arc`), so `AppState` is `Clone` and axum hands each handler its own copy.

use std::sync::Arc;

use rekordbox_jobs::Queue;
use rekordbox_meta::Meta;
use rekordbox_store::ArtifactStore;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    /// Playlists, the raw-file lookup, and batches.
    pub meta: Meta,
    /// The analysis job queue.
    pub queue: Queue,
    /// The content-addressable analysis store — the source of truth for
    /// "analyzed" (presence) and thus for batch progress.
    pub artifacts: ArtifactStore,
    pub config: Arc<Config>,
}