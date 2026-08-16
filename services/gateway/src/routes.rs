//! HTTP surface with session-based authentication and ingestion handlers.

use axum::extract::{FromRequestParts, Multipart, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bvault_jobs::{AnalysisJob, JobKind};
use bvault_meta::{Playlist, Track, SearchResult};

use crate::error::{ApiResult, GatewayError};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/tracks", get(list_tracks).post(register_track))
        .route("/playlists", get(list_playlists).post(create_playlist))
        .route("/playlists/{id}", get(get_playlist))
        .route("/playlists/{id}/hashes", get(playlist_hashes))
        .route("/analyze", post(analyze))
        .route("/batches/{id}", get(batch_progress))
        .route("/ingest/upload", post(ingest_upload))
        .route("/ingest/gdrive", post(ingest_gdrive))
        .route("/ingest/ytdlp", post(ingest_ytdlp))
        .route("/search", get(search_ytdlp))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
}

// ---- auth extractor & handlers ------------------------------------------

pub struct AuthUser {
    pub id: Uuid,
}

impl FromRequestParts<AppState> for AuthUser {
    type Rejection = GatewayError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(internal_key) = &state.config.internal_api_key {
            if let Some(req_key) = parts.headers.get("x-internal-key").and_then(|v| v.to_str().ok()) {
                if req_key == internal_key {
                    if let Some(user_id_str) = parts.headers.get("x-user-id").and_then(|v| v.to_str().ok()) {
                        if let Ok(id) = Uuid::parse_str(user_id_str) {
                            return Ok(AuthUser { id });
                        }
                    }
                }
            }
        }

        let auth_header = parts
            .headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|val| val.to_str().ok())
            .ok_or(GatewayError::Unauthorized)?;

        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(GatewayError::Unauthorized)?;

        let token_hash = bvault_auth::hash_token(token);
        let user_id = state
            .meta
            .lookup_session(&token_hash)
            .await?
            .ok_or(GatewayError::Unauthorized)?;

        Ok(AuthUser { id: user_id })
    }
}

#[derive(Deserialize)]
struct Credentials {
    username: String,
    password: String,
}

#[derive(Serialize)]
struct SessionResponse {
    token: String,
    user_id: Uuid,
    expires_at: DateTime<Utc>,
}

const TIMING_DUMMY_HASH: &str =
    "$argon2id$v=19$m=19456,t=2,p=1$uYzlrDGz7giqFQFQGB9H7g$8IO5Da+3v+dqlvrzr/W70Oc9VNNVdLgLs68JOzkC7o8";

async fn register(
    State(st): State<AppState>,
    Json(c): Json<Credentials>,
) -> ApiResult<Json<SessionResponse>> {
    let username = c.username.trim();
    if username.is_empty() || c.password.len() < 8 {
        return Err(GatewayError::BadRequest(
            "username is required and password must be at least 8 characters".into(),
        ));
    }
    let hash =
        bvault_auth::hash_password(&c.password).map_err(|e| GatewayError::Internal(e.to_string()))?;
    let user_id = st.meta.create_user(username, &hash).await?;
    Ok(Json(issue_session(&st, user_id).await?))
}

async fn login(
    State(st): State<AppState>,
    Json(c): Json<Credentials>,
) -> ApiResult<Json<SessionResponse>> {
    let user_id = match st.meta.find_credentials(&c.username).await? {
        Some((id, hash)) if bvault_auth::verify_password(&c.password, &hash) => id,
        Some(_) => return Err(GatewayError::Unauthorized),
        None => {
            let _ = bvault_auth::verify_password(&c.password, TIMING_DUMMY_HASH);
            return Err(GatewayError::Unauthorized);
        }
    };
    Ok(Json(issue_session(&st, user_id).await?))
}

