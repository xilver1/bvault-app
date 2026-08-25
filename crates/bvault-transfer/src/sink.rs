//! Where an export's bytes land: a small backend abstraction so the reconcile
//! loop is oblivious to *how* a USB is written.
//!
//! Two backends:
//! - [`UsbSink::Fs`] — a normal mounted filesystem (desktop, or `--path`). Uses
//!   the write-tmp-then-rename+fsync dance for crash safety.
//! - [`UsbSink::Saf`] — Android's Storage Access Framework, driven by the
//!   `termux-saf-*` utilities from the `termux-api` package. On a phone Termux
//!   cannot touch a plugged-in USB through the normal filesystem at all; every
//!   directory create, delete and file write goes through a `termux-saf-*`
//!   subprocess against a persisted *tree URI*.
//!
//! ## SAF command contract
//! All `termux-saf-*` calls here take `(<tree-uri> <relative-path>)`, where the
//! relative path is `/`-separated and rooted at the granted tree. This matches
//! the utility set exposed once `pkg install termux-api` is present. The command
//! strings are centralised in [`saf`] so a signature tweak is a one-line change.
//!
//! SAF has no atomic rename and no cheap stat, so the SAF backend writes
//! straight to the final path (removing any stale file first) and reports "file
//! absent" for every skip check — a phone re-writes the tree every export, which
//! is correct if slower than the desktop skip-by-size path.

use std::collections::HashSet;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;

use anyhow::{bail, Context, Result};
use tokio::fs::{self, File};
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::process::{Child, ChildStdin, Command};

/// The resolved write target for an export.
#[derive(Debug, Clone)]
pub enum UsbTarget {
    /// A normal mounted path — a desktop USB mount, or a `--path` override.
    Fs { root: PathBuf },
    /// An Android SAF tree, addressed by its persisted tree URI. All I/O shells
    /// out to `termux-saf-*`.
    Saf { tree_uri: String },
}

/// A write backend built from a [`UsbTarget`].
pub enum UsbSink {
    Fs(FsSink),
    Saf(SafSink),
}

impl UsbSink {
    pub fn new(target: UsbTarget) -> Self {
        match target {
            UsbTarget::Fs { root } => UsbSink::Fs(FsSink { root }),
            UsbTarget::Saf { tree_uri } => UsbSink::Saf(SafSink {
                tree_uri,
                ensured: Mutex::new(HashSet::new()),
            }),
        }
    }

    /// Recursively ensure a `/`-separated directory exists. `rel` empty = no-op.
    pub async fn ensure_dir(&self, rel: &str) -> Result<()> {
        match self {
            UsbSink::Fs(s) => s.ensure_dir(rel).await,
            UsbSink::Saf(s) => s.ensure_dir(rel).await,
        }
    }

    /// Size of an existing file at `rel`, or `None` if it isn't there. The SAF
    /// backend always answers `None` (no reliable cheap stat), so the reconcile
    /// loop never skips on a phone.
    pub async fn file_len(&self, rel: &str) -> Result<Option<u64>> {
        match self {
            UsbSink::Fs(s) => s.file_len(rel).await,
            UsbSink::Saf(_) => Ok(None),
        }
    }

    /// Begin writing the file at `rel`; stream bytes into the returned writer,
    /// then `commit()` (or `abort()` on error). The parent directory must
    /// already exist — call [`ensure_dir`](Self::ensure_dir) first.
    pub async fn create(&self, rel: &str) -> Result<UsbFileWriter> {
        match self {
            UsbSink::Fs(s) => Ok(UsbFileWriter::Fs(s.create(rel).await?)),
            UsbSink::Saf(s) => Ok(UsbFileWriter::Saf(s.create(rel).await?)),
        }
    }
}

/// An in-flight file write. Feed it with `write_all`, then `commit` or `abort`.
pub enum UsbFileWriter {
    Fs(FsWriter),
    Saf(SafWriter),
}

impl UsbFileWriter {
    pub async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        match self {
            UsbFileWriter::Fs(w) => w.write_all(buf).await,
            UsbFileWriter::Saf(w) => w.write_all(buf).await,
        }
    }

    /// Durably finalise the file (fs: flush+fsync+rename; SAF: close+wait).
    pub async fn commit(self) -> Result<()> {
        match self {
            UsbFileWriter::Fs(w) => w.commit().await,
            UsbFileWriter::Saf(w) => w.commit().await,
        }
    }

    /// Discard a partial write.
    pub async fn abort(self) {
        match self {
            UsbFileWriter::Fs(w) => w.abort().await,
            UsbFileWriter::Saf(w) => w.abort().await,
        }
    }
}

