use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use comfy_table::{Attribute, Cell, Table};
use dialoguer::{theme::ColorfulTheme, Select};
use reqwest::Client;
use serde::Deserialize;

use crate::client::{get_api_url, load_session};

#[derive(Deserialize)]
pub struct Track {
    pub hash: String,
    pub title: Option<String>,
    pub artist: Option<String>,
}

#[derive(Deserialize)]
pub struct Playlist {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub created_at: DateTime<Utc>,
}

pub async fn fetch_playlist_by_name(
    client: &Client,
    base_url: &str,
    token: &str,
    name: &str,
) -> Result<Option<Playlist>> {
    let playlists: Vec<Playlist> = client
        .get(format!("{}/playlists", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?
        .json()
        .await?;

    let matches: Vec<Playlist> = playlists.into_iter().filter(|p| p.name == name).collect();

    if matches.is_empty() {
        return Ok(None);
    }

    if matches.len() == 1 {
        return Ok(Some(matches.into_iter().next().unwrap()));
    }

    // Multiple matches, prompt user to select
    println!("? Multiple playlists found with the name '{}':", name);
    let mut items = vec![];
    for p in &matches {
        let desc = p.description.as_deref().unwrap_or("No description");
        items.push(format!(
            "Created: {} | Description: {}",
            p.created_at.format("%Y-%m-%d %H:%M"),
            desc
        ));
    }

    let selection = Select::with_theme(&ColorfulTheme::default())
        .with_prompt("Select which playlist you meant:")
        .default(0)
        .items(&items)
        .interact()?;

    Ok(Some(matches.into_iter().nth(selection).unwrap()))
}

pub async fn run_playlist_list_flow() -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlists: Vec<Playlist> = http
        .get(format!("{}/playlists", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL);
    table.set_header(vec![
        Cell::new("Name").add_attribute(Attribute::Bold),
        Cell::new("Created At").add_attribute(Attribute::Bold),
        Cell::new("Description").add_attribute(Attribute::Bold),
    ]);

    for p in playlists {
        table.add_row(vec![
            Cell::new(p.name),
            Cell::new(p.created_at.format("%Y-%m-%d %H:%M").to_string()),
            Cell::new(p.description.unwrap_or_default()),
        ]);
    }

    println!("{table}");
    Ok(())
}

pub async fn run_playlist_view_flow(name: &str) -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, name)
        .await?
        .context(format!("Playlist '{}' not found", name))?;

    let hashes: Vec<String> = http
        .get(format!("{}/playlists/{}/hashes", base_url, playlist.id))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;

    let all_tracks: Vec<Track> = http
        .get(format!("{}/tracks", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL);
    table.set_header(vec![
        Cell::new("Artist").add_attribute(Attribute::Bold),
        Cell::new("Title").add_attribute(Attribute::Bold),
    ]);

    for hash in hashes {
        if let Some(t) = all_tracks.iter().find(|t| t.hash == hash) {
            table.add_row(vec![
                Cell::new(t.artist.as_deref().unwrap_or("-")),
                Cell::new(t.title.as_deref().unwrap_or("-")),
            ]);
        }
    }

    println!("Playlist: {}", name);
    println!("{table}");
    Ok(())
}

pub async fn run_playlist_add_flow(name: &str, add: &Option<String>) -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let mut hashes = Vec::new();

    if let Some(add_str) = add {
        let queries: Vec<&str> = add_str
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .collect();
        for q in queries {
            let res = http
                .get(format!("{}/tracks", base_url))
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
                let t = search_results.remove(0);
                println!(
                    "✓ Added: {} - {}",
                    t.artist.as_deref().unwrap_or("Unknown"),
                    t.title.as_deref().unwrap_or("Unknown")
                );
                Some(t)
            } else {
                let top_candidates = search_results.into_iter().take(5).collect::<Vec<_>>();
                println!("? Found multiple matches for '{}':", q);
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
                    .with_prompt("Select track to add")
                    .default(0)
                    .items(&items)
                    .interact()?;

                if selection < top_candidates.len() {
                    let selected_track = top_candidates.into_iter().nth(selection).unwrap();
                    println!(
                        "✓ Added: {} - {}",
                        selected_track.artist.as_deref().unwrap_or("Unknown"),
                        selected_track.title.as_deref().unwrap_or("Unknown")
                    );
                    Some(selected_track)
                } else {
                    println!("- Skipped query: {}", q);
                    None
                }
            };

            if let Some(t) = selected_track {
                hashes.push(t.hash);
            }
        }
    }

    let payload = serde_json::json!({
        "name": name,
        "hashes": hashes,
    });

    let res = http
        .post(format!("{}/playlists", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&payload)
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to create/update playlist: {}", err);
    }

    println!("✓ Playlist '{}' successfully updated.", name);
    Ok(())
}

