//! 12-factor config: everything comes from the environment (injected from
//! ConfigMaps/Secrets/SSM at runtime). No config file, no hardcoded paths.

use std::path::PathBuf;

use anyhow::{anyhow, Result};

#[derive(Debug, Clone)]
pub struct Config {
    /// Postgres DSN, e.g. `postgres://user:pass@host/db`.
    pub database_url: String,
    /// Address to bind, e.g. `0.0.0.0:8080`.
    pub bind_addr: String,
    /// Root of the content-addressable analysis (artifact) store.
    pub artifact_store_root: PathBuf,
    pub db_max_connections: u32,
    /// Attempts before an analysis job is parked as `dead`.
    pub analysis_max_attempts: i32,
}

impl Config {
    pub fn from_env() -> Result<Self> {
        Ok(Self {
            database_url: required("DATABASE_URL")?,
            bind_addr: optional("BIND_ADDR", "0.0.0.0:8080"),
            artifact_store_root: required("ARTIFACT_STORE_ROOT")?.into(),
            db_max_connections: optional("DB_MAX_CONNECTIONS", "10").parse()?,
            analysis_max_attempts: optional("ANALYSIS_MAX_ATTEMPTS", "5").parse()?,
        })
    }
}

fn required(key: &str) -> Result<String> {
    std::env::var(key).map_err(|_| anyhow!("missing required env var {key}"))
}

fn optional(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}