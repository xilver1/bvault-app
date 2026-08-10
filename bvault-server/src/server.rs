//! Unix socket server for CLI communication
//!
//! Provides a simple JSON-RPC style interface for the lightweight CLI client.

use std::sync::Arc;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use serde::{Deserialize, Serialize};
use tracing::{info, warn, error, debug};

use bvault_core::AnalysisCache;
use crate::config::Config;
use crate::analyzer;
use crate::export;

/// Server state
struct ServerState {
    config: Config,
    cache: AnalysisCache,
}

/// Request from CLI client
#[derive(Debug, Deserialize)]
#[serde(tag = "method")]
#[serde(rename_all = "snake_case")]
enum Request {
    Analyze { path: Option<String> },
    Export { output: String },
    Status,
    CacheStats,
    CacheClear,
    ListTracks,
}

/// Response to CLI client
#[derive(Debug, Serialize)]
struct Response {
    success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<serde_json::Value>,
}

impl Response {
    fn ok(message: impl Into<String>) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: None,
        }
    }
    
    fn ok_with_data(message: impl Into<String>, data: serde_json::Value) -> Self {
        Self {
            success: true,
            message: Some(message.into()),
            data: Some(data),
        }
    }
    
    fn error(message: impl Into<String>) -> Self {
        Self {
            success: false,
            message: Some(message.into()),
            data: None,
        }
    }
}

/// Run the server
pub async fn run(config: Config, cache: AnalysisCache) -> anyhow::Result<()> {
    let bind_addr = &config.bind_addr;

    // Create TCP listener
    let listener = TcpListener::bind(bind_addr).await?;
    info!("Server listening on {}", bind_addr);

    let state = Arc::new(Mutex::new(ServerState { config, cache }));

    loop {
        match listener.accept().await {
            Ok((stream, addr)) => {
                debug!("Client connected from {}", addr);
                let state = Arc::clone(&state);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, state).await {
                        error!("Client error: {}", e);
                    }
                });
            }
            Err(e) => {
                warn!("Accept error: {}", e);
            }
        }
    }
}

/// Handle a single client connection
async fn handle_client(
    stream: TcpStream,
    state: Arc<Mutex<ServerState>>,
) -> anyhow::Result<()> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    
    while reader.read_line(&mut line).await? > 0 {
        debug!("Received: {}", line.trim());
        
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(request) => handle_request(request, &state).await,
            Err(e) => Response::error(format!("Invalid request: {}", e)),
        };
        
        let response_json = serde_json::to_string(&response)?;
        writer.write_all(response_json.as_bytes()).await?;
        writer.write_all(b"\n").await?;
        writer.flush().await?;
        
        line.clear();
    }
    
    Ok(())
}

/// Process a request
async fn handle_request(
    request: Request,
    state: &Arc<Mutex<ServerState>>,
) -> Response {
    match request {
        Request::Analyze { path } => {
            let state_guard = state.lock().await;
            let music_dir = path
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| state_guard.config.music_dir.clone());
            
            let config = Config {
                music_dir,
                ..state_guard.config.clone()
            };
            
            match analyzer::analyze_directory(&config, &state_guard.cache).await {
                Ok(result) => {
                    Response::ok_with_data(
                        format!("Analyzed {} tracks in {} playlists",
                                result.tracks.len(), result.playlists.len()),
                        serde_json::json!({
                            "track_count": result.tracks.len(),
                            "playlist_count": result.playlists.len(),
                            "tracks": result.tracks.iter().map(|t| serde_json::json!({
                                "id": t.id,
                                "title": t.title,
                                "artist": t.artist,
                                "bpm": t.bpm,
                                "key": t.key.map(|k| k.to_camelot()),
                                "duration": t.duration_secs,
                            })).collect::<Vec<_>>(),
                            "playlists": result.playlists.keys().collect::<Vec<_>>()
                        })
                    )
                }
                Err(e) => Response::error(format!("Analysis failed: {}", e)),
            }
        }

        Request::Export { output } => {
            let state_guard = state.lock().await;
            let output_path = std::path::Path::new(&output);

            // First analyze
            match analyzer::analyze_directory(&state_guard.config, &state_guard.cache).await {
                Ok(result) => {
                    match export::export_usb(
                        &result.tracks,
                        &result.playlists,
                        &state_guard.config.music_dir,
                        output_path
                    ) {
                        Ok(()) => Response::ok(format!("Exported {} tracks to {}", result.tracks.len(), output)),
                        Err(e) => Response::error(format!("Export failed: {}", e)),
                    }
                }
                Err(e) => Response::error(format!("Analysis failed: {}", e)),
            }
        }
        
        Request::Status => {
            Response::ok("Server running")
        }
        
        Request::CacheStats => {
            let state_guard = state.lock().await;
            match state_guard.cache.stats() {
                Ok(stats) => Response::ok_with_data(
                    "Cache statistics",
                    serde_json::json!({
                        "entries": stats.entry_count,
                        "size_bytes": stats.total_size_bytes,
                        "size_mb": stats.total_size_bytes as f64 / 1024.0 / 1024.0,
                    })
                ),
                Err(e) => Response::error(format!("Failed to get cache stats: {}", e)),
            }
        }
        
        Request::CacheClear => {
            let state_guard = state.lock().await;
            match state_guard.cache.clear() {
                Ok(()) => Response::ok("Cache cleared"),
                Err(e) => Response::error(format!("Failed to clear cache: {}", e)),
            }
        }
        
        Request::ListTracks => {
            let state_guard = state.lock().await;
            match analyzer::analyze_directory(&state_guard.config, &state_guard.cache).await {
                Ok(result) => Response::ok_with_data(
                    format!("{} tracks found in {} playlists",
                            result.tracks.len(), result.playlists.len()),
                    serde_json::json!({
                        "tracks": result.tracks.iter().map(|t| serde_json::json!({
                            "id": t.id,
                            "path": t.file_path,
                            "title": t.title,
                            "artist": t.artist,
                            "album": t.album,
                            "bpm": t.bpm,
                            "key": t.key.map(|k| k.to_camelot()),
                            "duration": t.duration_secs,
                        })).collect::<Vec<_>>(),
                        "playlists": result.playlists.iter().map(|(name, ids)| {
                            serde_json::json!({
                                "name": name,
                                "track_ids": ids,
                            })
                        }).collect::<Vec<_>>()
                    })
                ),
                Err(e) => Response::error(format!("Failed to list tracks: {}", e)),
            }
        }
    }
}
