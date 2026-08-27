//! bvault-export export-builder: renders a USB export into staging and serves
//! the stateless reconcile transfer. Builds are throwaway; the client (phone)
//! drives the diff-and-pull loop.

mod config;
mod error;
mod routes;
mod state;

use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use sqlx::postgres::PgPoolOptions;
use tokio::net::TcpListener;
use tracing::{info, warn};

use bvault_meta::Meta;
use bvault_store::{ArtifactStore, RawStore};

use crate::config::Config;
use crate::state::AppState;

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::from_env().context("loading config from environment")?;
    tokio::fs::create_dir_all(&config.staging_root)
        .await
        .context("creating staging root")?;

    // Read-only pool for resolving playlists; migrations are the gateway's job.
    let pool = PgPoolOptions::new()
        .max_connections(config.db_max_connections)
        .connect(&config.database_url)
        .await
        .context("connecting to Postgres")?;

    let state = AppState {
        meta: Meta::new(pool),
        artifacts: ArtifactStore::new(&config.artifact_store_root),
        raw: RawStore::new(&config.raw_store_root),
        config: Arc::new(config),
    };

    // Sweep abandoned exports so staging doesn't grow without bound.
    tokio::spawn(run_sweeper(state.clone()));

    let listener = TcpListener::bind(&state.config.bind_addr)
        .await
        .with_context(|| format!("binding {}", state.config.bind_addr))?;
    info!(addr = %state.config.bind_addr, "export-builder listening");

    axum::serve(listener, routes::build_router(state)).await?;
    Ok(())
}

/// Hourly sweep of export dirs older than the configured TTL.
async fn run_sweeper(state: AppState) {
    let mut tick = tokio::time::interval(Duration::from_secs(3600));
    loop {
        tick.tick().await;
        if let Err(e) = sweep_once(&state).await {
            warn!(error = %e, "export sweep failed");
        }
    }
}

async fn sweep_once(state: &AppState) -> std::io::Result<()> {
    let now = std::time::SystemTime::now();
    let mut entries = tokio::fs::read_dir(&state.config.staging_root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let meta = entry.metadata().await?;
        if !meta.is_dir() {
            continue;
        }
        let stale = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .map(|age| age > state.config.export_ttl)
            .unwrap_or(false);
        if stale {
            let path = entry.path();
            match tokio::fs::remove_dir_all(&path).await {
                Ok(()) => info!(path = %path.display(), "swept stale export"),
                Err(e) => warn!(path = %path.display(), error = %e, "failed to sweep export"),
            }
        }
    }
    Ok(())
}
