use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Select};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;

use crate::client::{get_api_url, load_session};

#[derive(Deserialize)]
struct SearchResult {
    title: String,
    url: String,
    uploader: String,
}

#[derive(Deserialize)]
struct Track {
    hash: String,
    title: Option<String>,
}

pub async fn run_ingest_flow(query: &str, youtube: bool) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let target_url = if youtube {
        println!("Searching YouTube for: {}", query);
        let res = http
            .get(&format!("{}/search", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .query(&[("q", query), ("limit", "5")])
            .send()
            .await?;

        if !res.status().is_success() {
            anyhow::bail!("Search failed: {}", res.status());
        }

        let results: Vec<SearchResult> = res.json().await?;
        if results.is_empty() {
            anyhow::bail!("No results found for '{}'", query);
        }

        println!("Top results:");
        let items: Vec<String> = results
            .iter()
            .map(|r| format!("{} ({})", r.title, r.uploader))
            .collect();

        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Choose a track to ingest:")
            .default(0)
            .items(&items)
            .interact()?;

        let chosen = &results[selection];
        println!("✓ {} chosen (\"{}\")", chosen.title, chosen.url);
        chosen.url.clone()
    } else {
        query.to_string()
    };

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
    pb.set_message("downloading & analyzing...");
    pb.enable_steady_tick(Duration::from_millis(100));

    // Get track count before
    let initial_tracks: Vec<Track> = http
        .get(&format!("{}/tracks", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;
    let initial_len = initial_tracks.len();

    let payload = serde_json::json!({ "url": target_url });
    let ingest_res = http
        .post(&format!("{}/ingest/ytdlp", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&payload)
        .send()
        .await?;

    if !ingest_res.status().is_success() {
        pb.finish_with_message(format!("✗ Ingest failed: {}", ingest_res.status()));
        anyhow::bail!("Ingest failed");
    }

    // Poll until a new track appears
    loop {
        sleep(Duration::from_secs(1)).await;
        let current_tracks: Vec<Track> = http
            .get(&format!("{}/tracks", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .send()
            .await?
            .json()
            .await?;

        if current_tracks.len() > initial_len {
            // Find the new track
            let new_track = current_tracks.iter().find(|ct| !initial_tracks.iter().any(|it| it.hash == ct.hash));
            if let Some(track) = new_track {
                pb.finish_with_message(format!("✓ ingested: {}", track.title.as_deref().unwrap_or("Unknown Title")));
            } else {
                pb.finish_with_message("✓ ingested successfully");
            }
            break;
        }
    }

    Ok(())
}
