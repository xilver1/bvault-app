use anyhow::{Context, Result};
use reqwest::Client;
use serde::Deserialize;
use dialoguer::{theme::ColorfulTheme, Select};
use comfy_table::{Table, Cell, Attribute};
use chrono::{DateTime, Utc};

use crate::client::{get_api_url, load_session};

#[derive(Deserialize)]
struct Track {
    hash: String,
    title: Option<String>,
    artist: Option<String>,
}

#[derive(Deserialize)]
struct Playlist {
    id: String,
    name: String,
    description: Option<String>,
    created_at: DateTime<Utc>,
}

fn clean_string(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { ' ' })
        .collect()
}

fn fuzzy_token_score(query: &str, target: &str) -> f64 {
    let q_words: Vec<&str> = query.split_whitespace().collect();
    let t_words: Vec<&str> = target.split_whitespace().collect();

    if q_words.is_empty() || t_words.is_empty() {
        return 0.0;
    }

    let mut total_score = 0.0;
    for qw in &q_words {
        let mut best_word_score = 0.0;
        for tw in &t_words {
            let score = if tw.contains(qw) {
                1.0
            } else {
                strsim::jaro_winkler(qw, tw)
            };
            if score > best_word_score {
                best_word_score = score;
            }
        }
        total_score += best_word_score;
    }

    total_score / (q_words.len() as f64)
}

async fn fetch_playlist_by_name(client: &Client, base_url: &str, token: &str, name: &str) -> Result<Option<Playlist>> {
    let playlists: Vec<Playlist> = client
        .get(&format!("{}/playlists", base_url))
        .header("Authorization", format!("Bearer {}", token))
        .send()
        .await?
        .json()
        .await?;
        
    Ok(playlists.into_iter().find(|p| p.name == name))
}

pub async fn run_playlist_list_flow() -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlists: Vec<Playlist> = http
        .get(&format!("{}/playlists", base_url))
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
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, name).await?
        .context(format!("Playlist '{}' not found", name))?;
        
    let hashes: Vec<String> = http
        .get(&format!("{}/playlists/{}/hashes", base_url, playlist.id))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?
        .json()
        .await?;
        
    let all_tracks: Vec<Track> = http
        .get(&format!("{}/tracks", base_url))
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

        let queries: Vec<&str> = add_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        for q in queries {
            let q_clean = clean_string(q);
            
            let mut scored_tracks: Vec<(f64, &Track)> = tracks.iter().map(|t| {
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
            
            if best_score >= 0.99 {
                hashes.push(best_track.hash.clone());
                println!("✓ Added (100%): {} - {}", best_track.artist.as_deref().unwrap_or("Unknown"), best_track.title.as_deref().unwrap_or("Unknown"));
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
                    .with_prompt("Select track to add")
                    .default(0)
                    .items(&items)
                    .interact()?;
                    
                if selection < top_candidates.len() {
                    let selected_track = top_candidates[selection].1;
                    hashes.push(selected_track.hash.clone());
                    println!("✓ Added: {} - {}", selected_track.artist.as_deref().unwrap_or("Unknown"), selected_track.title.as_deref().unwrap_or("Unknown"));
                } else {
                    println!("- Skipped query: {}", q);
                }
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
        anyhow::bail!("Failed to create/update playlist: {}", err);
    }

    println!("✓ Playlist '{}' successfully updated.", name);
    Ok(())
}

pub async fn run_playlist_remove_flow(name: &str, tracks_str: &str) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, name).await?
        .context(format!("Playlist '{}' not found", name))?;
        
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
        .send()
        .await?
        .json()
        .await?;
        
    // Filter tracks to only those currently in the playlist
    let playlist_tracks: Vec<&Track> = all_tracks.iter()
        .filter(|t| playlist_hashes.contains(&t.hash))
        .collect();

    let mut hashes_to_remove = Vec::new();
    let queries: Vec<&str> = tracks_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    
    for q in queries {
        let q_clean = clean_string(q);
        
        let mut scored_tracks: Vec<(f64, &&Track)> = playlist_tracks.iter().map(|t| {
            let title = t.title.as_deref().unwrap_or("");
            let artist = t.artist.as_deref().unwrap_or("");
            let target = format!("{} {}", artist, title);
            let t_clean = clean_string(&target);
            let score = fuzzy_token_score(&q_clean, &t_clean);
            (score, t)
        }).collect();
        
        scored_tracks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        
        if scored_tracks.is_empty() {
            println!("✗ No tracks in playlist to match query: {}", q);
            continue;
        }
        
        let (best_score, best_track) = scored_tracks[0];
        
        if best_score >= 0.99 {
            hashes_to_remove.push(best_track.hash.clone());
            println!("✓ Selected for removal (100%): {} - {}", best_track.artist.as_deref().unwrap_or("Unknown"), best_track.title.as_deref().unwrap_or("Unknown"));
        } else {
            let top_candidates: Vec<_> = scored_tracks.into_iter().filter(|(s, _)| *s > 0.4).take(5).collect();
            
            if top_candidates.is_empty() {
                println!("✗ No good match found in playlist for query: {}", q);
                continue;
            }
            
            println!("? Found multiple partial matches in playlist for '{}':", q);
            let mut items = vec![];
            for (s, t) in &top_candidates {
                items.push(format!("[{:.0}%] {} - {}", s * 100.0, t.artist.as_deref().unwrap_or("Unknown"), t.title.as_deref().unwrap_or("Unknown")));
            }
            items.push("Skip".to_string());
            
            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Select track to REMOVE")
                .default(0)
                .items(&items)
                .interact()?;
                
            if selection < top_candidates.len() {
                let selected_track = top_candidates[selection].1;
                hashes_to_remove.push(selected_track.hash.clone());
                println!("✓ Selected for removal: {} - {}", selected_track.artist.as_deref().unwrap_or("Unknown"), selected_track.title.as_deref().unwrap_or("Unknown"));
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
        .post(&format!("{}/playlists/{}/remove", base_url, playlist.id))
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
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let playlist = fetch_playlist_by_name(&http, &base_url, &session.token, name).await?
        .context(format!("Playlist '{}' not found", name))?;
        
    let res = http
        .delete(&format!("{}/playlists/{}", base_url, playlist.id))
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