// --- Filesystem backend -----------------------------------------------------

pub struct FsSink {
    root: PathBuf,
}

impl FsSink {
    async fn ensure_dir(&self, rel: &str) -> Result<()> {
        if rel.is_empty() {
            return Ok(());
        }
        fs::create_dir_all(self.root.join(rel))
            .await
            .context("creating directories on USB")
    }

    async fn file_len(&self, rel: &str) -> Result<Option<u64>> {
        match fs::metadata(self.root.join(rel)).await {
            Ok(m) => Ok(Some(m.len())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).context("stat on USB"),
        }
    }

    async fn create(&self, rel: &str) -> Result<FsWriter> {
        let dest = self.root.join(rel);
        // Append a suffix rather than replacing the extension, so sibling files
        // that share a stem (ANLZ0000.DAT/.EXT/.2EX) never collide on the tmp.
        let mut tmp = dest.clone().into_os_string();
        tmp.push(".bvault-tmp");
        let tmp = PathBuf::from(tmp);
        let file = BufWriter::new(
            File::create(&tmp)
                .await
                .with_context(|| format!("creating tmp file {}", tmp.display()))?,
        );
        Ok(FsWriter { file, tmp, dest })
    }
}

pub struct FsWriter {
    file: BufWriter<File>,
    tmp: PathBuf,
    dest: PathBuf,
}

impl FsWriter {
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        self.file.write_all(buf).await.context("write error")
    }

    async fn commit(mut self) -> Result<()> {
        self.file.flush().await.context("flush error")?;
        let inner = self.file.into_inner();
        inner.sync_all().await.context("fsync error")?;
        fs::rename(&self.tmp, &self.dest)
            .await
            .context("rename error")
    }

    async fn abort(self) {
        drop(self.file);
        let _ = fs::remove_file(&self.tmp).await;
    }
}

// --- Termux SAF backend -----------------------------------------------------

pub struct SafSink {
    tree_uri: String,
    /// Directory prefixes already `mkdir`-ed this run, so we don't re-spawn a
    /// subprocess for every file that lands in the same folder.
    ensured: Mutex<HashSet<String>>,
}

impl SafSink {
    async fn ensure_dir(&self, rel: &str) -> Result<()> {
        if rel.is_empty() {
            return Ok(());
        }
        // Build cumulative prefixes: a/b/c -> [a, a/b, a/b/c]. mkdir each,
        // tolerating "already exists", so we work whether or not the wrapper
        // creates intermediates itself.
        let mut prefix = String::new();
        for seg in rel.split('/').filter(|s| !s.is_empty()) {
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(seg);
            {
                let mut set = self.ensured.lock().unwrap();
                if set.contains(&prefix) {
                    continue;
                }
                set.insert(prefix.clone());
            }
            // Idempotent: a mkdir over an existing dir is a no-op we ignore.
            let _ = saf::mkdir(&self.tree_uri, &prefix).await;
        }
        Ok(())
    }

    async fn create(&self, rel: &str) -> Result<SafWriter> {
        // Guarantee clean overwrite semantics regardless of how the wrapper
        // treats an existing target: drop any stale file first.
        let _ = saf::remove(&self.tree_uri, rel).await;

        let mut child = Command::new("termux-saf-write")
            .arg(&self.tree_uri)
            .arg(rel)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning termux-saf-write (is the termux-api package installed?)")?;
        let stdin = child
            .stdin
            .take()
            .context("termux-saf-write gave no stdin handle")?;
        Ok(SafWriter {
            child,
            stdin: Some(stdin),
            tree_uri: self.tree_uri.clone(),
            rel: rel.to_string(),
        })
    }
}

pub struct SafWriter {
    child: Child,
    stdin: Option<ChildStdin>,
    tree_uri: String,
    rel: String,
}

