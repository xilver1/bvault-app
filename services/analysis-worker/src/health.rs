//! Liveness and readiness for the kubelet.
//!
//! The worker has no traffic, so "healthy" is about the claim loop, not about
//! answering requests:
//!
//! - **Live** (`/livez`) — the loop is making progress. Either it is idle and
//!   came round recently, or it is busy on a job that is still inside its
//!   lease. A job past its lease has already been handed to someone else by
//!   the queue, so this worker is doing wasted work and a restart is the right
//!   fix. Liveness deliberately ignores the database: if Postgres is down the
//!   loop backs off and retries on its own, and restarting every worker would
//!   fix nothing.
//! - **Ready** (`/readyz`) — the worker can actually do work right now: the
//!   last claim reached the database, and the music store mount answers.
//!   Not-ready never restarts anything; it shows up in rollouts, PDBs and
//!   alerts.
//!
//! Probe handlers only read atomics. The one check that touches the
//! filesystem runs in a background task, because a stale NFS mount blocks
//! `stat` forever and would otherwise hang the probe itself.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use axum::{Json, Router};
use serde_json::{json, Value};
use tokio::net::TcpListener;
use tracing::{info, warn};

/// How long the idle loop may go without coming round before it counts as
/// stuck. Generous on purpose: a claim against an unreachable database can
/// block for sqlx's 30 s acquire timeout before it errors and the loop backs
/// off, and that case must *not* trip liveness.
const IDLE_STALL_AFTER: Duration = Duration::from_secs(120);

/// How often the music-store mount is checked, and how long one check may take.
const STORE_CHECK_EVERY: Duration = Duration::from_secs(10);
const STORE_CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// Shared health state. Cheap to clone; every field is an `Arc`'d atomic.
#[derive(Clone)]
pub struct Health {
    inner: Arc<Inner>,
}

struct Inner {
    /// Unix seconds of the last completed loop iteration (claim returned).
    last_tick: AtomicU64,
    /// Unix seconds the current job started, or 0 when idle.
    job_started: AtomicU64,
    /// Whether the most recent claim reached the database.
    last_claim_ok: AtomicBool,
    /// Whether the last music-store check succeeded in time.
    store_ok: AtomicBool,
    /// The lease each claim takes; a job running longer is lost anyway.
    lease: Duration,
}

impl Health {
    pub fn new(lease: Duration) -> Self {
        Self {
            inner: Arc::new(Inner {
                // Start "just ticked" so the pod isn't declared dead while it
                // is still connecting to Postgres for the first time.
                last_tick: AtomicU64::new(now()),
                job_started: AtomicU64::new(0),
                last_claim_ok: AtomicBool::new(false),
                store_ok: AtomicBool::new(false),
                lease,
            }),
        }
    }

    /// Record one finished claim attempt (job, empty queue, or error).
    pub fn tick(&self, claim_ok: bool) {
        self.inner.last_tick.store(now(), Ordering::Relaxed);
        self.inner.last_claim_ok.store(claim_ok, Ordering::Relaxed);
    }

    pub fn job_started(&self) {
        self.inner.job_started.store(now(), Ordering::Relaxed);
    }

    pub fn job_finished(&self) {
        self.inner.job_started.store(0, Ordering::Relaxed);
        // Finishing a job is progress too; don't make the idle check wait for
        // the next claim to come back.
        self.inner.last_tick.store(now(), Ordering::Relaxed);
    }

    fn liveness(&self) -> (bool, Value) {
        let now = now();
        let job = self.inner.job_started.load(Ordering::Relaxed);
        if job != 0 {
            let age = now.saturating_sub(job);
            let limit = self.inner.lease.as_secs();
            (age < limit, json!({ "state": "busy", "job_age_s": age, "limit_s": limit }))
        } else {
            let age = now.saturating_sub(self.inner.last_tick.load(Ordering::Relaxed));
            let limit = IDLE_STALL_AFTER.as_secs();
            (age < limit, json!({ "state": "idle", "since_last_loop_s": age, "limit_s": limit }))
        }
    }

    fn readiness(&self) -> (bool, Value) {
        let db = self.inner.last_claim_ok.load(Ordering::Relaxed);
        let store = self.inner.store_ok.load(Ordering::Relaxed);
        (db && store, json!({ "database": db, "music_store": store }))
    }
}

/// Start the probe server and the background store checker. Returns once the
/// listener is bound, so a bad address fails startup instead of every probe.
pub async fn spawn(addr: &str, health: Health, store_root: PathBuf) -> anyhow::Result<()> {
    tokio::spawn(check_store(health.clone(), store_root));

    let app = Router::new()
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
        .with_state(health);

    let listener = TcpListener::bind(addr).await?;
    info!(%addr, "health server listening");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, app).await {
            warn!(error = %e, "health server stopped");
        }
    });
    Ok(())
}

async fn livez(State(h): State<Health>) -> (StatusCode, Json<Value>) {
    respond(h.liveness())
}

async fn readyz(State(h): State<Health>) -> (StatusCode, Json<Value>) {
    respond(h.readiness())
}

fn respond((ok, detail): (bool, Value)) -> (StatusCode, Json<Value>) {
    let code = if ok { StatusCode::OK } else { StatusCode::SERVICE_UNAVAILABLE };
    (code, Json(detail))
}

/// Periodically confirm the music store answers. Runs off the probe path, and
/// never starts a new check while a previous one is still stuck, so a dead NFS
/// mount costs one blocked thread rather than one per probe.
async fn check_store(health: Health, root: PathBuf) {
    let mut tick = tokio::time::interval(STORE_CHECK_EVERY);
    let mut in_flight: Option<tokio::task::JoinHandle<bool>> = None;
    loop {
        tick.tick().await;

        let handle = match in_flight.take() {
            // The last check is still blocked: the mount is not answering.
            Some(h) if !h.is_finished() => {
                health.inner.store_ok.store(false, Ordering::Relaxed);
                in_flight = Some(h);
                continue;
            }
            _ => {
                let root = root.clone();
                tokio::task::spawn_blocking(move || std::fs::read_dir(&root).is_ok())
            }
        };

        let mut handle = handle;
        let ok = match tokio::time::timeout(STORE_CHECK_TIMEOUT, &mut handle).await {
            Ok(Ok(ok)) => ok,
            Ok(Err(_)) => false, // the check panicked
            Err(_) => {
                // Timed out: keep the handle so we don't pile up more checks.
                in_flight = Some(handle);
                false
            }
        };
        if !ok {
            warn!(root = %root.display(), "music store check failed");
        }
        health.inner.store_ok.store(ok, Ordering::Relaxed);
    }
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
