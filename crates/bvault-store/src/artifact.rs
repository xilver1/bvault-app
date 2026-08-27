//! The content-addressable analysis store.
//!
//! A track's artifacts (its serialized analysis, ANLZ files, ...) live under a
//! directory *derived from the content hash*. Writing is atomic **by marker**:
//! every file is written and fsynced, then a `.complete` marker is written last.
//! So [`ArtifactStore::exists`] is true only for a fully-written, durable bundle
//! — and that presence is the system's entire definition of "analyzed".
//!
//! Crash behaviour is deliberately biased to safety: if a write is interrupted
//! before the marker, `exists` reports false and the (idempotent) job simply
//! re-runs. The failure direction is always "redo", never "falsely analyzed",
//! which is why no directory fsync is needed here.

use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;

use tracing::debug;

use crate::{Error, Result};

const MARKER: &str = ".complete";

/// Handle to the analysis store. `root` comes from config; artifact locations
/// are derived from the hash, so nothing anywhere records them.
#[derive(Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Directory holding `hash`'s artifacts, sharded by leading hex chars so no
    /// single directory grows unbounded.
    fn bundle_dir(&self, hash: &str) -> PathBuf {
        let a = hash.get(0..2).unwrap_or("00");
        let b = hash.get(2..4).unwrap_or("00");
        self.root.join(a).join(b).join(hash)
    }

    /// True iff a fully-written bundle exists for `hash`. This is the sole
    /// "analyzed" signal the gateway filters on — no DB flag involved.
    pub fn exists(&self, hash: &str) -> bool {
        self.bundle_dir(hash).join(MARKER).is_file()
    }

    /// Write a bundle of `(name, bytes)` artifacts for `hash` — atomic by marker
    /// and idempotent: if the bundle already exists this is a no-op, because
    /// analysis is deterministic and never invalidated.
    pub fn put(&self, hash: &str, files: &[(&str, &[u8])]) -> Result<()> {
        if self.exists(hash) {
            debug!(hash, "artifact bundle already present; skipping write");
            return Ok(());
        }
        let dir = self.bundle_dir(hash);
        fs::create_dir_all(&dir)?;

        // Write + fsync every artifact before the marker, so the marker can only
        // appear once the bytes behind it are durable.
        for (name, bytes) in files {
            let mut out = File::create(dir.join(name))?;
            out.write_all(bytes)?;
            out.sync_all()?;
        }

        // Marker last — its presence means "complete".
        File::create(dir.join(MARKER))?.sync_all()?;
        debug!(hash, artifacts = files.len(), "wrote artifact bundle");
        Ok(())
    }

    /// Read one named artifact from `hash`'s bundle.
    pub fn get(&self, hash: &str, name: &str) -> Result<Vec<u8>> {
        fs::read(self.bundle_dir(hash).join(name))
            .map_err(|_| Error::NotFound(format!("{hash}/{name}")))
    }

    /// Open one named artifact for streaming (e.g. copying ANLZ at export time).
    pub fn open(&self, hash: &str, name: &str) -> Result<File> {
        File::open(self.bundle_dir(hash).join(name))
            .map_err(|_| Error::NotFound(format!("{hash}/{name}")))
    }

    /// List the artifact names in `hash`'s bundle (excludes the marker).
    pub fn list(&self, hash: &str) -> Result<Vec<String>> {
        let mut names = Vec::new();
        let dir = self.bundle_dir(hash);
        for entry in fs::read_dir(&dir).map_err(|_| Error::NotFound(hash.to_string()))? {
            let name = entry?.file_name().to_string_lossy().into_owned();
            if name != MARKER {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn put_exists_get_list_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let store = ArtifactStore::new(tmp.path());
        let hash = "a1b2c3d4";

        assert!(!store.exists(hash));
        store
            .put(
                hash,
                &[("analysis.json", b"{}"), ("ANLZ0000.DAT", b"\x00\x01")],
            )
            .unwrap();

        assert!(store.exists(hash));
        assert_eq!(store.get(hash, "analysis.json").unwrap(), b"{}");
        assert_eq!(
            store.list(hash).unwrap(),
            vec!["ANLZ0000.DAT", "analysis.json"]
        );
    }

    #[test]
    fn put_is_idempotent() {
        let tmp = TempDir::new().unwrap();
        let store = ArtifactStore::new(tmp.path());
        store.put("ff00", &[("a", b"1")]).unwrap();
        // A second put with different bytes is a no-op: the bundle already exists.
        store.put("ff00", &[("a", b"2")]).unwrap();
        assert_eq!(store.get("ff00", "a").unwrap(), b"1");
    }
}
