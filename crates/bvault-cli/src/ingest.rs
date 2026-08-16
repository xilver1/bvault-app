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

#[derive(Deserialize)]
struct YTPlaylistsResponse {
    items: Option<Vec<YTPlaylist>>,
}

#[derive(Deserialize)]
struct YTPlaylist {
    id: String,
    snippet: YTPlaylistSnippet,
}

#[derive(Deserialize)]
struct YTPlaylistSnippet {
    title: String,
    #[serde(rename = "channelTitle")]
    channel_title: String,
}

#[derive(Deserialize)]
struct YTPlaylistItemsResponse {
    items: Option<Vec<YTPlaylistItem>>,
    #[serde(rename = "nextPageToken")]
    next_page_token: Option<String>,
}

#[derive(Deserialize)]
struct YTPlaylistItem {
    snippet: YTPlaylistItemSnippet,
}

#[derive(Deserialize)]
struct YTPlaylistItemSnippet {
    #[serde(rename = "resourceId")]
    resource_id: YTResourceId,
    title: String,
}

#[derive(Deserialize)]
struct YTResourceId {
    #[serde(rename = "videoId")]
    video_id: String,
}

pub async fn run_ingest_flow(query: Option<&str>, youtube: bool, local: bool, gdrive: bool, youtube_sso: bool, youtube_playlist: bool) -> Result<()> {
    let query_str = query.unwrap_or_default();
    let session = load_session().context("You are not logged in. Please run `bvault login` first.")?;
    let base_url = get_api_url();
    let http = Client::new();

    let pb = ProgressBar::new_spinner();
    pb.set_style(spinner_style());
    pb.enable_steady_tick(Duration::from_millis(100));

    if local {
        pb.set_message("Uploading local file...");
        let bytes = tokio::fs::read(query_str).await.context("Failed to read local file")?;
        let file_name = std::path::Path::new(query_str)
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
        let ingested: IngestResult = res.json().await?;
        pb.finish_with_message(format!("✓ ingested: {}", ingested.title.as_deref().unwrap_or("Unknown Title")));
        return Ok(());
    }

    if gdrive {
        pb.finish_and_clear();
        let access_token = google_oauth_flow("https://www.googleapis.com/auth/drive.readonly").await?;
        let pb = ProgressBar::new_spinner();
        pb.set_style(spinner_style());
        pb.enable_steady_tick(Duration::from_millis(100));
        pb.set_message(format!("Resolving Google Drive path: {}...", query_str));
        let folder_id = resolve_gdrive_path(&http, &access_token, query_str).await?;
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

    if youtube_sso || youtube_playlist {
        pb.finish_and_clear();
        let access_token = google_oauth_flow("https://www.googleapis.com/auth/youtube.readonly").await?;
        
        let playlist_id = if youtube_sso {
            let res = http.get("https://www.googleapis.com/youtube/v3/playlists")
                .bearer_auth(&access_token)
                .query(&[("part", "snippet"), ("mine", "true"), ("maxResults", "50")])
                .send().await?;
            if !res.status().is_success() {
                anyhow::bail!("Failed to fetch YouTube playlists: {}", res.status());
            }
            let data: YTPlaylistsResponse = res.json().await?;
            let items = data.items.unwrap_or_default();
            if items.is_empty() {
                anyhow::bail!("No YouTube playlists found on your account.");
            }
            let display_items: Vec<String> = items.iter().map(|p| format!("{} ({})", p.snippet.title, p.snippet.channel_title)).collect();
            let selection = Select::with_theme(&ColorfulTheme::default())
                .with_prompt("Select a YouTube playlist:")
                .default(0)
                .items(&display_items)
                .interact()?;
            println!("✓ Selected playlist: {}", items[selection].snippet.title);
            items[selection].id.clone()
        } else {
            let query = query.context("Playlist URL is required for --youtube-playlist")?;
            let re = regex::Regex::new(r"[?&]list=([^&]+)").unwrap();
            let caps = re.captures(query).context("Invalid YouTube playlist URL (missing list=...)")?;
            caps.get(1).unwrap().as_str().to_string()
        };

        println!("Fetching videos for playlist ID: {}", playlist_id);
        let mut video_ids = Vec::new();
        let mut page_token: Option<String> = None;
        loop {
            let mut req = http.get("https://www.googleapis.com/youtube/v3/playlistItems")
                .bearer_auth(&access_token)
                .query(&[
                    ("part", "snippet"), 
                    ("playlistId", &playlist_id), 
                    ("maxResults", "50")
                ]);
            if let Some(pt) = &page_token {
                req = req.query(&[("pageToken", pt)]);
            }
            let res = req.send().await?;
            if !res.status().is_success() {
                let err = res.text().await?;
                anyhow::bail!("Failed to fetch playlist items: {}", err);
            }
            let data: YTPlaylistItemsResponse = res.json().await?;
            if let Some(items) = data.items {
                for item in items {
                    if item.snippet.title != "Private video" && item.snippet.title != "Deleted video" {
                        video_ids.push(item.snippet.resource_id.video_id);
                    }
                }
            }
            page_token = data.next_page_token;
            if page_token.is_none() {
                break;
            }
        }

        if video_ids.is_empty() {
            anyhow::bail!("Playlist is empty or all videos are private/deleted.");
        }
        
        println!("Submitting {} videos for ingestion...", video_ids.len());
        let mut job_ids = Vec::new();
        let pb_submit = indicatif::ProgressBar::new(video_ids.len() as u64);
        for vid in &video_ids {
            let target_url = format!("https://www.youtube.com/watch?v={}", vid);
            let res = http.post(format!("{}/ingest/ytdlp", base_url))
                .header("Authorization", format!("Bearer {}", session.token))
                .json(&serde_json::json!({ "url": target_url })).send().await;
            if let Ok(res) = res {
                if res.status().is_success() {
                    if let Ok(accepted) = res.json::<IngestAccepted>().await {
                        job_ids.push(accepted.job_id);
                    }
                }
            }
            pb_submit.inc(1);
        }
        pb_submit.finish_with_message("All jobs submitted.");

        println!("Waiting for {} background jobs to complete...", job_ids.len());
        let pb_batch = indicatif::ProgressBar::new(job_ids.len() as u64);
        pb_batch.set_style(ProgressStyle::default_bar()
            .template("[{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} ({msg})")
            .unwrap()
            .progress_chars("#>-"));

        let mut completed_jobs = std::collections::HashSet::new();
        let mut failed = 0;
        
        loop {
            if completed_jobs.len() == job_ids.len() {
                break;
            }
            let mut progress_made = false;
            for &jid in &job_ids {
                if completed_jobs.contains(&jid) { continue; }
                let res = http.get(format!("{}/jobs/{}", base_url, jid))
                    .header("Authorization", format!("Bearer {}", session.token))
                    .send().await;
                if let Ok(r) = res {
                    if let Ok(status) = r.json::<JobStatusResponse>().await {
                        match status.status.as_str() {
                            "succeeded" => {
                                completed_jobs.insert(jid);
                                pb_batch.inc(1);
                                progress_made = true;
                            }
                            "failed" | "dead" => {
                                completed_jobs.insert(jid);
                                failed += 1;
                                pb_batch.inc(1);
                                progress_made = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
            pb_batch.set_message(format!("{} failed", failed));
            if !progress_made {
                sleep(Duration::from_secs(3)).await;
            }
        }
        pb_batch.finish_with_message(format!("Batch complete. {} failed.", failed));
        return Ok(());
    }

    let target_url = if youtube {
        pb.finish_and_clear();
        println!("Searching YouTube for: {}", query_str);
        let res = http.get(format!("{}/search", base_url))
            .header("Authorization", format!("Bearer {}", session.token))
            .query(&[("q", query_str), ("limit", "5")]).send().await?;
        if !res.status().is_success() { anyhow::bail!("Search failed: {}", res.status()); }
        let results: Vec<SearchResult> = res.json().await?;
        if results.is_empty() { anyhow::bail!("No results found for '{}'", query_str); }
        println!("Top results:");
        let items: Vec<String> = results.iter().map(|r| format!("{} ({})", r.title, r.uploader)).collect();
        let selection = Select::with_theme(&ColorfulTheme::default())
            .with_prompt("Choose a track to ingest:").default(0).items(&items).interact()?;
        let chosen = &results[selection];
        println!("✓ {} chosen (\"{}\")", chosen.title, chosen.url);
        chosen.url.clone()
    } else {
        query_str.to_string()
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
            _ => {}
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

async fn google_oauth_flow(scope_url: &str) -> Result<String> {
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
        .add_scope(Scope::new(scope_url.to_string()))
        .url();
        
    println!("Opening your browser to authenticate...");
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
