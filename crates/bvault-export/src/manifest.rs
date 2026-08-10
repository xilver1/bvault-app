//! The build's input and its output manifest.
//!
//! The manifest is the whole point of the split between build and transfer: it
//! is the *plan* the reconcile loop converges the USB toward, so the transfer
//! never has to parse the `.pdb` to know what the USB should contain.

use serde::Serialize;

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

/// The full description of what the USB should contain — every file, and where
/// its bytes come from.
#[derive(Debug, Clone, Serialize)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ManifestEntry {
    /// USB-root-relative path with forward slashes, e.g. `Contents/ab12cd.flac`
    /// or `PIONEER/rekordbox/export.pdb`.
    pub usb_path: String,
    pub size: u64,
    pub source: Source,
}

/// Where an entry's bytes come from during transfer.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A rendered file, present in the build's staging dir at `usb_path`.
    Staging,
    /// Streamed verbatim from the raw music store by content hash — never copied
    /// into staging.
    Raw { hash: String },
}