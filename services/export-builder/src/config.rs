//! 12-factor config from the environment. No config file, no hardcoded paths.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres DSN (read-only use here: resolving playlists via meta).
    pub database_url: String,
    pub bind_addr: String,
    /// Mounted music store (raw audio) — streamed to the USB, never copied.
    pub raw_store_root: PathBuf,
    /// Content-addressable analysis store (analysis.json in).
    pub artifact_store_root: PathBuf,
    /// Where builds are staged, one subdir per export id. Throwaway.
    pub staging_root: PathBuf,
    pub db_max_connections: u32,
    /// Abandoned exports older than this are swept.
    pub export_ttl: Duration,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: optional("BIND_ADDR", "0.0.0.0:8080"),
            raw_store_root: required("RAW_STORE_ROOT")?.into(),
            artifact_store_root: required("ARTIFACT_STORE_ROOT")?.into(),
            staging_root: required("STAGING_ROOT")?.into(),
            db_max_connections: optional("DB_MAX_CONNECTIONS", "5").parse()?,
            export_ttl: Duration::from_secs(optional("EXPORT_TTL_SECONDS", "86400").parse()?),
        })
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("missing required env var {key}"))
}

fn optional(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}
