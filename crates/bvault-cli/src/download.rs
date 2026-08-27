use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use std::path::PathBuf;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;

use crate::client::{get_api_url, load_session};
use crate::playlist::{fetch_playlist_by_name, Track};
use dialoguer::{theme::ColorfulTheme, Select};

pub async fn run_download_flow(query: &str, is_playlist: bool, out_dir: &str) -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let out_path = PathBuf::from(out_dir);
    if !out_path.exists() {
        fs::create_dir_all(&out_path)
            .await
            .context("Failed to create output directory")?;
    }

    let mut tracks_to_download = Vec::new(); // (hash, title)

    // all_tracks no longer fetched upfront for non-playlist flow

    if is_playlist {
        let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, query)
            .await?
            .context(format!("Playlist '{}' not found", query))?;

        let playlist_hashes: Vec<String> = http
            .get(&format!("{}/playlists/{}/hashes", base_url, playlist.id))
            .header("Authorization", format!("Bearer {}", session.token))
            .send()
            .await?
            .json()
            .await?;

        let all_tracks: Vec<Track> = http
            .get(&format!("{}/tracks", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .query(&[("limit", "100000")]) // Fetch all tracks to match playlist hashes
            .send()
            .await?
            .json()
            .await?;

        for hash in playlist_hashes {
            if let Some(t) = all_tracks.iter().find(|t| t.hash == hash) {
                let title = format!(
                    "{} - {}",
                    t.artist.as_deref().unwrap_or("Unknown"),
                    t.title.as_deref().unwrap_or("Unknown")
                );
                // Sanitize filename
                let safe_title =
                    title.replace(&['/', '\\', ':', '*', '?', '"', '<', '>', '|'][..], "_");
                tracks_to_download.push((hash.clone(), safe_title));
            }
        }

        if tracks_to_download.is_empty() {
            println!("Playlist is empty.");
            return Ok(());
        }
    } else {
        let queries: Vec<&str> = query
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        for q in queries {
            let res = http
                .get(&format!("{}/tracks", base_url))
                .header("Authorization", format!("Bearer {}", session.token))
                .query(&[("limit", "10"), ("q", q)])
                .send()
                .await?;
            let mut search_results: Vec<Track> = res.json().await?;

            if search_results.is_empty() {
                println!("✗ No tracks in library to match query: {}", q);
                continue;
            }

            let selected_track = if search_results.len() == 1 {
                Some(search_results.remove(0))
            } else {
                let top_candidates = search_results.into_iter().take(5).collect::<Vec<_>>();
                println!("? Found multiple partial matches for '{}':", q);
                let mut items = vec![];
                for t in &top_candidates {
                    items.push(format!(
                        "{} - {}",
                        t.artist.as_deref().unwrap_or("Unknown"),
                        t.title.as_deref().unwrap_or("Unknown")
                    ));
                }
                items.push("Skip".to_string());

                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Select track to download")
                    .default(0)
                    .items(&items)
                    .interact()?;

                if selection < top_candidates.len() {
                    Some(top_candidates.into_iter().nth(selection).unwrap())
                } else {
                    println!("- Skipped query: {}", q);
                    None
                }
            };

            if let Some(t) = selected_track {
                let title = format!(
                    "{} - {}",
                    t.artist.as_deref().unwrap_or("Unknown"),
                    t.title.as_deref().unwrap_or("Unknown")
                );
                let safe_title =
                    title.replace(&['/', '\\', ':', '*', '?', '"', '<', '>', '|'][..], "_");
                tracks_to_download.push((t.hash, safe_title));
            }
        }

        if tracks_to_download.is_empty() {
            println!("No tracks to download.");
            return Ok(());
        }
    }

    println!(
        "Starting download of {} tracks...",
        tracks_to_download.len()
    );

    for (hash, title) in tracks_to_download {
        let target_file = out_path.join(format!("{}.mp3", title));
        if target_file.exists() {
            println!("✓ Skipped (already exists): {}.mp3", title);
            continue;
        }

        println!("Downloading: {}.mp3", title);
        let mut res = http
            .get(&format!("{}/tracks/{}/raw", base_url, hash))
            .header("Authorization", format!("Bearer {}", session.token))
            .send()
            .await?;

        if !res.status().is_success() {
            println!("✗ Failed to download {}: {}", title, res.status());
            continue;
        }

        let total_size = res.content_length().unwrap_or(0);
        let pb = ProgressBar::new(total_size);
        pb.set_style(
            ProgressStyle::default_bar()
                .template("{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {bytes}/{total_bytes} ({eta})")
                .unwrap()
                .progress_chars("#>-"),
        );

        let mut file = File::create(&target_file).await?;
        let mut downloaded: u64 = 0;

        while let Some(chunk) = res.chunk().await? {
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;
            pb.set_position(downloaded);
        }

        file.flush().await?;
        pb.finish_with_message("done");
    }

    println!("✓ Download complete!");
    Ok(())
}
