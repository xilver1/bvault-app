//! rekordbox-cli: Lightweight client for Termux
//!
//! Communicates with rekordbox-server over TCP socket.
//! Designed to be tiny (<500KB) for mobile deployment.

use clap::{Parser, Subcommand};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use serde::{Deserialize, Serialize};

#[derive(Parser, Debug)]
#[command(name = "rekordbox")]
#[command(about = "Pioneer DJ export CLI client")]
struct Args {
    /// Server address (host:port)
    #[arg(short, long, default_value = "127.0.0.1:6969")]
    server: String,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Check server status
    Status,
    
    /// Analyze music directory
    Analyze {
        /// Optional path override
        #[arg(short, long)]
        path: Option<String>,
    },
    
    /// Export to USB device
    Export {
        /// Output path (USB mount point)
        output: String,
    },
    
    /// List analyzed tracks
    List,
    
    /// Show cache statistics
    CacheStats,
    
    /// Clear analysis cache
    CacheClear,
}

#[derive(Debug, Serialize)]
struct Request {
    method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output: Option<String>,
}

#[derive(Debug, Deserialize)]
struct Response {
    success: bool,
    message: Option<String>,
    data: Option<serde_json::Value>,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    
    let request = match args.command {
        Command::Status => Request {
            method: "status".into(),
            path: None,
            output: None,
        },
        Command::Analyze { ref path } => Request {
            method: "analyze".into(),
            path: path.clone(),
            output: None,
        },
        Command::Export { ref output } => Request {
            method: "export".into(),
            path: None,
            output: Some(output.clone()),
        },
        Command::List => Request {
            method: "list_tracks".into(),
            path: None,
            output: None,
        },
        Command::CacheStats => Request {
            method: "cache_stats".into(),
            path: None,
            output: None,
        },
        Command::CacheClear => Request {
            method: "cache_clear".into(),
            path: None,
            output: None,
        },
    };
    
    // Connect to server
    let stream = match TcpStream::connect(&args.server).await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to connect to server at {}: {}", args.server, e);
            eprintln!("Is rekordbox-server running?");
            std::process::exit(1);
        }
    };
    
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    
    // Send request
    let request_json = serde_json::to_string(&request)?;
    writer.write_all(request_json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;
    
    // Read response
    let mut response_line = String::new();
    reader.read_line(&mut response_line).await?;
    
    let response: Response = serde_json::from_str(&response_line)?;
    
    if response.success {
        if let Some(msg) = response.message {
            println!("✓ {}", msg);
        }
        
        if let Some(data) = response.data {
            print_data(&data, &args.command);
        }
    } else {
        eprintln!("✗ {}", response.message.unwrap_or_else(|| "Unknown error".into()));
        std::process::exit(1);
    }
    
    Ok(())
}

fn print_data(data: &serde_json::Value, command: &Command) {
    match command {
        Command::List => {
            if let Some(tracks) = data.as_array() {
                println!("\n{:<4} {:<30} {:<25} {:<8} {:<6}", "ID", "Title", "Artist", "BPM", "Key");
                println!("{}", "-".repeat(80));
                for track in tracks {
                    println!(
                        "{:<4} {:<30} {:<25} {:<8.1} {:<6}",
                        track["id"].as_u64().unwrap_or(0),
                        truncate(track["title"].as_str().unwrap_or(""), 29),
                        truncate(track["artist"].as_str().unwrap_or(""), 24),
                        track["bpm"].as_f64().unwrap_or(0.0),
                        track["key"].as_str().unwrap_or("-"),
                    );
                }
            }
        }
        Command::Analyze { .. } => {
            if let Some(tracks) = data.get("tracks").and_then(|t| t.as_array()) {
                println!("\nAnalyzed tracks:");
                for track in tracks.iter().take(10) {
                    println!(
                        "  {} - {} ({:.1} BPM, {})",
                        track["artist"].as_str().unwrap_or("?"),
                        track["title"].as_str().unwrap_or("?"),
                        track["bpm"].as_f64().unwrap_or(0.0),
                        track["key"].as_str().unwrap_or("-"),
                    );
                }
                if tracks.len() > 10 {
                    println!("  ... and {} more", tracks.len() - 10);
                }
            }
        }
        Command::CacheStats => {
            println!("\nCache statistics:");
            println!("  Entries: {}", data["entries"].as_u64().unwrap_or(0));
            println!("  Size: {:.2} MB", data["size_mb"].as_f64().unwrap_or(0.0));
        }
        _ => {
            // For other commands, just pretty-print the JSON if there's data
            if !data.is_null() {
                println!("{}", serde_json::to_string_pretty(data).unwrap_or_default());
            }
        }
    }
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        format!("{}…", &s[..max_len - 1])
    }
}