pub async fn run_playlist_remove_flow(name: &str, tracks_str: &str) -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, name)
        .await?
        .context(format!("Playlist '{}' not found", name))?;

    let playlist_hashes: Vec<String> = http
        .get(format!("{}/playlists/{}/hashes", base_url, playlist.id))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;

    let all_tracks: Vec<Track> = http
        .get(format!("{}/tracks", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;

    // Filter tracks to only those currently in the playlist
    let playlist_tracks: Vec<&Track> = all_tracks
        .iter()
        .filter(|t| playlist_hashes.contains(&t.hash))
        .collect();

    let mut hashes_to_remove = Vec::new();
    let queries: Vec<&str> = tracks_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    for q in queries {
        let q_clean = q.to_lowercase();

        let matches: Vec<&Track> = playlist_tracks
            .iter()
            .copied()
            .filter(|t| {
                let title = t.title.as_deref().unwrap_or("").to_lowercase();
                let artist = t.artist.as_deref().unwrap_or("").to_lowercase();
                title.contains(&q_clean) || artist.contains(&q_clean)
            })
            .collect();

        if matches.is_empty() {
            println!("✗ No tracks in playlist to match query: {}", q);
            continue;
        }

        if matches.len() == 1 {
            let best_track = matches[0];
            hashes_to_remove.push(best_track.hash.clone());
            println!(
                "✓ Selected for removal: {} - {}",
                best_track.artist.as_deref().unwrap_or("Unknown"),
                best_track.title.as_deref().unwrap_or("Unknown")
            );
        } else {
            let top_candidates = matches.into_iter().take(5).collect::<Vec<_>>();

            println!("? Found multiple matches in playlist for '{}':", q);
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
                .with_prompt("Select track to REMOVE")
                .default(0)
                .items(&items)
                .interact()?;

            if selection < top_candidates.len() {
                let selected_track = top_candidates[selection];
                hashes_to_remove.push(selected_track.hash.clone());
                println!(
                    "✓ Selected for removal: {} - {}",
                    selected_track.artist.as_deref().unwrap_or("Unknown"),
                    selected_track.title.as_deref().unwrap_or("Unknown")
                );
            } else {
                println!("- Skipped query: {}", q);
            }
        }
    }

    if hashes_to_remove.is_empty() {
        println!("No tracks to remove.");
        return Ok(());
    }

    let payload = serde_json::json!({
        "hashes": hashes_to_remove,
    });

    let res = http
        .post(format!("{}/playlists/{}/remove", base_url, playlist.id))
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&payload)
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to remove tracks: {}", err);
    }

    println!("✓ Successfully removed tracks from playlist '{}'.", name);
    Ok(())
}

pub async fn run_playlist_delete_flow(name: &str) -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, name)
        .await?
        .context(format!("Playlist '{}' not found", name))?;

    let res = http
        .delete(format!("{}/playlists/{}", base_url, playlist.id))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?;

    if !res.status().is_success() {
        let err = res.text().await.unwrap_or_default();
        anyhow::bail!("Failed to delete playlist: {}", err);
    }

    println!("✓ Playlist '{}' successfully deleted.", name);
    Ok(())
}
