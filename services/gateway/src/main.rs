//! bvault-export gateway: the HTTP API that fronts the pipeline. It owns the
//! database (runs migrations), resolves playlists into analysis work, submits
//! jobs, and reports batch progress by polling. Thin client, fat server.

mod config;
mod error;
mod routes;
mod state;

use std::sync::Arc;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing::info;

use bvault_jobs::Queue;
use bvault_meta::Meta;
use bvault_store::{ArtifactStore, RawStore};

use crate::config::Config;
use crate::state::AppState;

// Current-thread runtime: this gateway serves one household's phone, not a fleet.
// It keeps the pod's memory footprint minimal, in keeping with the whole design.
#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env().context("loading config from environment")?;

    let pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .connect(&config.database_url)
        .await
        .context("connecting to Postgres")?;

    // The gateway owns schema: one migrator over all tables (jobs, batches, meta).
    sqlx::migrate!("./migrations")
        .run(&pool)
        .await
        .context("running migrations")?;

    let state = AppState {
        meta: Meta::new(pool.clone()),
        queue: Queue::from_pool(pool.clone()),
        artifacts: ArtifactStore::new(&config.artifact_store_root),
        raw: RawStore::new(&config.raw_store_root),
        config: Arc::new(config),
    };

    let listener = TcpListener::bind(&state.config.bind_addr)
        .await
        .with_context(|| format!("binding {}", state.config.bind_addr))?;
    info!(addr = %state.config.bind_addr, "gateway listening");

    axum::serve(listener, routes::build_router(state)).await?;
    Ok(())
}