impl SafWriter {
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("termux-saf-write stdin already closed")?;
        stdin
            .write_all(buf)
            .await
            .context("piping to termux-saf-write")
    }

    async fn commit(mut self) -> Result<()> {
        // Close stdin so termux-saf-write sees EOF and finalises the document.
        if let Some(mut stdin) = self.stdin.take() {
            stdin.shutdown().await.context("closing termux-saf-write stdin")?;
        }
        let out = self
            .child
            .wait_with_output()
            .await
            .context("waiting on termux-saf-write")?;
        if !out.status.success() {
            let err = String::from_utf8_lossy(&out.stderr);
            bail!("termux-saf-write failed for {}: {}", self.rel, err.trim());
        }
        Ok(())
    }

    async fn abort(mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        let _ = saf::remove(&self.tree_uri, &self.rel).await;
    }
}

/// Thin wrappers over the `termux-saf-*` CLI, plus the selection/cleanup helpers
/// the client needs before a transfer. Every call is `(<tree-uri> <rel-path>)`.
pub mod saf {
    use anyhow::{bail, Context, Result};
    use serde::Deserialize;
    use tokio::process::Command;

    /// One persisted SAF tree, as reported by `termux-saf-dirs`.
    #[derive(Debug, Clone, Deserialize)]
    pub struct SafDir {
        pub name: String,
        pub uri: String,
    }

    /// A child entry, as reported by `termux-saf-ls`.
    #[derive(Debug, Clone, Deserialize)]
    struct SafEntry {
        #[allow(dead_code)]
        name: String,
    }

    /// True when we're running inside Termux on Android, where the SAF backend
    /// is the only way to reach a USB.
    pub fn detect() -> bool {
        if std::env::var_os("TERMUX_VERSION").is_some() {
            return true;
        }
        std::env::var("PREFIX")
            .map(|p| p.contains("com.termux"))
            .unwrap_or(false)
    }

    /// List the directory trees the user has already granted (`termux-saf-dirs`).
    pub async fn list_managed_dirs() -> Result<Vec<SafDir>> {
        let out = Command::new("termux-saf-dirs")
            .output()
            .await
            .context("running termux-saf-dirs (is the termux-api package installed?)")?;
        if !out.status.success() {
            bail!(
                "termux-saf-dirs failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let dirs: Vec<SafDir> =
            serde_json::from_slice(&out.stdout).context("parsing termux-saf-dirs JSON")?;
        Ok(dirs)
    }

    /// Open the Android directory picker so the user can grant a tree
    /// (`termux-saf-managedir`). Interactive: inherits the terminal and blocks
    /// until the system dialog is dismissed.
    pub async fn manage_dir() -> Result<()> {
        let status = Command::new("termux-saf-managedir")
            .status()
            .await
            .context("running termux-saf-managedir")?;
        if !status.success() {
            bail!("termux-saf-managedir exited without granting a directory");
        }
        Ok(())
    }

    /// Whether `rel` under `tree_uri` is empty or absent — i.e. safe to treat as
    /// Android-generated junk. Returns `true` for an empty listing (`[]`) and for
    /// a missing directory; `false` if it holds anything.
    pub async fn is_empty(tree_uri: &str, rel: &str) -> Result<bool> {
        let out = Command::new("termux-saf-ls")
            .arg(tree_uri)
            .arg(rel)
            .output()
            .await
            .context("running termux-saf-ls")?;
        if !out.status.success() {
            // A non-existent directory isn't an error worth surfacing here.
            return Ok(true);
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(true);
        }
        match serde_json::from_str::<Vec<SafEntry>>(trimmed) {
            Ok(entries) => Ok(entries.is_empty()),
            // Unparseable output: be conservative and treat as non-empty.
            Err(_) => Ok(false),
        }
    }

    /// Remove `rel` under `tree_uri` (`termux-saf-rm`).
    pub async fn remove(tree_uri: &str, rel: &str) -> Result<()> {
        let out = Command::new("termux-saf-rm")
            .arg(tree_uri)
            .arg(rel)
            .output()
            .await
            .context("running termux-saf-rm")?;
        if !out.status.success() {
            bail!(
                "termux-saf-rm {rel} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// Create `rel` under `tree_uri` (`termux-saf-mkdir`).
    pub(super) async fn mkdir(tree_uri: &str, rel: &str) -> Result<()> {
        let out = Command::new("termux-saf-mkdir")
            .arg(tree_uri)
            .arg(rel)
            .output()
            .await
            .context("running termux-saf-mkdir")?;
        if !out.status.success() {
            bail!(
                "termux-saf-mkdir {rel} failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }
}
