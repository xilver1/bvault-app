//! bvault-server: Audio analysis and Pioneer export generation
//!
//! This server runs on the NAS (Dell Wyse 5070) and handles:
//! - Audio file analysis (BPM, waveforms, beat grids)
//! - PDB database generation
//! - ANLZ file generation
//! - Communication with CLI client via TCP socket

mod analyzer;
mod config;
mod export;
mod navidrome;
mod server;
mod waveform;

use std::path::PathBuf;

use clap::Parser;
use tracing::{info, Level};
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};

use bvault_core::AnalysisCache;
use config::{Config, NavidromeConfig};

#[derive(Parser, Debug)]
#[command(name = "bvault-server")]
#[command(about = "Pioneer DJ export server for NAS deployment")]
struct Args {
    /// Music directory to analyze
    #[arg(short, long, default_value = "/mnt/ssd/pre-export")]
    music_dir: PathBuf,
    
    /// Cache directory for analysis results
    #[arg(short, long, default_value = "/var/cache/rekordbox")]
    cache_dir: PathBuf,
    
    /// TCP bind address (host:port) - use 0.0.0.0 for network access
    #[arg(short, long, default_value = "0.0.0.0:6969")]
    bind: String,
    
    /// Run in foreground (don't daemonize)
    #[arg(short, long)]
    foreground: bool,
    
    /// Export directly to path without running server
    #[arg(short, long)]
    export: Option<PathBuf>,
    
    /// Log level (trace, debug, info, warn, error)
    #[arg(long, default_value = "info")]
    log_level: String,

    /// Log file directory (defaults to cache_dir)
    #[arg(long)]
    log_dir: Option<PathBuf>,

    /// Navidrome server URL (e.g., http://192.168.1.100:4533)
    #[arg(long, env = "NAVIDROME_URL")]
    navidrome_url: Option<String>,

    /// Navidrome username
    #[arg(long, env = "NAVIDROME_USER")]
    navidrome_user: Option<String>,

    /// Navidrome password
    #[arg(long, env = "NAVIDROME_PASS")]
    navidrome_pass: Option<String>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    
    // Setup dual logging (terminal + file)
    let level = match args.log_level.to_lowercase().as_str() {
        "trace" => Level::TRACE,
        "debug" => Level::DEBUG,
        "info" => Level::INFO,
        "warn" => Level::WARN,
        "error" => Level::ERROR,
        _ => Level::INFO,
    };

    let log_dir = args.log_dir.as_ref().unwrap_or(&args.cache_dir);

    // Ensure log directory exists
    std::fs::create_dir_all(log_dir)?;

    // Rolling file appender - daily rotation
    let file_appender = RollingFileAppender::new(
        Rotation::DAILY,
        log_dir,
        "rekordbox-server.log",
    );
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    // Build subscriber with both terminal and file output
    let filter = EnvFilter::from_default_env()
        .add_directive(level.into());

    tracing_subscriber::registry()
        .with(filter)
        .with(
            fmt::layer()
                .with_target(false)
                .compact()
        )
        .with(
            fmt::layer()
                .with_target(true)
                .with_ansi(false)
                .with_writer(non_blocking)
        )
        .init();

    // Keep the guard alive for the duration of the program
    // (moved to end of main to ensure logs are flushed)
    let _log_guard = _guard;
    
    info!("bvault-server starting");
    info!("Music directory: {:?}", args.music_dir);
    info!("Cache directory: {:?}", args.cache_dir);
    info!("Log directory: {:?}", log_dir);
    
    // Initialize cache
    let cache = AnalysisCache::new(&args.cache_dir)?;

    // Build Navidrome config if all parameters provided
    let navidrome = match (&args.navidrome_url, &args.navidrome_user, &args.navidrome_pass) {
        (Some(url), Some(user), Some(pass)) => {
            info!("Navidrome integration enabled: {}", url);
            Some(NavidromeConfig::new(url.clone(), user.clone(), pass.clone()))
        }
        (Some(_), _, _) | (_, Some(_), _) | (_, _, Some(_)) => {
            tracing::warn!("Navidrome config incomplete - need --navidrome-url, --navidrome-user, and --navidrome-pass");
            None
        }
        _ => None,
    };

    let config = Config {
        music_dir: args.music_dir,
        cache_dir: args.cache_dir,
        output_dir: args.export.clone(),
        bind_addr: args.bind,
        max_concurrent: 1, // Single-threaded for memory efficiency
        navidrome,
    };
    
    // If --export is specified, run export directly and exit
    if let Some(output_path) = args.export {
        info!("Running direct export to {:?}", output_path);

        let result = analyzer::analyze_directory(&config, &cache).await?;
        export::export_usb(&result.tracks, &result.playlists, &config.music_dir, &output_path)?;

        info!("Export complete");
        return Ok(());
    }
    
    // Otherwise run as server
    server::run(config, cache).await
}
