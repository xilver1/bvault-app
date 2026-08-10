//! HTTP surface. Polling model: `POST /analyze` returns a batch id, and the
//! client polls `GET /batches/{id}` for progress that is *derived* on read.

use axum::extract::{Path, Query, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use bvault_jobs::{AnalysisJob, JobKind};
use bvault_meta::{Playlist, Track};

use crate::error::{ApiResult, GatewayError};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(health))
        .route("/tracks", get(list_tracks).post(register_track))
        .route("/playlists", get(list_playlists).post(create_playlist))
        .route("/playlists/{id}", get(get_playlist))
        .route("/playlists/{id}/hashes", get(playlist_hashes))
        .route("/analyze", post(analyze))
        .route("/batches/{id}", get(batch_progress))
        .with_state(state)
}

async fn health() -> &'static str {
    "ok"
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
    State(st): State<AppState>,
    Query(p): Query<Pagination>,
) -> ApiResult<Json<Vec<Track>>> {
    Ok(Json(st.meta.list_tracks(p.limit, p.offset).await?))
}

#[derive(Deserialize)]
struct RegisterTrack {
    hash: String,
    raw_location: String,
    title: Option<String>,
    artist: Option<String>,
}

/// Ingestion registers a track here after storing its raw audio. Upsert, so
/// re-ingesting the same hash is safe.
async fn register_track(
    State(st): State<AppState>,
    Json(t): Json<RegisterTrack>,
) -> ApiResult<Json<serde_json::Value>> {
    st.meta
        .upsert_track(&t.hash, &t.raw_location, t.title.as_deref(), t.artist.as_deref())
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
    State(st): State<AppState>,
    Json(req): Json<CreatePlaylist>,
) -> ApiResult<Json<CreatedId>> {
    if req.name.trim().is_empty() {
        return Err(GatewayError::BadRequest("playlist name is required".into()));
    }
    let id = st
        .meta
        .create_playlist(&req.name, req.description.as_deref(), &req.hashes)
        .await?;
    Ok(Json(CreatedId { id }))
}

async fn list_playlists(State(st): State<AppState>) -> ApiResult<Json<Vec<Playlist>>> {
    Ok(Json(st.meta.list_playlists().await?))
}

async fn get_playlist(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Playlist>> {
    st.meta
        .get_playlist(id)
        .await?
        .map(Json)
        .ok_or(GatewayError::NotFound)
}

async fn playlist_hashes(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<Vec<String>>> {
    Ok(Json(st.meta.playlist_hashes(id).await?))
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
    /// Total tracks in the submission (the progress denominator).
    total: usize,
    /// How many were actually enqueued — the rest were already analyzed.
    enqueued: usize,
}

/// Submit an analysis: resolve the union of the selected playlists' hashes,
/// snapshot them as a batch, and enqueue only the ones the store doesn't already
/// have. Returns immediately with the batch id to poll.
async fn analyze(
    State(st): State<AppState>,
    Json(req): Json<AnalyzeRequest>,
) -> ApiResult<Json<AnalyzeResponse>> {
    if req.playlist_ids.is_empty() {
        return Err(GatewayError::BadRequest("no playlists selected".into()));
    }

    // Union of member hashes (+ raw locations), deduplicated across playlists.
    let tracks = st.meta.resolve_hashes(&req.playlist_ids).await?;
    let all_hashes: Vec<String> = tracks.iter().map(|(h, _)| h.clone()).collect();

    // Snapshot the FULL set so progress counts already-analyzed tracks honestly.
    let batch_id = st.meta.create_batch(req.name.as_deref(), &all_hashes).await?;

    // Enqueue only what the store lacks. Enqueue is idempotent, so a racing
    // duplicate submission is harmless.
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

/// Progress, derived on read — never stored. `done` from artifact-store presence
/// (the "analyzed" truth), `failed` from the jobs dead-letter, so a permanently
/// failed track completes the batch instead of hanging the bar.
async fn batch_progress(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Json<BatchProgress>> {
    let batch = st.meta.get_batch(id).await?.ok_or(GatewayError::NotFound)?;

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