use axum::extract::{DefaultBodyLimit, FromRequestParts, Multipart, Path, Query, State};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bvault_jobs::{AnalysisJob, Job, JobKind, JobStatus, YtDlpIngestJob};
use bvault_meta::{Playlist, SearchResult};

use crate::error::{ApiResult, GatewayError};
use crate::state::AppState;

/// Audio uploads are large (lossless DJ files, not just mp3), so the upload
/// route opts out of axum's 2 MiB default. Other routes keep the small default.
const MAX_UPLOAD_BYTES: usize = 200 * 1024 * 1024; // 200 MiB

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/auth/register", post(register))
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/tracks", get(list_tracks).post(register_track))
        .route("/playlists", get(list_playlists).post(create_playlist))
        .route("/playlists/{id}", get(get_playlist).delete(delete_playlist_endpoint))
        .route("/playlists/{id}/hashes", get(playlist_hashes))
        .route("/playlists/{id}/remove", post(remove_playlist_tracks_endpoint))
        .route("/analyze", post(analyze))
        .route("/batches/{id}", get(batch_progress))
        .route(
            "/ingest/upload",
            post(ingest_upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route("/ingest/gdrive", post(ingest_gdrive))
        .route("/ingest/ytdlp", post(ingest_ytdlp))
        .route("/ingest/ytdlp/completed", get(completed_ytdlp))
        .route("/search", get(search_ytdlp))
        .route("/jobs/{id}", get(get_job))
        .route("/internal/jobs/{id}", post(report_job))
        .route("/internal/jobs/claim", post(claim_job))
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

pub struct InternalAuth;

impl FromRequestParts<AppState> for InternalAuth {
    type Rejection = GatewayError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Self::Rejection> {
        if let Some(internal_key) = &state.config.internal_api_key {
            if let Some(req_key) = parts.headers.get("x-internal-key").and_then(|v| v.to_str().ok()) {
                if req_key == internal_key {
                    return Ok(InternalAuth);
                }
            }
        }
        Err(GatewayError::Unauthorized)
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
    /// Optional case-insensitive title substring filter (library search).
    #[serde(default)]
    q: Option<String>,
}
fn default_limit() -> i64 {
    100
}

/// Enriched track row for the library listing: DB display metadata plus, when a
/// track has been analyzed, duration/bpm/bitrate/size read from the artifact
/// store's `analysis.json` (store presence = truth). `size_bytes` still resolves
/// for un-analyzed tracks by stat-ing the raw file. Artist is intentionally
/// dropped — for yt-dlp ingests it's the uploading channel, not the artist, and
/// there is no reliable rule to recover it from the title.
#[derive(Serialize)]
struct TrackView {
    hash: String,
    title: Option<String>,
    added_at: DateTime<Utc>,
    duration_secs: Option<f64>,
    bpm: Option<f64>,
    bitrate: Option<u32>,
    size_bytes: Option<u64>,
}

/// The few fields we read out of `analysis.json`. serde ignores the rest, so the
/// gateway needn't depend on the full analysis/core crate to read a summary.
#[derive(Deserialize)]
struct AnalysisSummary {
    duration_secs: f64,
    bpm: f64,
    bitrate: u32,
    file_size: u64,
}

async fn list_tracks(
    user: AuthUser,
    State(st): State<AppState>,
    Query(p): Query<Pagination>,
) -> ApiResult<Json<Vec<TrackView>>> {
    let q = p.q.as_deref().map(str::trim).filter(|s| !s.is_empty());
    let tracks = st.meta.list_tracks(Some(user.id), p.limit, p.offset, q).await?;

    let mut out = Vec::with_capacity(tracks.len());
    for t in tracks {
        // Analysis-derived fields only exist once the track is analyzed; absence
        // of the artifact is simply "not analyzed yet", shown as blanks.
        let summary = st
            .artifacts
            .get(&t.hash, "analysis.json")
            .ok()
            .and_then(|bytes| serde_json::from_slice::<AnalysisSummary>(&bytes).ok());

        let (duration_secs, bpm, bitrate, mut size_bytes) = match summary {
            Some(s) => (Some(s.duration_secs), Some(s.bpm), Some(s.bitrate), Some(s.file_size)),
            None => (None, None, None, None),
        };

        // Size is cheap to show even without analysis: stat the raw file.
        if size_bytes.is_none() {
            if let Ok(path) = st.raw.resolve(&t.raw_location) {
                if let Ok(md) = std::fs::metadata(path) {
                    size_bytes = Some(md.len());
                }
            }
        }

        out.push(TrackView {
            hash: t.hash,
            title: t.title,
            added_at: t.added_at,
            duration_secs,
            bpm,
            bitrate,
            size_bytes,
        });
    }
    Ok(Json(out))
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

async fn delete_playlist_endpoint(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    st.meta.delete_playlist(Some(user.id), id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct RemoveTracksRequest {
    hashes: Vec<String>,
}

async fn remove_playlist_tracks_endpoint(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
    Json(req): Json<RemoveTracksRequest>,
) -> ApiResult<StatusCode> {
    st.meta.remove_playlist_tracks(Some(user.id), id, &req.hashes).await?;
    Ok(StatusCode::NO_CONTENT)
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

    if !st.artifacts.exists(&hash) {
        let payload = AnalysisJob {
            hash: hash.clone(),
            raw_location: raw_location.clone(),
            fallback_title: final_title.clone(),
        };
        let _ = st.queue
            .enqueue(JobKind::Analysis, &hash, &payload, st.config.analysis_max_attempts)
            .await;
    }

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
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| GatewayError::Internal(format!("client err: {e}")))?;
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

        let mut download_url = format!("https://www.googleapis.com/drive/v3/files/{}?alt=media", file.id);
        let mut bytes_res = client
            .get(&download_url)
            .bearer_auth(&req.access_token)
            .send()
            .await;

        let mut redirects = 0;
        while let Ok(r) = &bytes_res {
            if r.status().is_redirection() && redirects < 5 {
                if let Some(loc) = r.headers().get(reqwest::header::LOCATION) {
                    if let Ok(loc_str) = loc.to_str() {
                        download_url = loc_str.to_string();
                        bytes_res = client.get(&download_url).bearer_auth(&req.access_token).send().await;
                        redirects += 1;
                        continue;
                    }
                }
            }
            break;
        }

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

            if !st.artifacts.exists(&hash) {
                let payload = AnalysisJob {
                    hash: hash.clone(),
                    raw_location: raw_location.clone(),
                    fallback_title: title.clone(),
                };
                let _ = st.queue
                    .enqueue(JobKind::Analysis, &hash, &payload, st.config.analysis_max_attempts)
                    .await;
            }

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
    job_id: i64,
    status: String,
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
    let _service_url = st.config.yt_dlp_service_url.as_deref()
        .ok_or_else(|| GatewayError::BadRequest("yt-dlp ingestion service is not configured".into()))?;

    // Record the run so the CLI can poll it. dedup on (user, url) collapses a
    // repeat request while one is in flight into the same job (no double download).
    let dedup_key = format!("{}:{}", user.id, req.url);
    let payload = YtDlpIngestJob { url: req.url.clone(), user_id: user.id.to_string() };
    let job_id = st.queue.enqueue(JobKind::YtDlpIngest, &dedup_key, &payload, 1).await?;

    Ok(Json(YtDlpIngestResponse { job_id, status: "accepted".into() }))
}

/// URLs this user has already ingested via yt-dlp (jobs in terminal `succeeded`
/// state). The CLI diffs a playlist against this so a resumed run only submits
/// tracks that never finished — without it, job dedup is in-flight only, so a
/// re-run re-downloads and re-transcodes everything already done.
async fn completed_ytdlp(
    user: AuthUser,
    State(st): State<AppState>,
) -> ApiResult<Json<Vec<String>>> {
    let prefix = format!("{}:", user.id);
    let keys = st.queue.completed_keys(JobKind::YtDlpIngest, &prefix).await?;
    let urls = keys
        .into_iter()
        .filter_map(|k| k.strip_prefix(&prefix).map(str::to_string))
        .collect();
    Ok(Json(urls))
}

#[derive(Serialize)]
struct JobStatusResponse {
    status: JobStatus,
    error: Option<String>,
}

/// Poll one ingest job's terminal state. Scoped to the owner via the payload's
/// user_id (yt-dlp jobs carry it); anything else 404s.
async fn get_job(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<i64>,
) -> ApiResult<Json<JobStatusResponse>> {
    let job = st.queue.get(id).await?.ok_or(GatewayError::NotFound)?;
    if !job_owned_by(&job, &user) {
        return Err(GatewayError::NotFound); // 404 not 403, to avoid id enumeration
    }
    Ok(Json(JobStatusResponse { status: job.status, error: job.last_error }))
}

#[derive(Deserialize)]
struct JobReport {
    ok: bool,
    error: Option<String>,
}

/// Internal callback from the yt-dlp service to flip a job terminal. Auth rides
/// the same internal-key path as AuthUser; ownership is re-checked so a stray
/// token can only touch its own jobs.
async fn report_job(
    user: AuthUser,
    State(st): State<AppState>,
    Path(id): Path<i64>,
    Json(r): Json<JobReport>,
) -> ApiResult<StatusCode> {
    let job = st.queue.get(id).await?.ok_or(GatewayError::NotFound)?;
    if !job_owned_by(&job, &user) {
        return Err(GatewayError::NotFound);
    }
    if r.ok {
        st.queue.complete(id).await?;
    } else {
        st.queue.mark_dead(id, r.error.as_deref().unwrap_or("yt-dlp ingest failed")).await?;
    }
    Ok(StatusCode::NO_CONTENT)
}

fn job_owned_by(job: &Job, user: &AuthUser) -> bool {
    job.payload.0.get("user_id").and_then(|v| v.as_str())
        == Some(user.id.to_string().as_str())
}

#[derive(Deserialize)]
struct ClaimRequest {
    kind: JobKind,
    lease_secs: u64,
}

#[derive(Serialize)]
struct ClaimResponse {
    id: i64,
    payload: serde_json::Value,
}

async fn claim_job(
    _auth: InternalAuth,
    State(st): State<AppState>,
    Json(req): Json<ClaimRequest>,
) -> ApiResult<axum::response::Response> {
    use axum::response::IntoResponse;
    if let Some(job) = st.queue.claim(req.kind, std::time::Duration::from_secs(req.lease_secs)).await? {
        Ok(Json(ClaimResponse { id: job.id, payload: job.payload.0 }).into_response())
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
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