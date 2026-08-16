use anyhow::{Context, Result};
use dialoguer::{theme::ColorfulTheme, Select};
use indicatif::{ProgressBar, ProgressStyle};
use reqwest::Client;
use serde::Deserialize;
use std::time::Duration;
use tokio::time::sleep;

use oauth2::{
    basic::BasicClient, AuthUrl, ClientId, ClientSecret, CsrfToken, RedirectUrl, Scope, TokenResponse, TokenUrl,
};
use tiny_http::{Response, Server};

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

pub async fn run_ingest_flow(query: &str, youtube: bool, local: bool, gdrive: bool) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::default_spinner()
            .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
            .template("{spinner:.green} {msg}")
            .unwrap(),
    );
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

    let ingest_res = if local {
        pb.set_message("Uploading local file...");
        let bytes = tokio::fs::read(query).await.context("Failed to read local file")?;
        let file_name = std::path::Path::new(query)
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
            
        let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
        let form = reqwest::multipart::Form::new().part("file", part);

        http.post(&format!("{}/ingest/upload", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .multipart(form)
            .send()
            .await?
    } else if gdrive {
        pb.finish_and_clear();
        let access_token = google_oauth_flow().await?;
        
        // pb was finished, recreate it
        let pb = ProgressBar::new_spinner();
        pb.set_style(
            ProgressStyle::default_spinner()
                .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
                .template("{spinner:.green} {msg}")
                .unwrap(),
        );
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_message("Importing from Google Drive...");

        let payload = serde_json::json!({
            "access_token": access_token,
            "folder_id": query,
        });

        http.post(&format!("{}/ingest/gdrive", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .json(&payload)
            .send()
            .await?
    } else {
        // default / youtube flow
        let target_url = if youtube {
            pb.finish_and_clear(); // clear pb to show prompt
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
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_message("downloading & analyzing...");

        let payload = serde_json::json!({ "url": target_url });
        http.post(&format!("{}/ingest/ytdlp", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .json(&payload)
            .send()
            .await?
    };

    if !ingest_res.status().is_success() {
        let status = ingest_res.status();
        let _ = ingest_res.text().await; // consume error body if any
        pb.finish_with_message(format!("✗ Ingest failed: {}", status));
        anyhow::bail!("Ingest failed");
    }

    if gdrive {
        pb.finish_with_message("✓ Google drive folder import started! Tracks will appear in library as they are processed.");
        return Ok(());
    }

    // Poll until a new track appears (only for single track uploads/ytdlp)
    pb.set_message("Waiting for processing to finish...");
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

async fn google_oauth_flow() -> Result<String> {
    let client_id = ClientId::new(include_str!("../gdrive_client_id.txt").trim().to_string());
    let client_secret = ClientSecret::new(include_str!("../gdrive_client_secret.txt").trim().to_string());
    
    // We bind to a fixed port 8081 for the redirect URI
    let redirect_uri = RedirectUrl::new("http://localhost:8081".to_string())?;
    
    let auth_url = AuthUrl::new("https://accounts.google.com/o/oauth2/v2/auth".to_string())?;
    let token_url = TokenUrl::new("https://oauth2.googleapis.com/token".to_string())?;
    
    let client = BasicClient::new(client_id, Some(client_secret), auth_url, Some(token_url))
        .set_redirect_uri(redirect_uri);
        
    let (authorize_url, _csrf_state) = client
        .authorize_url(CsrfToken::new_random)
        .add_scope(Scope::new("https://www.googleapis.com/auth/drive.readonly".to_string()))
        .url();
        
    println!("Opening your browser to authenticate with Google Drive...");
    if webbrowser::open(authorize_url.as_str()).is_err() {
        println!("Please open this URL in your browser:\n{}", authorize_url);
    }
    
    // Start local server to catch the callback
    // Use tokio spawn_blocking so we don't block the async runtime
    let access_token = tokio::task::spawn_blocking(move || -> Result<String> {
        let server = Server::http("127.0.0.1:8081").map_err(|e| anyhow::anyhow!("Failed to bind server: {}", e))?;
        for request in server.incoming_requests() {
            let url = request.url().to_string();
            if url.starts_with("/?") {
                let code = url.split("code=").nth(1).and_then(|s| s.split('&').next());
                if let Some(c) = code {
                    let response = Response::from_string("Success! You can close this tab and return to the terminal.");
                    let _ = request.respond(response);
                    
                    // Since we are inside spawn_blocking, we must block on the async exchange request
                    let rt = tokio::runtime::Handle::current();
                    let token_res = rt.block_on(async {
                        client
                            .exchange_code(oauth2::AuthorizationCode::new(c.to_string()))
                            .request_async(oauth2::reqwest::async_http_client)
                            .await
                    })?;
                    return Ok(token_res.access_token().secret().clone());
                }
            }
            let response = Response::from_string("Invalid request or authorization failed.");
            let _ = request.respond(response);
        }
        anyhow::bail!("Server closed before authorization completed")
    })
    .await??;
    
    Ok(access_token)
}
