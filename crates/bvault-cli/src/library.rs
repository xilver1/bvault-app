use anyhow::{Context, Result};
use comfy_table::{Table, Cell, Color, Attribute};
use reqwest::Client;
use serde::Deserialize;
use chrono::{DateTime, Utc};

use crate::client::{get_api_url, load_session};

#[derive(Deserialize)]
struct Track {
    hash: String,
    title: Option<String>,
    artist: Option<String>,
    added_at: DateTime<Utc>,
}

pub async fn run_library_flow() -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let res = http
        .get(&format!("{}/tracks", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .send()
        .await?;

    if !res.status().is_success() {
        anyhow::bail!("Failed to fetch library: {}", res.status());
    }

    let tracks: Vec<Track> = res.json().await?;

    let mut table = Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL);
    
    table.set_header(vec![
        Cell::new("Artist").add_attribute(Attribute::Bold),
        Cell::new("Title").add_attribute(Attribute::Bold),
        Cell::new("Added At").add_attribute(Attribute::Bold),
    ]);

    for t in tracks {
        table.add_row(vec![
            Cell::new(t.artist.as_deref().unwrap_or("-")),
            Cell::new(t.title.as_deref().unwrap_or("-")),
            Cell::new(t.added_at.format("%Y-%m-%d").to_string()),
        ]);
    }

    println!("{table}");
    Ok(())
}