async fn logout(State(st): State<AppState>, headers: HeaderMap) -> ApiResult<StatusCode> {
    if let Some(token) = bearer_token(&headers) {
        st.meta.delete_session(&bvault_auth::hash_token(token)).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn issue_session(st: &AppState, user_id: Uuid) -> ApiResult<SessionResponse> {
    let token = bvault_auth::generate_token();
    let ttl = chrono::Duration::from_std(st.config.session_ttl)
        .unwrap_or_else(|_| chrono::Duration::days(30));
    let expires_at = Utc::now() + ttl;
    st.meta
        .create_session(&bvault_auth::hash_token(&token), user_id, expires_at)
        .await?;
    Ok(SessionResponse {
        token,
        user_id,
        expires_at,
    })
}

fn bearer_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::AUTHORIZATION)?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

// ---- library ------------------------------------------------------------

#[derive(Deserialize)]
struct Pagination {
    #[serde(default = "default_limit")]
    limit: i64,
    #[serde(default)]
    offset: i64,
}
fn default_limit() -> i64 {
    100
}

async fn list_tracks(
    user: AuthUser,
    State(st): State<AppState>,
    Query(p): Query<Pagination>,
) -> ApiResult<Json<Vec<Track>>> {
    Ok(Json(st.meta.list_tracks(Some(user.id), p.limit, p.offset).await?))
}

#[derive(Deserialize)]
struct RegisterTrack {
    hash: String,
    raw_location: String,
    title: Option<String>,
    artist: Option<String>,
}

async fn register_track(
    user: AuthUser,
    State(st): State<AppState>,
    Json(t): Json<RegisterTrack>,
) -> ApiResult<Json<serde_json::Value>> {
    st.meta
        .upsert_track(Some(user.id), &t.hash, &t.raw_location, t.title.as_deref(), t.artist.as_deref())
        .await?;
    Ok(Json(serde_json::json!({ "hash": t.hash })))
}

// ---- playlists ----------------------------------------------------------

#[derive(Deserialize)]
struct CreatePlaylist {
    name: String,
    description: Option<String>,
    #[serde(default)]
    hashes: Vec<String>,
}

#[derive(Serialize)]
struct CreatedId {
    id: Uuid,
}

async fn create_playlist(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<CreatePlaylist>,
) -> ApiResult<Json<CreatedId>> {
    if req.name.trim().is_empty() {
        return Err(GatewayError::BadRequest("playlist name is required".into()));
    }
    let id = st
        .meta
        .create_playlist(Some(user.id), &req.name, req.description.as_deref(), &req.hashes)
        .await?;
    Ok(Json(CreatedId { id }))
}

async fn list_playlists(
    user: AuthUser,
    State(st): State<AppState>,
) -> ApiResult<Json<Vec<Playlist>>> {
    Ok(Json(st.meta.list_playlists(Some(user.id)).await?))
}

async fn get_playlist(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Playlist>> {
    st.meta
        .get_playlist(Some(user.id), id)
        .await?
        .map(Json)
        .ok_or(GatewayError::NotFound)
}

async fn playlist_hashes(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<String>>> {
    Ok(Json(st.meta.playlist_hashes(Some(user.id), id).await?))
}

// ---- analyze + progress -------------------------------------------------

#[derive(Deserialize)]
struct AnalyzeRequest {
    playlist_ids: Vec<Uuid>,
    name: Option<String>,
}

#[derive(Serialize)]
struct AnalyzeResponse {
    batch_id: Uuid,
    total: usize,
    enqueued: usize,
}

async fn analyze(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<AnalyzeRequest>,
) -> ApiResult<Json<AnalyzeResponse>> {
    if req.playlist_ids.is_empty() {
        return Err(GatewayError::BadRequest("no playlists selected".into()));
    }

    let tracks = st.meta.resolve_hashes(Some(user.id), &req.playlist_ids).await?;
    let all_hashes: Vec<String> = tracks.iter().map(|(h, _)| h.clone()).collect();

    let batch_id = st.meta.create_batch(Some(user.id), req.name.as_deref(), &all_hashes).await?;

    let mut enqueued = 0usize;
    for (hash, raw_location) in tracks {
        if st.artifacts.exists(&hash) {
            continue;
        }
        let payload = AnalysisJob {
            hash: hash.clone(),
            raw_location,
            fallback_title: None,
        };
        st.queue
            .enqueue(JobKind::Analysis, &hash, &payload, st.config.analysis_max_attempts)
            .await?;
        enqueued += 1;
    }

    Ok(Json(AnalyzeResponse {
        batch_id,
        total: all_hashes.len(),
        enqueued,
    }))
}

#[derive(Serialize)]
struct BatchProgress {
    id: Uuid,
    name: Option<String>,
    total: i64,
    done: i64,
    failed: i64,
    complete: bool,
}

async fn batch_progress(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<BatchProgress>> {
    let batch = st.meta.get_batch(Some(user.id), id).await?.ok_or(GatewayError::NotFound)?;

    let total = batch.hashes.len() as i64;
    let done = batch
        .hashes
        .iter()
        .filter(|h| st.artifacts.exists(h.as_str()))
        .count() as i64;
    let failed = st.queue.count_dead(JobKind::Analysis, &batch.hashes).await?;

    Ok(Json(BatchProgress {
        id,
        name: batch.name,
        total,
        done,
        failed,
        complete: done + failed >= total,
    }))
}

// ---- ingestion -----------------------------------------------------------

#[derive(Serialize)]
struct IngestResult {
    hash: String,
    raw_location: String,
    title: Option<String>,
    artist: Option<String>,
}

/// Direct binary/multipart audio upload endpoint.
async fn ingest_upload(
    user: AuthUser,
    State(st): State<AppState>,
    mut multipart: Multipart,
) -> ApiResult<Json<IngestResult>> {
    let mut file_bytes: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut title_override: Option<String> = None;
    let mut artist_override: Option<String> = None;

    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|e| GatewayError::BadRequest(e.to_string()))?
    {
        let name = field.name().unwrap_or("").to_string();
        if name == "file" || name == "audio" {
            file_name = field.file_name().map(|s| s.to_string());
            let data = field
                .bytes()
                .await
                .map_err(|e| GatewayError::BadRequest(e.to_string()))?;
            file_bytes = Some(data.to_vec());
        } else if name == "title" {
            if let Ok(text) = field.text().await {
                title_override = Some(text);
            }
        } else if name == "artist" {
            if let Ok(text) = field.text().await {
                artist_override = Some(text);
            }
        }
    }

    let bytes = file_bytes.ok_or_else(|| GatewayError::BadRequest("missing audio file payload".into()))?;
    let hash = bvault_hash::hash_hex(bvault_hash::hash_bytes(&bytes));
    let ext = file_name
        .as_deref()
        .and_then(|n| std::path::Path::new(n).extension())
        .and_then(|e| e.to_str())
        .unwrap_or("mp3");

    let raw_location = st
        .raw
        .write(&hash, ext, &bytes)
        .map_err(|e| GatewayError::Internal(e.to_string()))?;

    let fallback_title = file_name
        .as_deref()
        .map(|n| std::path::Path::new(n).file_stem().unwrap_or_default().to_string_lossy().to_string());

    let final_title = title_override.or(fallback_title);
    let final_artist = artist_override;

    st.meta
        .upsert_track(
            Some(user.id),
            &hash,
            &raw_location,
            final_title.as_deref(),
            final_artist.as_deref(),
        )
        .await?;

    Ok(Json(IngestResult {
        hash,
        raw_location,
        title: final_title,
        artist: final_artist,
    }))
}

#[derive(Deserialize)]
struct GDriveIngestRequest {
    access_token: String,
    folder_id: String,
}

#[derive(Serialize)]
struct GDriveIngestResponse {
    imported_count: usize,
    tracks: Vec<IngestResult>,
}

#[derive(Deserialize)]
struct GDriveFileList {
    files: Option<Vec<GDriveFile>>,
}

#[derive(Deserialize)]
struct GDriveFile {
    id: String,
    name: String,
    #[serde(rename = "mimeType")]
    mime_type: Option<String>,
}

/// Google Drive folder import handler.
async fn ingest_gdrive(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<GDriveIngestRequest>,
) -> ApiResult<Json<GDriveIngestResponse>> {
    let client = reqwest::Client::new();
    let q = format!("'{}' in parents and trashed = false", req.folder_id);

    let list_res: GDriveFileList = client
        .get("https://www.googleapis.com/drive/v3/files")
        .bearer_auth(&req.access_token)
        .query(&[("q", q), ("fields", "files(id, name, mimeType)".to_string())])
        .send()
        .await
        .map_err(|e| GatewayError::Internal(format!("gdrive list error: {e}")))?
        .json()
        .await
        .map_err(|e| GatewayError::Internal(format!("gdrive parse error: {e}")))?;

    let files = list_res.files.unwrap_or_default();
    let mut imported = Vec::new();

    for file in files {
        let is_audio = file
            .mime_type
            .as_deref()
            .map(|m| m.starts_with("audio/") || m == "application/octet-stream")
            .unwrap_or(true);
        if !is_audio {
            continue;
        }

        let download_url = format!("https://www.googleapis.com/drive/v3/files/{}?alt=media", file.id);
        let bytes_res = client
            .get(&download_url)
            .bearer_auth(&req.access_token)
            .send()
            .await;

        let bytes = match bytes_res {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(b) => b,
                Err(_) => continue,
            },
            _ => continue,
        };

        let hash = bvault_hash::hash_hex(bvault_hash::hash_bytes(&bytes));
        let ext = std::path::Path::new(&file.name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mp3");

        if let Ok(raw_location) = st.raw.write(&hash, ext, &bytes) {
            let title = std::path::Path::new(&file.name)
                .file_stem()
                .map(|s| s.to_string_lossy().to_string());
            let _ = st
                .meta
                .upsert_track(Some(user.id), &hash, &raw_location, title.as_deref(), None)
                .await;

            imported.push(IngestResult {
                hash,
                raw_location,
                title,
                artist: None,
            });
        }
    }

    Ok(Json(GDriveIngestResponse {
        imported_count: imported.len(),
        tracks: imported,
    }))
}

#[derive(Deserialize)]
struct YtDlpIngestRequest {
    url: String,
}

#[derive(Serialize)]
struct YtDlpIngestResponse {
    status: String,
    message: String,
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
    #[serde(default = "default_search_limit")]
    limit: i64,
}
fn default_search_limit() -> i64 { 10 }

/// Trigger yt-dlp microservice audio extraction and ingestion.
async fn ingest_ytdlp(
    user: AuthUser,
    State(st): State<AppState>,
    Json(req): Json<YtDlpIngestRequest>,
) -> ApiResult<Json<YtDlpIngestResponse>> {
    let service_url = st
        .config
        .yt_dlp_service_url
        .as_deref()
        .ok_or_else(|| GatewayError::BadRequest("yt-dlp ingestion service is not configured".into()))?;

    let client = reqwest::Client::new();
    let payload = serde_json::json!({
        "url": req.url,
        "user_id": user.id,
    });

    let resp = client
        .post(format!("{}/extract", service_url.trim_end_matches('/')))
        .json(&payload)
        .send()
        .await
        .map_err(|e| GatewayError::Internal(format!("yt-dlp service error: {e}")))?;

    if !resp.status().is_success() {
        return Err(GatewayError::Internal("yt-dlp extraction job failed to start".into()));
    }

    Ok(Json(YtDlpIngestResponse {
        status: "accepted".into(),
        message: "yt-dlp audio ingestion initiated".into(),
    }))
}

async fn search_ytdlp(
    _user: AuthUser,
    State(st): State<AppState>,
    Query(query): Query<SearchQuery>,
) -> ApiResult<Json<Vec<SearchResult>>> {
    let service_url = st.config.yt_dlp_service_url.as_deref()
        .ok_or_else(|| GatewayError::BadRequest("yt-dlp service not configured".into()))?;

    let client = reqwest::Client::new();
    let mut req = client
        .get(format!("{}/search", service_url.trim_end_matches('/')))
        .query(&[("q", &query.q), ("limit", &query.limit.to_string())]);

    if let Some(key) = &st.config.internal_api_key {
        req = req.header("X-Internal-Key", key);   // symmetric internal auth
    }

    let resp = req.send().await
        .map_err(|e| GatewayError::Internal(format!("yt-dlp search error: {e}")))?;
    if !resp.status().is_success() {
        return Err(GatewayError::Internal("yt-dlp search request failed".into()));
    }
    let results = resp.json().await
        .map_err(|e| GatewayError::Internal(format!("yt-dlp search parse error: {e}")))?;
    Ok(Json(results))
}