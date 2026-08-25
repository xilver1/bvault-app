use std::sync::Arc;

use anyhow::{Context, Result};
use reqwest::Client;
use tracing::debug;

use bvault_manifest::{Manifest, Source};

use crate::sink::{UsbSink, UsbTarget};

pub trait TransferProgress: Send + Sync {
    fn on_start(&self, total_files: usize, total_bytes: u64);
    fn on_file_start(&self, usb_path: &str, size: u64);
    fn on_file_progress(&self, bytes: u64);
    fn on_file_done(&self, usb_path: &str);
    fn on_file_skipped(&self, usb_path: &str);
    fn on_error(&self, usb_path: &str, error: &str);
    fn on_complete(&self);
}

pub struct ReconcileOptions {
    pub base_url: String,
    pub export_id: String,
    /// Where the export is written — a filesystem path (desktop / `--path`) or
    /// an Android SAF tree (phone). The reconcile loop is agnostic to which.
    pub target: UsbTarget,
    pub auth_token: String,
}

/// The `/`-separated parent of a USB path, or `""` for a root-level file.
fn parent_of(usb_path: &str) -> &str {
    match usb_path.rfind('/') {
        Some(i) => &usb_path[..i],
        None => "",
    }
}

pub async fn reconcile_export<P: TransferProgress + 'static>(
    opts: ReconcileOptions,
    progress: Arc<P>,
) -> Result<()> {
    let client = Client::new();
    let sink = UsbSink::new(opts.target);
    let manifest_url = format!("{}/exports/{}/manifest", opts.base_url, opts.export_id);

    // 1. Fetch manifest
    debug!("Fetching manifest from {}", manifest_url);
    let res = client
        .get(&manifest_url)
        .header("Authorization", format!("Bearer {}", opts.auth_token))
        .send()
        .await
        .context("Failed to fetch manifest")?;

    if !res.status().is_success() {
        anyhow::bail!("Failed to fetch manifest: {}", res.status());
    }

    let manifest: Manifest = res.json().await.context("Failed to parse manifest JSON")?;

    let total_files = manifest.entries.len();
    let total_bytes: u64 = manifest.entries.iter().map(|e| e.size).sum();

    progress.on_start(total_files, total_bytes);

    for entry in &manifest.entries {
        // Ensure the parent directory exists on the target.
        sink.ensure_dir(parent_of(&entry.usb_path))
            .await
            .context("Failed to create directories on USB")?;

        // Skip an already-present, same-size file (desktop only; SAF reports
        // absent and always rewrites).
        if let Some(len) = sink.file_len(&entry.usb_path).await? {
            if len == entry.size {
                progress.on_file_skipped(&entry.usb_path);
                continue;
            }
        }

        progress.on_file_start(&entry.usb_path, entry.size);

        let file_url = format!(
            "{}/exports/{}/files/{}",
            opts.base_url, opts.export_id, entry.usb_path
        );

        let mut res = client
            .get(&file_url)
            .header("Authorization", format!("Bearer {}", opts.auth_token))
            .send()
            .await
            .context("Failed to start file download")?;

        if !res.status().is_success() {
            progress.on_error(&entry.usb_path, &format!("HTTP {}", res.status()));
            continue;
        }

        let mut writer = match sink.create(&entry.usb_path).await {
            Ok(w) => w,
            Err(e) => {
                progress.on_error(&entry.usb_path, &format!("Open error: {}", e));
                continue;
            }
        };

        let mut hasher = bvault_hash::ContentHasher::new();
        let mut success = true;

        loop {
            match res.chunk().await {
                Ok(Some(chunk)) => {
                    if let Err(e) = writer.write_all(&chunk).await {
                        progress.on_error(&entry.usb_path, &format!("Write error: {}", e));
                        success = false;
                        break;
                    }
                    hasher.update(&chunk);
                    progress.on_file_progress(chunk.len() as u64);
                }
                Ok(None) => break, // EOF
                Err(e) => {
                    progress.on_error(&entry.usb_path, &format!("Network error: {}", e));
                    success = false;
                    break;
                }
            }
        }

        if !success {
            writer.abort().await;
            continue;
        }

        // Hash verification for raw audio (verbatim content-addressed store).
        if let Source::Raw { hash } = &entry.source {
            let computed_hash = bvault_hash::hash_hex(hasher.finalize());
            if &computed_hash != hash {
                progress.on_error(&entry.usb_path, "Hash mismatch: file corrupted during transfer");
                writer.abort().await;
                continue;
            }
        }

        if let Err(e) = writer.commit().await {
            progress.on_error(&entry.usb_path, &format!("Commit error: {}", e));
            continue;
        }

        progress.on_file_done(&entry.usb_path);
    }

    // Best-effort server-side cleanup of the staged build.
    let cleanup_url = format!("{}/exports/{}", opts.base_url, opts.export_id);
    let _ = client
        .delete(&cleanup_url)
        .header("Authorization", format!("Bearer {}", opts.auth_token))
        .send()
        .await;

    progress.on_complete();
    Ok(())
}
