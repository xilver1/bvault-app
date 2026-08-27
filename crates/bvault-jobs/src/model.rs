//! The durable job/queue contract shared by the gateway (producer) and the
//! analysis worker (consumer). These types are the wire format between the two
//! services; changing them is a schema change.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;

/// What kind of work a job represents. Postgres enum `job_kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "job_kind", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum JobKind {
    /// Analyze one track by content hash.
    Analysis,
    /// Fetch+transcode one URL via the Python yt-dlp service. The row exists
    /// only so the CLI can poll terminal state; the work runs out-of-band.
    YtDlpIngest,
}

/// Payload for a [`JobKind::YtDlpIngest`] job. `user_id` is carried here rather
/// than as a column so ownership checks work without a jobs-table schema change
/// while user-scoping lands in Step 1b.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct YtDlpIngestJob {
    pub url: String,
    pub user_id: String,
}

/// Lifecycle state. Postgres enum `job_status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, sqlx::Type, Serialize, Deserialize)]
#[sqlx(type_name = "job_status", rename_all = "snake_case")]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    /// Waiting to be claimed.
    Pending,
    /// Leased by a worker; `locked_until` bounds the lease.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Failed but still retryable (returned to the pool).
    Failed,
    /// Failed past `max_attempts` — parked for inspection, never retried.
    Dead,
}

/// A queue row as read back from Postgres.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Job {
    pub id: i64,
    pub kind: JobKind,
    /// Idempotency key — the content hash (hex) for analysis.
    pub dedup_key: String,
    pub payload: Json<serde_json::Value>,
    pub status: JobStatus,
    pub attempts: i32,
    pub max_attempts: i32,
    pub locked_until: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl Job {
    /// Deserialize the JSONB payload into a typed contract struct
    /// (e.g. [`AnalysisJob`]).
    pub fn payload_as<T>(&self) -> Result<T, serde_json::Error>
    where
        T: for<'de> Deserialize<'de>,
    {
        serde_json::from_value(self.payload.0.clone())
    }
}

/// Payload for an [`JobKind::Analysis`] job.
///
/// The gateway resolves the hash to its raw-file location (from the raw-file
/// lookup table) and puts **both** here, so the worker never touches the
/// metadata DB: it reads the path it's handed, analyzes, and writes artifacts to
/// the content-addressable store keyed by the hash.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalysisJob {
    /// Content hash (hex) — identity, dedup key, and artifact-store key.
    pub hash: String,
    /// Where the raw audio lives in the mounted music store.
    pub raw_location: String,
    /// Ingestion-known title, used only if the file itself carries no tag.
    #[serde(default)]
    pub fallback_title: Option<String>,
}
