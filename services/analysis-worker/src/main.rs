//! bvault-export analysis worker: the queue consumer. It claims analysis jobs,
//! reads raw audio from the music store, runs the pure analysis engine, and
//! writes the result to the content-addressable artifact store. Stateless and
//! horizontally scalable — run as many replicas as the cluster allows.

mod config;
mod worker;

use std::sync::Arc;

use anyhow::Context;
use tracing::info;

use bvault_analysis::AnalyzeOptions;
use bvault_jobs::Queue;
use bvault_store::{ArtifactStore, RawStore};

use crate::config::Config;
use crate::worker::Worker;

// Current-thread runtime: the async side only orchestrates (claim/complete/fail
// + sleep); the heavy analysis runs on the blocking pool. Minimal footprint.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env().context("loading config from environment")?;

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
    worker.run(config.lease, config.poll_interval).await
}
