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
struct GDriveFiles {
    files: Option<Vec<GDriveFileId>>,
}

#[derive(Deserialize)]
struct GDriveFileId {
    id: String,
}

#[derive(Deserialize)]
struct IngestResult {
    title: Option<String>,
}

#[derive(Deserialize)]
struct IngestAccepted {
    job_id: i64,
}

#[derive(Deserialize)]
struct JobStatusResponse {
    status: String,
    error: Option<String>,
}

pub async fn run_ingest_flow(query: &str, youtube: bool, local: bool, gdrive: bool) -> Result<()> {
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style());
    pb.enable_steady_tick(Duration::from_millis(100));

    if local {
        pb.set_message("Uploading local file...");
        let bytes = tokio::fs::read(query).await.context("Failed to read local file")?;
        let file_name = std::path::Path::new(query)
            .file_name().unwrap_or_default().to_string_lossy().to_string();
        let part = reqwest::multipart::Part::bytes(bytes).file_name(file_name);
        let form = reqwest::multipart::Form::new().part("file", part);

        let res = http.post(format!("{}/ingest/upload", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .multipart(form).send().await?;
        if !res.status().is_success() {
            let status = res.status(); let _ = res.text().await;
            pb.finish_with_message(format!("✗ Upload failed: {}", status));
            anyhow::bail!("upload failed");
        }
        // Local upload is synchronous: the response *is* the result.
        let ingested: IngestResult = res.json().await?;
        pb.finish_with_message(format!("✓ ingested: {}", ingested.title.as_deref().unwrap_or("Unknown Title")));
        return Ok(());
    }

    if gdrive {
        pb.finish_and_clear();
        let access_token = google_oauth_flow().await?;
        let pb = ProgressBar::new_spinner();
        pb.set_style(spinner_style());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_message(format!("Resolving Google Drive path: {}...", query));
        let folder_id = resolve_gdrive_path(&http, &access_token, query).await?;
        pb.set_message("Importing from Google Drive...");
        let payload = serde_json::json!({ "access_token": access_token, "folder_id": folder_id });
        let res = http.post(format!("{}/ingest/gdrive", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .json(&payload).send().await?;
        if !res.status().is_success() {
            let status = res.status(); let _ = res.text().await;
            pb.finish_with_message(format!("✗ Import failed: {}", status));
            anyhow::bail!("gdrive import failed");
        }
        pb.finish_with_message("✓ Google Drive folder import started! Tracks appear as they process.");
        return Ok(());
    }

    // youtube (with search) or a direct URL — both go through /ingest/ytdlp,
    // which now returns a job id we poll to real terminal state.
    let target_url = if youtube {
        pb.finish_and_clear();
        println!("Searching YouTube for: {}", query);
        let res = http.get(format!("{}/search", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .query(&[("q", query), ("limit", "5")]).send().await?;
        if !res.status().is_success() { anyhow::bail!("Search failed: {}", res.status()); }
        let results: Vec<SearchResult> = res.json().await?;
        if results.is_empty() { anyhow::bail!("No results found for '{}'", query); }
        println!("Top results:");
        let items: Vec<String> = results.iter().map(|r| format!("{} ({})", r.title, r.uploader)).collect();
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Choose a track to ingest:").default(0).items(&items).interact()?;
        let chosen = &results[selection];
        println!("✓ {} chosen (\"{}\")", chosen.title, chosen.url);
        chosen.url.clone()
    } else {
        query.to_string()
    };

    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style());
    pb.enable_steady_tick(Duration::from_millis(100));
    pb.set_message("downloading & analyzing...");

    let res = http.post(format!("{}/ingest/ytdlp", base_url))
        .header("Authorization", format!("Bearer {}", session.token))
        .json(&serde_json::json!({ "url": target_url })).send().await?;
    if !res.status().is_success() {
        let status = res.status(); let _ = res.text().await;
        pb.finish_with_message(format!("✗ Ingest failed to start: {}", status));
        anyhow::bail!("ingest failed to start");
    }
    let accepted: IngestAccepted = res.json().await?;

    // Poll to a terminal state, with a timeout backstop so a dead background
    // job can't hang the CLI forever.
    let deadline = std::time::Instant::now() + Duration::from_secs(300);
    loop {
        if std::time::Instant::now() >= deadline {
            pb.finish_with_message("✗ Timed out waiting for ingest. Check yt-dlp-ingest logs.");
            anyhow::bail!("ingest timed out after 300s");
        }
        sleep(Duration::from_secs(2)).await;
        let status: JobStatusResponse = http.get(format!("{}/jobs/{}", base_url, accepted.job_id))
            .header("Authorization", format!("Bearer {}", session.token))
            .send().await?.json().await?;
        match status.status.as_str() {
            "succeeded" => { pb.finish_with_message("✓ ingested successfully"); break; }
            "failed" | "dead" => {
                pb.finish_with_message(format!("✗ Ingest failed: {}",
                    status.error.unwrap_or_else(|| "unknown error".into())));
                anyhow::bail!("ingest failed");
            }
            _ => {} // pending / running — keep waiting
        }
    }
    Ok(())
}

fn spinner_style() -> ProgressStyle {
    ProgressStyle::default_spinner()
        .tick_chars("⠁⠂⠄⡀⢀⠠⠐⠈ ")
        .template("{spinner:.green} {msg}")
        .unwrap()
}

async fn resolve_gdrive_path(http: &Client, access_token: &str, path: &str) -> Result<String> {
    if !path.contains('/') {
        return Ok(path.to_string());
    }

    let mut current_parent = "root".to_string();
    let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    
    if segments.is_empty() {
        return Ok("root".to_string());
    }

    for segment in segments {
        // Query to find a folder with the exact name inside the current_parent
        let q = format!("'{}' in parents and name = '{}' and trashed = false and mimeType = 'application/vnd.google-apps.folder'", current_parent, segment);
        
        let res = http.get("https://www.googleapis.com/drive/v3/files")
            .bearer_auth(access_token)
            .query(&[("q", q), ("fields", "files(id)".to_string())])
            .send()
            .await?;
            
        if !res.status().is_success() {
            anyhow::bail!("Failed to query Google Drive API: {}", res.status());
        }
        
        let data: GDriveFiles = res.json().await?;
        if let Some(files) = data.files {
            if let Some(first_file) = files.first() {
                current_parent = first_file.id.clone();
            } else {
                anyhow::bail!("Folder '{}' not found in path '{}'", segment, path);
            }
        } else {
            anyhow::bail!("Folder '{}' not found in path '{}'", segment, path);
        }
    }
    
    Ok(current_parent)
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
