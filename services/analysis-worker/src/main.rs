//! bvault-export analysis worker: the queue consumer. It claims analysis jobs,
//! reads raw audio from the music store, runs the pure analysis engine, and
//! writes the result to the content-addressable artifact store. Stateless and
//! horizontally scalable — run as many replicas as the cluster allows.

mod config;
mod health;
mod worker;

use std::sync::Arc;

use anyhow::Context;
use tracing::info;

use bvault_analysis::AnalyzeOptions;
use bvault_jobs::Queue;
use bvault_store::{ArtifactStore, RawStore};

use crate::config::Config;
use crate::health::Health;
use crate::worker::Worker;

// Current-thread runtime: the async side only orchestrates (claim/complete/fail
// + sleep); the heavy analysis runs on the blocking pool. Minimal footprint.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    // Held for the life of the process so the batch exporter gets flushed on
    // the way out rather than losing the last jobs' spans.
    let _telemetry = bvault_telemetry::init("bvault-analysis-worker");

    let config = Config::from_env().context("loading config from environment")?;

    // Up before the first Postgres connect, so a slow database at boot shows
    // as not-ready rather than as a failed liveness probe and a restart.
    let health = Health::new(config.lease);
    health::spawn(&config.health_bind_addr, health.clone(), config.raw_store_root.clone())
        .await
        .context("starting health server")?;

    let queue = Queue::connect(&config.database_url, config.db_max_connections)
        .await
        .context("connecting to Postgres")?;

    let worker = Arc::new(Worker {
        queue,
        raw: RawStore::new(&config.raw_store_root),
        artifacts: ArtifactStore::new(&config.artifact_store_root),
        opts: AnalyzeOptions::default(),
    });

    info!(
        lease_s = config.lease.as_secs(),
        poll_s = config.poll_interval.as_secs(),
        "starting analysis worker"
    );
    worker.run(config.lease, config.poll_interval, health).await
}
