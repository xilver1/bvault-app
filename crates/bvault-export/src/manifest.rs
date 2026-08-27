//! The build's input types, plus a re-export of the serialized manifest.
//!
//! The manifest itself (`Manifest`/`ManifestEntry`/`Source`) now lives in the
//! leaf `bvault-manifest` crate, so the CLI/transfer can parse it without
//! pulling this crate (and SQLCipher/OpenSSL). It's re-exported here unchanged,
//! so every server-side call site keeps using `bvault_export::Manifest` etc.

pub use bvault_manifest::{Manifest, ManifestEntry, Source};

/// Input to a build.
pub struct ExportInput<'a> {
    /// `(content hash, store-relative raw location)` for every track in the
    /// export, in the order that defines 1-based PDB id assignment.
    pub tracks: &'a [(String, String)],
    /// Playlists as name -> member hashes (each hash also appears in `tracks`).
    pub playlists: &'a [PlaylistInput],
    /// DJ profile name written to `djprofile.nxs`.
    pub profile_name: &'a str,
}

/// One playlist: a name and its member hashes (unordered set).
pub struct PlaylistInput {
    pub name: String,
    pub hashes: Vec<String>,
}
