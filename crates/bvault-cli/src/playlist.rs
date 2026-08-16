use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;

use crate::client::{get_api_url, load_session};

#[derive(Deserialize)]
struct Track {
    hash: String,
    title: Option<String>,
    artist: Option<String>,
}

pub async fn run_playlist_flow(name: &str, add: &Option<String>) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let mut hashes = Vec::new();

    if let Some(add_str) = add {
        let tracks: Vec<Track> = http
            .get(&format!("{}/tracks", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .send()
            .await?
            .json()
            .await?;

        let queries: Vec<&str> = add_str.split(',').map(|s| s.trim()).collect();
        for q in queries {
            let q_lower = q.to_lowercase();
            // find track by title or artist
            if let Some(t) = tracks.iter().find(|t| {
                t.title.as_deref().unwrap_or("").to_lowercase().contains(&q_lower)
                || t.artist.as_deref().unwrap_or("").to_lowercase().contains(&q_lower)
            }) {
                hashes.push(t.hash.clone());
                println!("✓ Added: {} - {}", t.artist.as_deref().unwrap_or("Unknown"), t.title.as_deref().unwrap_or("Unknown"));
            } else {
                println!("✗ Track not found for query: {}", q);
            }
        }
    }

    let payload = serde_json::json!({
        "name": name,
        "hashes": hashes,
    });

    let res = http
        .post(&format!("{}/playlists", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&payload)
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to create playlist: {}", err);
    }

    println!("✓ Playlist '{}' created successfully.", name);
    Ok(())
}
