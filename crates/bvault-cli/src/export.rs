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

pub async fn run_export_flow(playlist_name: &str) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();

    // Find a USB device
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

    let chosen_disk = removable[selection];
    let usb_root = chosen_disk.mount_point().to_path_buf();
    
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
        usb_root,
        auth_token: session.token.clone(),
    };

    reconcile_export(opts, progress).await?;
    
    println!("✓ rekordbox USB written — plug into any CDJ");

    Ok(())
}
