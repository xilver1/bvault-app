use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Select};
use sysinfo::Disks;
use std::path::PathBuf;
use std::sync::Arc;
use bvault_transfer::{reconcile_export, ReconcileOptions};
use crate::client::{load_session, get_api_url};
use crate::tui::ExportProgress;
use serde::Deserialize;
use uuid::Uuid;
use reqwest::Client;

#[derive(Deserialize)]
struct CreateExportResponse {
    export_id: Uuid,
    // manifest: Manifest ... (we don't need to parse it here)
}

pub async fn run_export_flow(playlist_name: &str, usb: bool, path: Option<String>) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();

    if !usb && path.is_none() {
        anyhow::bail!("You must specify either --usb or --path");
    }

    let usb_root = if let Some(p) = path {
        // Tier 3: Manual Fallback
        PathBuf::from(p)
    } else {
        // We have --usb flag, auto-detect
        let termux_storage_path = dirs::home_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("storage");
        
        if termux_storage_path.exists() && termux_storage_path.is_dir() {
            // Tier 1: Termux Auto-Detection
            let mut items = vec![];
            let mut paths = vec![];

            let entries = std::fs::read_dir(&termux_storage_path)?;
            for entry in entries.flatten() {
                if entry.file_type().is_ok() {
                    let name = entry.file_name().to_string_lossy().to_string();
                    items.push(format!("Android Storage: {}", name));
                    paths.push(entry.path());
                }
            }

            if items.is_empty() {
                anyhow::bail!("Termux storage found, but no drives are mapped! Did you run `termux-setup-storage`?");
            }

            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Choose your Termux drive:")
                .default(0)
                .items(&items)
                .interact()?;

            let chosen_drive = paths[selection].clone();
            // Append Android's strict allowed write path for Termux
            chosen_drive.join("Android/data/com.termux/files")
        } else {
            // Tier 2: Desktop Auto-Detection (sysinfo)
            let disks = Disks::new_with_refreshed_list();
            let removable: Vec<_> = disks.iter().filter(|d| d.is_removable()).collect();
            
            if removable.is_empty() {
                anyhow::bail!("No removable USB devices found!");
            }

            let items: Vec<String> = removable
                .iter()
                .map(|d| format!("{} - {:?}", d.mount_point().display(), d.name()))
                .collect();

            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Choose your USB device:")
                .default(0)
                .items(&items)
                .interact()?;

            removable[selection].mount_point().to_path_buf()
        }
    };
    
    println!("✓ {} Chosen", usb_root.display());

    println!("⠋ resolving playlist '{}'...", playlist_name);
    
    let http = Client::new();
    
    // Let's fetch all playlists to find the ID
    let playlists_res = http.get(&format!("{}/playlists", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send().await?;
        
    #[derive(Deserialize)]
    struct Playlist {
        id: Uuid,
        name: String,
    }
    
    let playlists: Vec<Playlist> = playlists_res.json().await.unwrap_or_default();
    let pl_id = playlists.into_iter().find(|p| p.name == playlist_name)
        .map(|p| p.id)
        .context(format!("Playlist '{}' not found", playlist_name))?;

    println!("⠋ building rekordbox layout (PDB + ANLZ)…");
    
    let export_payload = serde_json::json!({
        "playlist_ids": [pl_id]
    });

    let build_res = http.post(&format!("{}/exports", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&export_payload)
        .send().await?;

    if !build_res.status().is_success() {
        anyhow::bail!("Failed to build export: {}", build_res.status());
    }

    let build_data: CreateExportResponse = build_res.json().await?;
    
    println!("✓ Build ready. Starting transfer...");

    let progress = Arc::new(ExportProgress::new());

    let opts = ReconcileOptions {
        base_url,
        export_id: build_data.export_id.to_string(),
        usb_root: usb_root.clone(),
        auth_token: session.token.clone(),
    };

    reconcile_export(opts, progress).await?;
    
    // We need to determine if we used the termux flow
    let is_termux = usb_root.to_string_lossy().contains("com.termux");

    if is_termux {
        println!("✓ rekordbox USB written to Android app storage!");
        println!("  IMPORTANT: Android 11+ prevents writing directly to the root of your USB drive.");
        println!("  Please open your Android File Manager (e.g. Solid Explorer) and MOVE the 'PIONEER' and 'Contents' folders");
        println!("  from {} to the absolute root of your USB drive before plugging it into a CDJ.", usb_root.display());
    } else {
        println!("✓ rekordbox USB written — plug into any CDJ");
    }

    Ok(())
}
