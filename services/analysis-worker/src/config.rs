//! 12-factor config from the environment. No config file, no hardcoded paths.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres DSN for the job queue.
    pub database_url: String,
    /// Root of the mounted music store (raw audio).
    pub raw_store_root: PathBuf,
    /// Root of the content-addressable analysis store (artifacts out).
    pub artifact_store_root: PathBuf,
    pub db_max_connections: u32,
    /// Lease per claimed job. Set comfortably above worst-case single-track
    /// analysis, so a job never outlives its lease and no heartbeat is needed.
    pub lease: Duration,
    /// Wait between polls when the queue is empty.
    pub poll_interval: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            raw_store_root: required("RAW_STORE_ROOT")?.into(),
            artifact_store_root: required("ARTIFACT_STORE_ROOT")?.into(),
            db_max_connections: optional("DB_MAX_CONNECTIONS", "4").parse()?,
            lease: Duration::from_secs(optional("LEASE_SECONDS", "300").parse()?),
            poll_interval: Duration::from_secs(optional("POLL_INTERVAL_SECONDS", "5").parse()?),
        })
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("missing required env var {key}"))
}

fn optional(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}