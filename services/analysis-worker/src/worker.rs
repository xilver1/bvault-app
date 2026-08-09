//! The claim → analyze → store loop.
//!
//! One job at a time; scale out by running more replicas (the k8s-native model),
//! not by threading inside a single worker. Analysis is CPU-bound and blocking,
//! so it runs on a blocking thread while only the queue calls stay async.
//!
//! Correctness leans entirely on the substrate: claims auto-reclaim dead leases,
//! enqueue is idempotent, and artifact writes are content-addressed and
//! idempotent — so a reclaimed or duplicated job simply redoes deterministic
//! work and writes the same bytes. Nothing here needs its own locking.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use tracing::{error, info};

use rekordbox_analysis::AnalyzeOptions;
use rekordbox_jobs::{AnalysisJob, Job, JobKind, Queue};
use rekordbox_store::{ArtifactStore, RawStore};

/// Shared, read-only handles for every job.
pub struct Worker {
    pub queue: Queue,
    pub raw: RawStore,
    pub artifacts: ArtifactStore,
    pub opts: AnalyzeOptions,
}

impl Worker {
    /// Run forever: claim, process, repeat; sleep when idle. Only returns on an
    /// unrecoverable error (which restarts the pod).
    pub async fn run(self: Arc<Self>, lease: Duration, poll: Duration) -> Result<()> {
        info!("analysis worker ready");
        loop {
            match self.queue.claim(JobKind::Analysis, lease).await {
                Ok(Some(job)) => self.clone().process(job).await,
                Ok(None) => tokio::time::sleep(poll).await,
                Err(e) => {
                    // Transient DB hiccup — back off and retry, don't crash.
                    error!(error = %e, "claim failed; backing off");
                    tokio::time::sleep(poll).await;
                }
            }
        }
    }

    /// Drive one job to a terminal state.
    async fn process(self: Arc<Self>, job: Job) {
        let job_id = job.id;
        let payload: AnalysisJob = match job.payload_as() {
            Ok(p) => p,
            Err(e) => {
                // A malformed payload can never succeed; fail it (it dead-letters
                // after max_attempts) rather than spin.
                let _ = self.queue.fail(job_id, &format!("bad payload: {e}")).await;
                return;
            }
        };

        let hash = payload.hash.clone();
        let worker = self.clone();
        // CPU-bound + blocking I/O → off the async runtime.
        let outcome = tokio::task::spawn_blocking(move || worker.analyze(&payload)).await;

        match outcome {
            Ok(Ok(())) => {
                info!(job = job_id, %hash, "analyzed");
                let _ = self.queue.complete(job_id).await;
            }
            Ok(Err(e)) => {
                error!(job = job_id, %hash, error = %e, "analysis failed");
                let _ = self.queue.fail(job_id, &e.to_string()).await;
            }
            Err(join) => {
                error!(job = job_id, %hash, error = %join, "analysis task panicked");
                let _ = self.queue.fail(job_id, &format!("panicked: {join}")).await;
            }
        }
    }

    /// The actual work — fully synchronous, runs on a blocking thread.
    fn analyze(&self, payload: &AnalysisJob) -> Result<()> {
        // A reclaimed or duplicate job whose output already exists is a no-op.
        if self.artifacts.exists(&payload.hash) {
            return Ok(());
        }

        // The hex hash is identity; parse it back to the u64 the analyzer records
        // (the same value ingestion computed with core's hasher).
        let file_hash = u64::from_str_radix(&payload.hash, 16)
            .with_context(|| format!("hash is not hex: {}", payload.hash))?;

        let file = self.raw.open(&payload.raw_location)?;
        let file_size = file.metadata()?.len();
        let hint_ext = Path::new(&payload.raw_location)
            .extension()
            .and_then(|e| e.to_str());

        let analysis = rekordbox_analysis::analyze_source(
            Box::new(file),
            hint_ext,
            file_size,
            file_hash,
            payload.fallback_title.as_deref(),
            &self.opts,
        )?;

        // Persist the small, regenerate the large: the durable artifact is the
        // analysis result itself. ANLZ, the .pdb, and Contents are rendered from
        // it at export time — they depend on the export layout (e.g. the
        // /Contents path baked into ANLZ's PPTH tag), so they're not produced
        // here.
        let json = serde_json::to_vec(&analysis).context("serializing analysis")?;
        self.artifacts
            .put(&payload.hash, &[("analysis.json", &json)])?;
        Ok(())
    }
}