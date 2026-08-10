//! Server configuration

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct Config {
    /// Root music directory (pre-export folder)
    pub music_dir: PathBuf,
    /// Cache directory for analysis results
    pub cache_dir: PathBuf,
    /// Output directory for USB export
    pub output_dir: Option<PathBuf>,
    /// TCP bind address (host:port)
    pub bind_addr: String,
    /// Max concurrent analysis tasks
    pub max_concurrent: usize,
    /// Navidrome configuration (optional)
    pub navidrome: Option<NavidromeConfig>,
}

/// Navidrome/Subsonic API configuration
#[derive(Debug, Clone)]
pub struct NavidromeConfig {
    /// Server URL (e.g., http://192.168.1.100:4533)
    pub url: String,
    /// Username
    pub user: String,
    /// Password
    pub pass: String,
}

impl NavidromeConfig {
    pub fn new(url: String, user: String, pass: String) -> Self {
        Self { url, user, pass }
    }
}
