use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::fs::{self, File};
use tokio::io::{AsyncWriteExt, BufWriter};
use anyhow::{Context, Result};
use reqwest::Client;
use tracing::{info, warn, debug};

use bvault_export::{Manifest, ManifestEntry, Source};

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
    pub usb_root: PathBuf,
    pub auth_token: String,
}

pub async fn reconcile_export<P: TransferProgress + 'static>(
    opts: ReconcileOptions,
    progress: Arc<P>,
) -> Result<()> {
    let client = Client::new();
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
        let dest_path = opts.usb_root.join(&entry.usb_path);
        
        // Ensure parent directory exists
        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent).await.context("Failed to create directories on USB")?;
        }

        // Check if we can skip it
        if dest_path.exists() {
            if let Ok(meta) = fs::metadata(&dest_path).await {
                if meta.len() == entry.size {
                    progress.on_file_skipped(&entry.usb_path);
                    continue;
                }
            }
        }

        progress.on_file_start(&entry.usb_path, entry.size);

        let file_url = format!(
            "{}/exports/{}/files/{}",
            opts.base_url,
            opts.export_id,
            entry.usb_path
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

        let tmp_path = dest_path.with_extension("tmp");
        let mut file = BufWriter::new(File::create(&tmp_path).await.context("Failed to create tmp file")?);
        
        let mut hasher = bvault_hash::ContentHasher::new();
        
        let mut success = true;
        loop {
            match res.chunk().await {
                Ok(Some(chunk)) => {
                    if let Err(e) = file.write_all(&chunk).await {
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
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }

        // Fsync before renaming
        if let Err(e) = file.flush().await {
            progress.on_error(&entry.usb_path, &format!("Flush error: {}", e));
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }
        
        let inner_file = file.into_inner();
        if let Err(e) = inner_file.sync_all().await {
            progress.on_error(&entry.usb_path, &format!("Fsync error: {}", e));
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }

        // Hash verification for raw audio
        if let Source::Raw { hash } = &entry.source {
            let computed_hash = bvault_hash::hash_hex(hasher.finalize());
            if &computed_hash != hash {
                progress.on_error(&entry.usb_path, "Hash mismatch: file corrupted during transfer");
                let _ = fs::remove_file(&tmp_path).await;
                continue;
            }
        }

        // Rename tmp to final
        if let Err(e) = fs::rename(&tmp_path, &dest_path).await {
            progress.on_error(&entry.usb_path, &format!("Rename error: {}", e));
            let _ = fs::remove_file(&tmp_path).await;
            continue;
        }

        progress.on_file_done(&entry.usb_path);
    }

    // Optional: cleanup
    // We could DELETE /exports/{export_id} here or let the user/caller do it
    let cleanup_url = format!("{}/exports/{}", opts.base_url, opts.export_id);
    let _ = client.delete(&cleanup_url).header("Authorization", format!("Bearer {}", opts.auth_token)).send().await;

    progress.on_complete();
    Ok(())
}
