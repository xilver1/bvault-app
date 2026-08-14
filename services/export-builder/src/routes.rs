//! HTTP surface for the stateless reconcile transfer.
//!
//! - `POST /exports` builds the export into staging and returns `{export_id,
//!   manifest}`.
//! - `GET /exports/{id}/manifest` re-serves the manifest (the plan) — the client
//!   fetches it on resume to re-diff against the USB.
//! - `GET /exports/{id}/files/{*path}` streams one file: a rendered file from
//!   staging, or raw audio from the music store, resolved by the server so the
//!   client never sees internal locations.
//! - `DELETE /exports/{id}` drops the staging after a completed transfer.
//!
//! No transfer/session state lives here: the client enumerates the USB, diffs
//! against the manifest, and pulls what's missing. Two small on-disk files per
//! export make the server restart-tolerant: `manifest.json` (client-facing) and
//! `sources.json` (server-internal `usb_path -> raw_location` for audio).

use std::collections::HashMap;
use std::path::PathBuf;

use axum::body::Body;
use axum::extract::{Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio_util::io::ReaderStream;
use uuid::Uuid;

use bvault_export::{build_export, ExportInput, Manifest, PlaylistInput, Source};

use crate::error::{ApiResult, ExportError};
use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/exports", post(create_export))
        .route("/exports/{id}/manifest", get(get_manifest))
        .route("/exports/{id}/files/{*path}", get(serve_file))
        .route("/exports/{id}", delete(delete_export))
        .with_state(state)
}

#[derive(Deserialize)]
struct CreateExport {
    playlist_ids: Vec<Uuid>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    profile_name: Option<String>,
}

#[derive(Serialize)]
struct CreateExportResponse {
    export_id: Uuid,
    manifest: Manifest,
}

async fn create_export(
    State(st): State<AppState>,
    Json(req): Json<CreateExport>,
) -> ApiResult<Json<CreateExportResponse>> {
    if req.playlist_ids.is_empty() {
        return Err(ExportError::BadRequest("no playlists selected".into()));
    }

    // Union of member hashes (+ locations), filtered to what's actually analyzed
    // — export operates on analyzed tracks only; a stray unanalyzed one is skipped.
    let union = st.meta.resolve_hashes(None, &req.playlist_ids).await?;
    let tracks: Vec<(String, String)> =
        union.into_iter().filter(|(h, _)| st.artifacts.exists(h)).collect();
    if tracks.is_empty() {
        return Err(ExportError::BadRequest("no analyzed tracks to export".into()));
    }

    // Playlist name + membership for the PDB/device-library playlist entries.
    let mut playlists = Vec::new();
    for id in &req.playlist_ids {
        if let Some(pl) = st.meta.get_playlist(None, *id).await? {
            let hashes = st.meta.playlist_hashes(None, *id).await?;
            playlists.push(PlaylistInput { name: pl.name, hashes });
        }
    }

    let export_id = Uuid::new_v4();
    let dir = st.config.staging_root.join(export_id.to_string());
    let tree = dir.join("tree");
    let profile = req.profile_name.unwrap_or_else(|| "bvault".to_string());

    // build_export is CPU/FS-blocking (SQLCipher, file writes) → blocking thread.
    let build_state = st.clone();
    let (manifest, raw_sources) = tokio::task::spawn_blocking(
        move || -> Result<(Manifest, HashMap<String, String>), bvault_export::Error> {
            let input = ExportInput {
                tracks: &tracks,
                playlists: &playlists,
                profile_name: &profile,
            };
            let manifest = build_export(&input, &build_state.artifacts, &build_state.raw, &tree)?;

            // Server-side resolver: usb_path -> raw_location for audio entries, so
            // the client never learns internal store paths.
            let loc: HashMap<&str, &str> =
                tracks.iter().map(|(h, l)| (h.as_str(), l.as_str())).collect();
            let mut raw_sources = HashMap::new();
            for e in &manifest.entries {
                if let Source::Raw { hash } = &e.source {
                    if let Some(l) = loc.get(hash.as_str()) {
                        raw_sources.insert(e.usb_path.clone(), l.to_string());
                    }
                }
            }
            Ok((manifest, raw_sources))
        },
    )
    .await
    .map_err(|e| ExportError::Internal(format!("build task panicked: {e}")))??;

    // Persist the two records next to the tree.
    tokio::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec(&manifest).map_err(|e| ExportError::Internal(e.to_string()))?,
    )
    .await
    .map_err(|e| ExportError::Internal(e.to_string()))?;
    tokio::fs::write(
        dir.join("sources.json"),
        serde_json::to_vec(&raw_sources).map_err(|e| ExportError::Internal(e.to_string()))?,
    )
    .await
    .map_err(|e| ExportError::Internal(e.to_string()))?;

    Ok(Json(CreateExportResponse { export_id, manifest }))
}

async fn get_manifest(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<Response> {
    let path = st.config.staging_root.join(id.to_string()).join("manifest.json");
    let bytes = tokio::fs::read(&path).await.map_err(|_| ExportError::NotFound)?;
    Ok(([(header::CONTENT_TYPE, "application/json")], bytes).into_response())
}

async fn serve_file(
    State(st): State<AppState>,
    Path((id, usb_path)): Path<(Uuid, String)>,
) -> ApiResult<Response> {
    let dir = st.config.staging_root.join(id.to_string());
    if !dir.is_dir() {
        return Err(ExportError::NotFound);
    }

    // Audio? (present in the server-side resolver) → stream from the music store.
    let sources: HashMap<String, String> = {
        let bytes = tokio::fs::read(dir.join("sources.json"))
            .await
            .map_err(|_| ExportError::NotFound)?;
        serde_json::from_slice(&bytes).map_err(|e| ExportError::Internal(e.to_string()))?
    };
    if let Some(location) = sources.get(&usb_path) {
        let path = st.raw.resolve(location).map_err(|_| ExportError::NotFound)?;
        return stream_path(path).await;
    }

    // Otherwise a rendered file in staging. Guard against path traversal.
    guard_relative(&usb_path)?;
    let path = dir.join("tree").join(&usb_path);
    if !path.is_file() {
        return Err(ExportError::NotFound);
    }
    stream_path(path).await
}

async fn delete_export(
    State(st): State<AppState>,
    Path(id): Path<Uuid>,
) -> ApiResult<StatusCode> {
    let dir = st.config.staging_root.join(id.to_string());
    if dir.is_dir() {
        tokio::fs::remove_dir_all(&dir)
            .await
            .map_err(|e| ExportError::Internal(e.to_string()))?;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Full-file stream (no Range — transfer resumes at file granularity, not bytes).
async fn stream_path(path: PathBuf) -> ApiResult<Response> {
    let file = tokio::fs::File::open(&path)
        .await
        .map_err(|_| ExportError::NotFound)?;
    let size = file.metadata().await.map(|m| m.len()).unwrap_or(0);
    let body = Body::from_stream(ReaderStream::new(file));
    Ok((
        [
            (header::CONTENT_TYPE, "application/octet-stream".to_string()),
            (header::CONTENT_LENGTH, size.to_string()),
        ],
        body,
    )
        .into_response())
}

/// Reject anything that could escape the staging tree.
fn guard_relative(p: &str) -> ApiResult<()> {
    let bad = p.is_empty()
        || p.starts_with('/')
        || p.split('/').any(|c| c.is_empty() || c == ".." || c == ".");
    if bad {
        return Err(ExportError::BadRequest("invalid path".into()));
    }
    Ok(())
}