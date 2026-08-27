//! Shared application state — all cheap to clone.

use std::sync::Arc;

use bvault_meta::Meta;
use bvault_store::{ArtifactStore, RawStore};

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    /// Resolves playlists -> hashes (read-only).
    pub meta: Meta,
    /// Reads analysis.json to render the export.
    pub artifacts: ArtifactStore,
    /// Resolves + streams raw audio during transfer.
    pub raw: RawStore,
    pub config: Arc<Config>,
}
