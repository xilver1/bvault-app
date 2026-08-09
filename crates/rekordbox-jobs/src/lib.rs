//! rekordbox-jobs: the Postgres-backed job queue and the job/result contract
//! shared between the gateway and the analysis worker.
//!
//! Design in one breath: the gateway [`Queue::enqueue`]s one job per content
//! hash; workers [`Queue::claim`] them under a lease, [`Queue::heartbeat`] long
//! ones, and [`Queue::complete`] / [`Queue::fail`] them. Idempotent enqueue and
//! automatic lease-expiry reclaim mean duplicate submissions and dead workers
//! are handled by the queue itself, not by callers.
//!
//! Whether a track is *analyzed* is deliberately **not** tracked here — that is
//! derived from the content-addressable analysis store (presence == analyzed).
//! The queue only knows about work, not results, and nothing about batches.

mod model;
mod queue;

pub use model::{AnalysisJob, Job, JobKind, JobStatus};
pub use queue::Queue;

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("database: {0}")]
    Db(#[from] sqlx::Error),

    #[error("payload (de)serialization: {0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    // Pure contract round-trip — no database needed. DB-backed queue behaviour
    // is covered by integration tests gated on a DATABASE_URL (see README).
    #[test]
    fn analysis_payload_roundtrips() {
        let job = AnalysisJob {
            hash: "a1b2c3".into(),
            raw_location: "/music/artist/track.flac".into(),
            fallback_title: Some("Track".into()),
        };
        let value = serde_json::to_value(&job).unwrap();
        let back: AnalysisJob = serde_json::from_value(value).unwrap();
        assert_eq!(back.hash, job.hash);
        assert_eq!(back.raw_location, job.raw_location);
        assert_eq!(back.fallback_title, job.fallback_title);
    }

    #[test]
    fn status_serializes_snake_case() {
        // The Serde form is what any HTTP/JSON surface will expose.
        let s = serde_json::to_string(&JobStatus::Succeeded).unwrap();
        assert_eq!(s, "\"succeeded\"");
    }
}