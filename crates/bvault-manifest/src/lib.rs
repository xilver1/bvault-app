//! bvault-manifest: the serialized description of a USB export.
//!
//! This is the *contract* between the server that builds an export and the
//! client that transfers it: the manifest lists every file the USB should hold
//! and where each file's bytes come from, so the reconcile loop never has to
//! parse the `.pdb` to know the target state.
//!
//! It lives in its own leaf crate — serde and nothing else — so the CLI and
//! transfer deserialize it without depending on `bvault-export` (and thus
//! `bvault-core` -> rusqlite/SQLCipher/OpenSSL). `bvault-export` re-exports these
//! types, so server-side code keeps importing them from there unchanged.

use serde::{Deserialize, Serialize};

/// The full description of what the USB should contain — every file, and where
/// its bytes come from.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub entries: Vec<ManifestEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// USB-root-relative path with forward slashes, e.g. `Contents/ab12cd.flac`
    /// or `PIONEER/rekordbox/export.pdb`.
    pub usb_path: String,
    pub size: u64,
    pub source: Source,
}

/// Where an entry's bytes come from during transfer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Source {
    /// A rendered file, present in the build's staging dir at `usb_path`.
    Staging,
    /// Streamed verbatim from the raw music store by content hash — never copied
    /// into staging.
    Raw { hash: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    // The tagged shape the phone will parse when resolving each manifest entry.
    #[test]
    fn source_serializes_tagged() {
        let raw = serde_json::to_string(&Source::Raw {
            hash: "ab12".into(),
        })
        .unwrap();
        assert_eq!(raw, r#"{"kind":"raw","hash":"ab12"}"#);
        let staged = serde_json::to_string(&Source::Staging).unwrap();
        assert_eq!(staged, r#"{"kind":"staging"}"#);
    }
}
