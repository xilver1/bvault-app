use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use comfy_table::{Attribute, Cell, ContentArrangement, Table};
use reqwest::Client;
use serde::Deserialize;

use crate::client::{get_api_url, load_session};

/// A high limit so `library` shows the whole library, not just the first page
/// (the server still paginates; this is simply "give me everything").
const LIBRARY_LIMIT: i64 = 100_000;

/// Below this terminal width the table doesn't fit (Android terminals sit around
/// 40–55 columns), so we switch to a stacked one-track-per-block layout.
const NARROW_COLS: u16 = 62;

#[derive(Deserialize)]
struct Track {
    title: Option<String>,
    added_at: DateTime<Utc>,
    duration_secs: Option<f64>,
    bpm: Option<f64>,
    bitrate: Option<u32>,
    size_bytes: Option<u64>,
}

pub async fn run_library_flow(search: Option<&str>) -> Result<()> {
    let session =
        load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let mut req = http
        .get(format!("{}/tracks", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .query(&[("limit", LIBRARY_LIMIT.to_string())]);
    if let Some(q) = search.map(str::trim).filter(|s| !s.is_empty()) {
        req = req.query(&[("q", q)]);
    }

    let res = req.send().await?;
    if !res.status().is_success() {
        anyhow::bail!("Failed to fetch library: {}", res.status());
    }
    let tracks: Vec<Track> = res.json().await?;

    if tracks.is_empty() {
        match search {
            Some(q) => println!("No tracks match \"{}\".", q),
            None => println!("Your library is empty. Ingest something with `bvault ingest`."),
        }
        return Ok(());
    }

    if term_width() < NARROW_COLS {
        print_stacked(&tracks);
    } else {
        print_table(&tracks);
    }
    Ok(())
}

fn print_table(tracks: &[Track]) {
    let mut table = Table::new();
    table.load_preset(comfy_table::presets::UTF8_FULL);
    // Dynamic arrangement wraps the (wide) Title column to whatever width the
    // terminal actually has, so even a borderline-width screen stays readable.
    table.set_content_arrangement(ContentArrangement::Dynamic);

    table.set_header(vec![
        Cell::new("Title").add_attribute(Attribute::Bold),
        Cell::new("Length").add_attribute(Attribute::Bold),
        Cell::new("BPM").add_attribute(Attribute::Bold),
        Cell::new("Kbps").add_attribute(Attribute::Bold),
        Cell::new("MB").add_attribute(Attribute::Bold),
        Cell::new("Added").add_attribute(Attribute::Bold),
    ]);

    for t in tracks {
        table.add_row(vec![
            Cell::new(t.title.as_deref().unwrap_or("-")),
            Cell::new(fmt_len(t.duration_secs)),
            Cell::new(fmt_bpm(t.bpm)),
            Cell::new(fmt_bitrate(t.bitrate)),
            Cell::new(fmt_mb(t.size_bytes)),
            Cell::new(t.added_at.format("%Y-%m-%d").to_string()),
        ]);
    }

    println!("{table}");
    println!("{} track(s).", tracks.len());
}

/// Narrow-screen layout: one track per block, metadata on a single dot-separated
/// line under the title. Fits comfortably on a phone terminal.
fn print_stacked(tracks: &[Track]) {
    for t in tracks {
        println!("{}", t.title.as_deref().unwrap_or("-"));
        println!(
            "  {} · {} · {} · {} · {}",
            fmt_len(t.duration_secs),
            fmt_bpm(t.bpm),
            fmt_bitrate(t.bitrate),
            fmt_mb(t.size_bytes),
            t.added_at.format("%Y-%m-%d"),
        );
    }
    println!("\n{} track(s).", tracks.len());
}

fn term_width() -> u16 {
    terminal_size::terminal_size()
        .map(|(terminal_size::Width(w), _)| w)
        .unwrap_or(100) // not a tty (piped) → assume wide, print the full table
}

fn fmt_len(secs: Option<f64>) -> String {
    match secs {
        Some(s) => {
            let total = s.round() as u64;
            format!("{}:{:02}", total / 60, total % 60)
        }
        None => "—".into(),
    }
}

fn fmt_bpm(bpm: Option<f64>) -> String {
    bpm.map(|b| format!("{:.0}", b))
        .unwrap_or_else(|| "—".into())
}

fn fmt_bitrate(kbps: Option<u32>) -> String {
    kbps.map(|b| b.to_string()).unwrap_or_else(|| "—".into())
}

fn fmt_mb(bytes: Option<u64>) -> String {
    bytes
        .map(|b| format!("{:.1}", b as f64 / (1024.0 * 1024.0)))
        .unwrap_or_else(|| "—".into())
}
