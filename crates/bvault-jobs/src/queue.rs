//! Postgres-backed work queue.
//!
//! Claiming uses `FOR UPDATE SKIP LOCKED` so any number of workers can pull work
//! concurrently without blocking one another. Each claim takes a lease
//! (`locked_until`); **dead-worker recovery is automatic** because a claim also
//! considers `running` jobs whose lease has expired — a pod killed mid-job just
//! loses its lease and the job is re-served. Combined with content-hash-keyed,
//! idempotent artifact writes, redoing a job is always safe.
//!
//! The queue knows nothing about batches: a batch is a derived, user-facing view
//! assembled by the gateway from the artifact store (done) and the dead-letter
//! (failed). The only concession here is [`Queue::count_dead`], a generic
//! "how many of these hashes are dead" probe over a plain hash list.

use std::time::Duration;

use serde::Serialize;
use sqlx::postgres::{PgPool, PgPoolOptions};

use crate::model::{Job, JobKind, JobStatus};
use crate::Result;

/// Handle to the queue. Cheap to clone — wraps a connection pool.
#[derive(Clone)]
pub struct Queue {
    pool: PgPool,
}

impl Queue {
    /// Fetch a single job by id (for status polling). `None` if unknown.
    pub async fn get(&self, job_id: i64) -> Result<Option<Job>> {
        let job = sqlx::query_as::<_, Job>(
            r#"
            select id, kind, dedup_key, payload, status, attempts,
                   max_attempts, locked_until, last_error, created_at
            from jobs where id = $1
            "#,
        )
        .bind(job_id)
        .fetch_optional(&self.pool)
        .await?;
        Ok(job)
    }

    /// Terminally park a job as `dead` — for work executed outside the worker
    /// pool (the yt-dlp service), where `fail`'s retry path doesn't apply
    /// because nothing will re-claim it.
    pub async fn mark_dead(&self, job_id: i64, error: &str) -> Result<()> {
        sqlx::query(
            r#"
            update jobs
            set status = 'dead', last_error = $2, locked_until = null,
                finished_at = now(), updated_at = now()
            where id = $1
            "#,
        )
        .bind(job_id)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Connect to Postgres (DSN injected from config — never hardcoded).
    /// Migrations are the gateway's responsibility, not the queue's.
    pub async fn connect(url: &str, max_conns: u32) -> Result<Self> {
        let pool = PgPoolOptions::new()
            .max_connections(max_conns)
            .connect(url)
            .await?;
        Ok(Self { pool })
    }

    /// Build from an existing pool — for tests, or to share the gateway's pool.
    pub fn from_pool(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Idempotent enqueue. If an active (pending/running) job already exists for
    /// this `(kind, dedup_key)`, its id is returned and nothing new is created,
    /// so re-submitting the same hash is a no-op rather than a duplicate.
    ///
    /// Returns the job id (new or existing).
    pub async fn enqueue<T: Serialize>(
        &self,
        kind: JobKind,
        dedup_key: &str,
        payload: &T,
        max_attempts: i32,
    ) -> Result<i64> {
        let payload = serde_json::to_value(payload)?;
        // The no-op UPDATE on conflict lets RETURNING give us the existing id
        // whether we inserted a new row or collided with an active one.
        let (id,): (i64,) = sqlx::query_as(
            r#"
            insert into jobs (kind, dedup_key, payload, max_attempts)
            values ($1, $2, $3, $4)
            on conflict (kind, dedup_key) where status in ('pending', 'running')
            do update set updated_at = now()
            returning id
            "#,
        )
        .bind(kind)
        .bind(dedup_key)
        .bind(payload)
        .bind(max_attempts)
        .fetch_one(&self.pool)
        .await?;
        Ok(id)
    }

    /// Atomically claim the oldest runnable job of `kind`, leasing it for
    /// `lease`. Returns `None` when the queue is empty. Safe under concurrency:
    /// `SKIP LOCKED` means competing workers never block on the same row.
    pub async fn claim(&self, kind: JobKind, lease: Duration) -> Result<Option<Job>> {
        let job = sqlx::query_as::<_, Job>(
            r#"
            update jobs
            set status = 'running',
                attempts = attempts + 1,
                locked_at = now(),
                locked_until = now() + make_interval(secs => $2),
                updated_at = now()
            where id = (
                select id from jobs
                where kind = $1
                  and (status = 'pending'
                       or (status = 'running' and locked_until < now()))
                order by created_at
                for update skip locked
                limit 1
            )
            returning id, kind, dedup_key, payload, status, attempts,
                      max_attempts, locked_until, last_error, created_at
            "#,
        )
        .bind(kind)
        .bind(lease.as_secs() as i64)
        .fetch_optional(&self.pool)
        .await?;
        Ok(job)
    }

    /// Extend a running job's lease. Call periodically during long analyses so
    /// the job isn't reclaimed as dead while it's still being worked.
    pub async fn heartbeat(&self, job_id: i64, lease: Duration) -> Result<()> {
        sqlx::query(
            r#"
            update jobs
            set locked_until = now() + make_interval(secs => $2), updated_at = now()
            where id = $1 and status = 'running'
            "#,
        )
        .bind(job_id)
        .bind(lease.as_secs() as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Mark a job succeeded. Idempotent artifact writes make re-completing a
    /// reclaimed job harmless.
    pub async fn complete(&self, job_id: i64) -> Result<()> {
        sqlx::query(
            r#"
            update jobs
            set status = 'succeeded', locked_until = null,
                finished_at = now(), updated_at = now()
            where id = $1
            "#,
        )
        .bind(job_id)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Record a failure. Retryable (back to `pending`) until `attempts` reaches
    /// `max_attempts`, then parked as `dead` for inspection rather than looping.
    /// `attempts` was already incremented at claim time.
    pub async fn fail(&self, job_id: i64, error: &str) -> Result<()> {
        sqlx::query(
            r#"
            update jobs
            set status = case when attempts >= max_attempts then 'dead' else 'pending' end,
                last_error = $2,
                locked_until = null,
                finished_at = case when attempts >= max_attempts then now() else null end,
                updated_at = now()
            where id = $1
            "#,
        )
        .bind(job_id)
        .bind(error)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    /// Count jobs by status for a kind — a coarse queue-health probe. (Batch
    /// progress is computed by the gateway from the store, not from here.)
    pub async fn counts(&self, kind: JobKind) -> Result<Vec<(JobStatus, i64)>> {
        let rows = sqlx::query_as::<_, (JobStatus, i64)>(
            "select status, count(*) from jobs where kind = $1 group by status",
        )
        .bind(kind)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows)
    }

    /// Of the given dedup keys (hashes), how many are parked as `dead`. Lets the
    /// gateway count permanently-failed tracks toward a batch's completion so a
    /// dead track doesn't hang the progress bar forever.
    pub async fn count_dead(&self, kind: JobKind, dedup_keys: &[String]) -> Result<i64> {
        let (n,): (i64,) = sqlx::query_as(
            r#"
            select count(*) from jobs
            where kind = $1 and status = 'dead' and dedup_key = any($2)
            "#,
        )
        .bind(kind)
        .bind(dedup_keys)
        .fetch_one(&self.pool)
        .await?;
        Ok(n)
    }
}