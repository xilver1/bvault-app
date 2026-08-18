use anyhow::{Context, Result};
use reqwest::Client;
use std::path::PathBuf;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use indicatif::{ProgressBar, ProgressStyle};

use crate::client::{get_api_url, load_session};
use crate::playlist::{fetch_playlist_by_name, Track, clean_string, fuzzy_token_score};
use dialoguer::{theme::ColorfulTheme, Select};

pub async fn run_download_flow(query: &str, is_playlist: bool, out_dir: &str) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let out_path = PathBuf::from(out_dir);
    if !out_path.exists() {
        fs::create_dir_all(&out_path).await.context("Failed to create output directory")?;
    }

    let mut tracks_to_download = Vec::new(); // (hash, title)

    let all_tracks: Vec<Track> = http
        .get(&format!("{}/tracks", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;

    if is_playlist {
        let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, query).await?
            .context(format!("Playlist '{}' not found", query))?;

        let playlist_hashes: Vec<String> = http
            .get(&format!("{}/playlists/{}/hashes", base_url, playlist.id))
            .header("Authorization", format!("Bearer {}", session.token))
            .send()
            .await?
            .json()
            .await?;

        for hash in playlist_hashes {
            if let Some(t) = all_tracks.iter().find(|t| t.hash == hash) {
                let title = format!("{} - {}", t.artist.as_deref().unwrap_or("Unknown"), t.title.as_deref().unwrap_or("Unknown"));
                // Sanitize filename
                let safe_title = title.replace(&['/', '\\', ':', '*', '?', '"', '<', '>', '|'][..], "_");
                tracks_to_download.push((hash.clone(), safe_title));
            }
        }
        
        if tracks_to_download.is_empty() {
            println!("Playlist is empty.");
            return Ok(());
        }
    } else {
        let queries: Vec<&str> = query.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        for q in queries {
            let q_clean = clean_string(q);
            let mut scored_tracks: Vec<(f64, &Track)> = all_tracks.iter().map(|t| {
                let title = t.title.as_deref().unwrap_or("");
                let artist = t.artist.as_deref().unwrap_or("");
                let target = format!("{} {}", artist, title);
                let t_clean = clean_string(&target);
                let score = fuzzy_token_score(&q_clean, &t_clean);
                (score, t)
            }).collect();

            scored_tracks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

            if scored_tracks.is_empty() {
                println!("✗ No tracks in library to match query: {}", q);
                continue;
            }

            let (best_score, best_track) = scored_tracks[0];
            let mut selected_track = None;

            if best_score >= 0.99 {
                selected_track = Some(best_track);
            } else {
                let top_candidates: Vec<_> = scored_tracks.into_iter().filter(|(s, _)| *s > 0.4).take(5).collect();
                if top_candidates.is_empty() {
                    println!("✗ No good match found for query: {}", q);
                    continue;
                }

                println!("? Found multiple partial matches for '{}':", q);
                let mut items = vec![];
                for (s, t) in &top_candidates {
                    items.push(format!("[{:.0}%] {} - {}", s * 100.0, t.artist.as_deref().unwrap_or("Unknown"), t.title.as_deref().unwrap_or("Unknown")));
                }
                items.push("Skip".to_string());

                let selection = Select::with_theme(&ColorfulTheme::default())
                    .with_prompt("Select track to download")
                    .default(0)
                    .items(&items)
                    .interact()?;

                if selection < top_candidates.len() {
                    selected_track = Some(top_candidates[selection].1);
                } else {
                    println!("- Skipped query: {}", q);
                }
            }

            if let Some(t) = selected_track {
                let title = format!("{} - {}", t.artist.as_deref().unwrap_or("Unknown"), t.title.as_deref().unwrap_or("Unknown"));
                let safe_title = title.replace(&['/', '\\', ':', '*', '?', '"', '<', '>', '|'][..], "_");
                tracks_to_download.push((t.hash.clone(), safe_title));
            }
        }
        
        if tracks_to_download.is_empty() {
            println!("No tracks to download.");
            return Ok(());
        }
    }

    println!("Starting download of {} tracks...", tracks_to_download.len());

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
