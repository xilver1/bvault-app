use crate::client::{get_api_url, load_session};
use crate::tui::ExportProgress;
use anyhow::{Context, Result};
use bvault_transfer::{reconcile_export, saf, ReconcileOptions, UsbTarget};
use dialoguer::{theme::ColorfulTheme, Select};
use reqwest::Client;
use serde::Deserialize;
use std::path::PathBuf;
use std::sync::Arc;
use sysinfo::Disks;
use uuid::Uuid;

#[derive(Deserialize)]
struct CreateExportResponse {
    export_id: Uuid,
    // manifest: Manifest ... (we don't need to parse it here)
}

pub async fn run_export_flow(playlist_name: &str, usb: bool, path: Option<String>) -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();

    if !usb && path.is_none() {
        anyhow::bail!("You must specify either --usb or --path");
    }

    // Resolve *where* we write. Three cases:
    //   --path        -> a literal filesystem path (works everywhere)
    //   --usb, phone  -> Android SAF: pick a granted tree, scrub Android's junk
    //   --usb, desktop-> a removable mount discovered via sysinfo
    let target = resolve_target(path).await?;

    if let UsbTarget::Fs { root } = &target {
        println!("✓ {} Chosen", root.display());
    }

    println!("⠋ resolving playlist '{}'...", playlist_name);

    let http = Client::new();

    // Let's fetch all playlists to find the ID
    let playlists_res = http
        .get(&format!("{}/playlists", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?;

    #[derive(Deserialize)]
    struct Playlist {
        id: Uuid,
        name: String,
    }

    let playlists: Vec<Playlist> = playlists_res.json().await.unwrap_or_default();
    let pl_id = playlists
        .into_iter()
        .find(|p| p.name == playlist_name)
        .map(|p| p.id)
        .context(format!("Playlist '{}' not found", playlist_name))?;

    println!("⠋ building rekordbox layout (PDB + ANLZ)…");

    let export_payload = serde_json::json!({
        "playlist_ids": [pl_id]
    });

    let build_res = http
        .post(&format!("{}/exports", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&export_payload)
        .send()
        .await?;

    if !build_res.status().is_success() {
        anyhow::bail!("Failed to build export: {}", build_res.status());
    }

    let build_data: CreateExportResponse = build_res.json().await?;

    println!("✓ Build ready. Starting transfer...");

    let progress = Arc::new(ExportProgress::new());

    let opts = ReconcileOptions {
        base_url,
        export_id: build_data.export_id.to_string(),
        target: target.clone(),
        auth_token: session.token.clone(),
    };

    reconcile_export(opts, progress).await?;

    match target {
        UsbTarget::Saf { .. } => {
            println!("✓ rekordbox export written straight to the USB root — plug into any CDJ");
        }
        UsbTarget::Fs { .. } => {
            println!("✓ rekordbox USB written — plug into any CDJ");
        }
    }

    Ok(())
}

/// Decide the write target from the flags and the runtime environment.
async fn resolve_target(path: Option<String>) -> Result<UsbTarget> {
    if let Some(p) = path {
        // Tier 3: explicit path — trusted verbatim on every platform.
        return Ok(UsbTarget::Fs {
            root: PathBuf::from(p),
        });
    }

    if saf::detect() {
        // Tier 1: Android/Termux. The USB is unreachable through the normal
        // filesystem; go through the Storage Access Framework.
        resolve_saf_target().await
    } else {
        // Tier 2: desktop — a removable mount.
        resolve_desktop_target()
    }
}

/// Desktop USB pick via sysinfo's removable-disk enumeration.
fn resolve_desktop_target() -> Result<UsbTarget> {
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

    Ok(UsbTarget::Fs {
        root: removable[selection].mount_point().to_path_buf(),
    })
}

/// Android SAF pick: choose (or grant) a directory tree, then remove the empty
/// folders Android auto-creates on a freshly plugged USB.
async fn resolve_saf_target() -> Result<UsbTarget> {
    // 1. Pick a granted tree, offering to open the system picker for a new one.
    let chosen = loop {
        let mut trees = saf::list_managed_dirs().await?;

        let mut items: Vec<String> = trees.iter().map(|d| d.name.clone()).collect();
        let grant_idx = items.len();
        items.push("➕ Grant a new directory (open the Android picker)…".to_string());

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Choose your USB drive")
            .default(0)
            .items(&items)
            .interact()?;

        if selection == grant_idx {
            println!(
                "  Opening the Android folder picker — select your USB's root and allow access."
            );
            saf::manage_dir().await?;
            continue;
        }

        break trees.swap_remove(selection);
    };

    println!("✓ {} Chosen", chosen.name);
    let tree_uri = chosen.uri;

    // 2. Scrub Android's auto-created empties. A same-named folder that already
    //    holds files is the user's, and is left untouched.
    for junk in ["LOST.DIR", "Movies", "Music", "Pictures"] {
        match saf::is_empty(&tree_uri, junk).await {
            Ok(true) => {
                if saf::remove(&tree_uri, junk).await.is_ok() {
                    println!("  · removed Android-created '{}'", junk);
                }
            }
            Ok(false) => {
                println!("  · kept '{}' — it already contains files", junk);
            }
            Err(_) => {}
        }
    }

    Ok(UsbTarget::Saf { tree_uri })
}
