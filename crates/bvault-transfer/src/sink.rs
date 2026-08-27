//! Where an export's bytes land: a small backend abstraction so the reconcile
//! loop is oblivious to *how* a USB is written.
//!
//! Two backends:
//! - [`UsbSink::Fs`] — a normal mounted filesystem (desktop, or `--path`). Uses
//!   the write-tmp-then-rename+fsync dance for crash safety.
//! - [`UsbSink::Saf`] — Android's Storage Access Framework, driven by the
//!   `termux-saf-*` utilities from the `termux-api` package. On a phone Termux
//!   cannot touch a plugged-in USB through the normal filesystem at all.
//!
//! ## SAF command contract (important)
//! The `termux-saf-*` utilities are **URI-addressed and single-level**. There is
//! no path resolution: the second argument is a *display name*, not a relative
//! path, and a `/` in it is sanitised to `_` by Android's DocumentsContract.
//! Every document (directory or file) has its own opaque `content://` URI, and
//! the only way to reach a nested path is to walk it one segment at a time:
//!
//! - `termux-saf-ls   <dir-uri>`            -> JSON of the dir's children, each
//!                                            with its own `uri`.
//! - `termux-saf-mkdir <parent-uri> <name>` -> create one child directory.
//! - `termux-saf-write <parent-uri> <name>` -> create+stream a file from stdin.
//! - `termux-saf-rm    <parent-uri> <name>` -> remove one child.
//!
//! So the SAF backend keeps a cache of `relative-dir-path -> uri`, seeded with
//! `"" -> tree_uri`, and resolves each directory by `ls`-ing the parent and
//! `mkdir`-ing the missing segment. Files are written into the leaf directory's
//! URI by name.
//!
//! SAF has no atomic rename and no cheap stat, so the backend writes straight to
//! the final name (removing any stale one first, since SAF would otherwise
//! create a `name (1)` duplicate) and reports "file absent" for every skip check
//! — a phone re-writes the tree every export, which is correct if slower than
//! the desktop skip-by-size path.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::Mutex;

