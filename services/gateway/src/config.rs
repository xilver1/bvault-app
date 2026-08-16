//! 12-factor config: everything comes from the environment (injected from
//! ConfigMaps/Secrets/SSM at runtime). No config file, no hardcoded paths.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres DSN, e.g. `postgres://user:pass@host/db`.
    pub database_url: String,
    /// Address to bind, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// Root of the raw audio store.
    pub raw_store_root: PathBuf,
    /// Root of the content-addressable analysis (artifact) store.
    pub artifact_store_root: PathBuf,
    pub db_max_connections: u32,
    /// Attempts before an analysis job is parked as `dead`.
    pub analysis_max_attempts: i32,
    /// How long a freshly issued session token stays valid.
    pub session_ttl: Duration,
    /// URL of the yt-dlp ingestion microservice (if enabled).
    pub yt_dlp_service_url: Option<String>,
    /// API Key used to secure inter-service comms (e.g. for yt-dlp-ingest -> gateway)
    pub internal_api_key: Option<String>,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: optional("BIND_ADDR", "0.0.0.0:8080"),
            raw_store_root: optional("RAW_STORE_ROOT", "/mnt/music-store").into(),
            artifact_store_root: required("ARTIFACT_STORE_ROOT")?.into(),
            db_max_connections: optional("DB_MAX_CONNECTIONS", "10").parse()?,
            analysis_max_attempts: optional("ANALYSIS_MAX_ATTEMPTS", "5").parse()?,
            // Default 30 days — long-lived enough for a phone client, still bounded.
            session_ttl: Duration::from_secs(optional("SESSION_TTL_SECONDS", "2592000").parse()?),
            yt_dlp_service_url: std::env::var("YT_DLP_SERVICE_URL").ok(),
            internal_api_key: std::env::var("INTERNAL_API_KEY").ok(),
        })
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("missing required env var {key}"))
}

fn optional(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}