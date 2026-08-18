//! bvault-export: renders a USB export's "brain" — `export.pdb`, the
//! encrypted device library, ANLZ files, and the aux files — from stored
//! analyses, and returns a manifest describing the whole USB tree.
//!
//! Two deliberate boundaries:
//! - **Core is untouched.** This crate only *orchestrates* bvault-core's
//!   transformation functions (`PdbBuilder`, `generate_*_file`,
//!   `build_export_library`); it produces no format bytes itself.
//! - **Audio is not copied.** Only the small rendered files land in staging; the
//!   manifest references raw audio in the music store by hash, so the transfer
//!   streams gigabytes straight from the store rather than duplicating them.
//!
//! It is also the one crate that re-enables core's `device-library` feature
//! (SQLCipher), confining OpenSSL to the export image.

mod build;
mod manifest;

pub use build::build_export;
pub use manifest::{ExportInput, Manifest, ManifestEntry, PlaylistInput, Source};

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("store: {0}")]
    Store(#[from] bvault_store::Error),

    #[error("core: {0}")]
    Core(#[from] bvault_core::Error),

    #[error("decoding a stored analysis: {0}")]
    Decode(String),
}

pub type Result<T> = std::result::Result<T, Error>;