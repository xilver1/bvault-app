//! The raw music store — read-only access to ingested audio.

use std::fs::File;
use std::path::PathBuf;

use crate::{Error, Result};

/// Read-only handle to the mounted music library. `root` comes from config
/// (e.g. the NFS PV mount point), never hardcoded.
#[derive(Clone)]
pub struct RawStore {
    root: PathBuf,
}

impl RawStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Resolve a store-relative `location` to an absolute path, guaranteeing it
    /// stays under the store root (rejects `..` and symlink escapes via
    /// canonicalization). The target must exist — raw files are only ever read.
    pub fn resolve(&self, location: &str) -> Result<PathBuf> {
        let canonical = self
            .root
            .join(location)
            .canonicalize()
            .map_err(|_| Error::NotFound(location.to_string()))?;
        let root = self.root.canonicalize()?;
        if !canonical.starts_with(&root) {
            return Err(Error::Traversal(location.to_string()));
        }
        Ok(canonical)
    }

    /// Open a raw audio file for reading. The returned `File` is a Symphonia
    /// `MediaSource`, so the worker can box it straight into the analyzer.
    pub fn open(&self, location: &str) -> Result<File> {
        Ok(File::open(self.resolve(location)?)?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn resolves_within_root() {
        let tmp = TempDir::new().unwrap();
        File::create(tmp.path().join("song.flac"))
            .unwrap()
            .write_all(b"audio")
            .unwrap();

        let store = RawStore::new(tmp.path());
        assert!(store.resolve("song.flac").unwrap().ends_with("song.flac"));
    }

    #[test]
    fn rejects_escape() {
        let tmp = TempDir::new().unwrap();
        let store = RawStore::new(tmp.path());
        // Escaping the root fails, whether by non-existence or traversal.
        assert!(store.resolve("../secret").is_err());
    }
}