use anyhow::{anyhow, bail, Context, Result};
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt, BufWriter};
use tokio::process::{Child, ChildStderr, ChildStdin, Command};

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
            UsbTarget::Saf { tree_uri } => {
                let mut cache = HashMap::new();
                cache.insert(String::new(), tree_uri.clone());
                UsbSink::Saf(SafSink {
                    tree_uri,
                    dir_uris: Mutex::new(cache),
                })
            }
        }
    }

    /// Recursively ensure a `/`-separated directory exists. `rel` empty = no-op.
    pub async fn ensure_dir(&self, rel: &str) -> Result<()> {
        match self {
            UsbSink::Fs(s) => s.ensure_dir(rel).await,
            UsbSink::Saf(s) => s.ensure_dir_uri(rel).await.map(|_| ()),
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
    /// then `commit()` (or `abort()` on error).
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
    /// `relative-dir-path -> content:// uri`, seeded with `"" -> tree_uri`. Built
    /// up as we walk, so each directory is resolved/created at most once.
    dir_uris: Mutex<HashMap<String, String>>,
}

impl SafSink {
    fn cache_get(&self, rel: &str) -> Option<String> {
        self.dir_uris.lock().unwrap().get(rel).cloned()
    }

    fn cache_put(&self, rel: &str, uri: &str) {
        self.dir_uris
            .lock()
            .unwrap()
            .insert(rel.to_string(), uri.to_string());
    }

    /// Resolve (creating as needed) the URI of the `/`-separated directory
    /// `rel`, walking one segment at a time from the tree root.
    async fn ensure_dir_uri(&self, rel: &str) -> Result<String> {
        if let Some(u) = self.cache_get(rel) {
            return Ok(u);
        }

        let mut parent_rel = String::new();
        let mut parent_uri = self.tree_uri.clone();

        for seg in rel.split('/').filter(|s| !s.is_empty()) {
            let child_rel = if parent_rel.is_empty() {
                seg.to_string()
            } else {
                format!("{parent_rel}/{seg}")
            };

            if let Some(u) = self.cache_get(&child_rel) {
                parent_uri = u;
                parent_rel = child_rel;
                continue;
            }

            // Does the segment already exist under this parent? (Re-exports, and
            // Android's own folders.) List first so we never create a duplicate.
            let children = saf::ls(&parent_uri).await?;
            let uri = match children.into_iter().find(|e| e.name == seg) {
                Some(e) => e.uri,
                None => {
                    // Create it, then take the URI mkdir printed, or find it.
                    match saf::mkdir(&parent_uri, seg).await? {
                        Some(u) => u,
                        None => saf::ls(&parent_uri)
                            .await?
                            .into_iter()
                            .find(|e| e.name == seg)
                            .map(|e| e.uri)
                            .ok_or_else(|| {
                                anyhow!("created directory '{seg}' but could not resolve its URI")
                            })?,
                    }
                }
            };

            self.cache_put(&child_rel, &uri);
            parent_uri = uri;
            parent_rel = child_rel;
        }

        Ok(parent_uri)
    }

    async fn create(&self, rel: &str) -> Result<SafWriter> {
        let (parent_rel, name) = split_parent(rel);
        let parent_uri = self.ensure_dir_uri(parent_rel).await?;

        // SAF's createDocument would make a "name (1)" duplicate if the target
        // already exists, so drop any stale file first (no-op if absent).
        let _ = saf::rm(&parent_uri, name).await;

        let mut child = Command::new("termux-saf-write")
            .arg(&parent_uri)
            .arg(name)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .context("spawning termux-saf-write (is the termux-api package installed?)")?;
        let stdin = child
            .stdin
            .take()
            .context("termux-saf-write gave no stdin handle")?;
        let stderr = child.stderr.take();

        Ok(SafWriter {
            child,
            stdin: Some(stdin),
            stderr,
            parent_uri,
            name: name.to_string(),
        })
    }
}

pub struct SafWriter {
    child: Child,
    stdin: Option<ChildStdin>,
    stderr: Option<ChildStderr>,
    parent_uri: String,
    name: String,
}

impl SafWriter {
    async fn write_all(&mut self, buf: &[u8]) -> Result<()> {
        // Take the handle out so we can also touch `self` (stderr) on error
        // without a double mutable borrow.
        let mut stdin = match self.stdin.take() {
            Some(s) => s,
            None => bail!("termux-saf-write stdin already closed"),
        };
        match stdin.write_all(buf).await {
            Ok(()) => {
                self.stdin = Some(stdin);
                Ok(())
            }
            // A broken pipe means the helper already died — surface its stderr
            // instead of the meaningless "broken pipe".
            Err(e) => {
                drop(stdin);
                let detail = self.drain_stderr().await;
                Err(anyhow!(
                    "termux-saf-write '{}' failed ({e}){}",
                    self.name,
                    detail
                ))
            }
        }
    }

    async fn commit(mut self) -> Result<()> {
        // Close stdin so termux-saf-write sees EOF and finalises the document.
        if let Some(mut stdin) = self.stdin.take() {
            stdin
                .shutdown()
                .await
                .context("closing termux-saf-write stdin")?;
        }
        let status = self
            .child
            .wait()
            .await
            .context("waiting on termux-saf-write")?;
        if !status.success() {
            let detail = self.drain_stderr().await;
            bail!("termux-saf-write '{}' failed{}", self.name, detail);
        }
        Ok(())
    }

    async fn abort(mut self) {
        drop(self.stdin.take());
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
        let _ = saf::rm(&self.parent_uri, &self.name).await;
    }

    /// Best-effort read of the helper's stderr, as `": <text>"` or empty.
    async fn drain_stderr(&mut self) -> String {
        if let Some(mut e) = self.stderr.take() {
            let mut s = String::new();
            let _ = e.read_to_string(&mut s).await;
            let s = s.trim();
            if !s.is_empty() {
                return format!(": {s}");
            }
        }
        String::new()
    }
}

/// Split a `/`-separated USB path into `(parent_dir, file_name)`.
fn split_parent(rel: &str) -> (&str, &str) {
    match rel.rfind('/') {
        Some(i) => (&rel[..i], &rel[i + 1..]),
        None => ("", rel),
    }
}

/// Thin wrappers over the `termux-saf-*` CLI, plus the selection/cleanup helpers
/// the client needs before a transfer.
///
/// Argument shape is always URI-addressed and single-level:
///   * `ls    <dir-uri>`
///   * `mkdir <parent-uri> <name>`
///   * `rm    <parent-uri> <name>`
///   * `write <parent-uri> <name>` (stdin)
pub mod saf {
    use anyhow::{anyhow, bail, Context, Result};
    use serde::Deserialize;
    use tokio::process::Command;

    /// One persisted SAF tree, as reported by `termux-saf-dirs`.
    #[derive(Debug, Clone, Deserialize)]
    pub struct SafDir {
        pub name: String,
        pub uri: String,
    }

    /// A child entry, as reported by `termux-saf-ls`. Extra keys (`type`,
    /// `last_modified`, ...) are ignored.
    #[derive(Debug, Clone, Deserialize)]
    pub(super) struct SafEntry {
        pub name: String,
        pub uri: String,
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

    /// List a directory's immediate children by its URI.
    pub(super) async fn ls(dir_uri: &str) -> Result<Vec<SafEntry>> {
        let out = Command::new("termux-saf-ls")
            .arg(dir_uri)
            .output()
            .await
            .context("running termux-saf-ls")?;
        if !out.status.success() {
            bail!(
                "termux-saf-ls failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let text = String::from_utf8_lossy(&out.stdout);
        let trimmed = text.trim();
        if trimmed.is_empty() {
            return Ok(Vec::new());
        }
        serde_json::from_str(trimmed).context("parsing termux-saf-ls JSON")
    }

    /// Create one child directory `name` under `parent_uri`. Returns the new
    /// directory's URI if the helper prints it, else `None` (caller re-`ls`es).
    pub(super) async fn mkdir(parent_uri: &str, name: &str) -> Result<Option<String>> {
        let out = Command::new("termux-saf-mkdir")
            .arg(parent_uri)
            .arg(name)
            .output()
            .await
            .context("running termux-saf-mkdir")?;
        if !out.status.success() {
            bail!(
                "termux-saf-mkdir '{name}' failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let uri = stdout
            .lines()
            .map(|l| l.trim())
            .find(|l| l.starts_with("content://"))
            .map(|l| l.to_string());
        Ok(uri)
    }

    /// Remove one child `name` under `parent_uri` (`termux-saf-rm`).
    pub(super) async fn rm(parent_uri: &str, name: &str) -> Result<()> {
        let out = Command::new("termux-saf-rm")
            .arg(parent_uri)
            .arg(name)
            .output()
            .await
            .context("running termux-saf-rm")?;
        if !out.status.success() {
            bail!(
                "termux-saf-rm '{name}' failed: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(())
    }

    /// Whether `name` under `tree_uri` is an empty directory — i.e. safe to treat
    /// as Android-generated junk. `Err` when the folder is absent or unreadable,
    /// so the caller silently skips it.
    pub async fn is_empty(tree_uri: &str, name: &str) -> Result<bool> {
        let child = ls(tree_uri)
            .await?
            .into_iter()
            .find(|e| e.name == name)
            .ok_or_else(|| anyhow!("'{name}' not present"))?;
        let inner = ls(&child.uri).await?;
        Ok(inner.is_empty())
    }

    /// Remove `name` under `tree_uri` (`termux-saf-rm`).
    pub async fn remove(tree_uri: &str, name: &str) -> Result<()> {
        rm(tree_uri, name).await
    }
}
