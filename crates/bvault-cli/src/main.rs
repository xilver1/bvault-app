use clap::{Parser, Subcommand};
use anyhow::Result;
use tracing_subscriber;

mod client;
mod tui;
mod export;
mod login;
mod logout;
mod register;
mod ingest;
mod library;
mod playlist;

#[derive(Parser)]
#[command(name = "bvault")]
#[command(about = "BeatVault command line client", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Login to BeatVault
    Login,
    /// Logout from BeatVault
    Logout,
    /// Register a new account
    Register,
    /// Ingest a track (YouTube)
    Ingest {
        #[arg(required_unless_present = "youtube_sso")]
        query: Option<String>,
        #[arg(long, conflicts_with_all = ["local", "gdrive", "youtube_sso", "youtube_playlist"])]
        youtube: bool,
        #[arg(long, conflicts_with_all = ["youtube", "gdrive", "youtube_sso", "youtube_playlist"])]
        local: bool,
        #[arg(long, conflicts_with_all = ["youtube", "local", "youtube_sso", "youtube_playlist"])]
        gdrive: bool,
        #[arg(long, conflicts_with_all = ["youtube", "local", "gdrive", "youtube_playlist"])]
        youtube_sso: bool,
        #[arg(long, conflicts_with_all = ["youtube", "local", "gdrive", "youtube_sso"])]
        youtube_playlist: bool,
        /// For --local on a directory: make each top-level subfolder a playlist.
        #[arg(long, requires = "local")]
        playlists: bool,
    },
    /// List library (optionally filter by title)
    Library {
        /// Case-insensitive title search
        #[arg(long)]
        search: Option<String>,
    },
    /// Manage playlists
    Playlist {
        #[command(subcommand)]
        command: PlaylistCommands,
    },
    /// Export a playlist to USB
    Export {
        playlist_name: String,
        #[arg(long, conflicts_with = "path")]
        usb: bool,
        #[arg(long, conflicts_with = "usb")]
        path: Option<String>,
    },
}

#[derive(Subcommand)]
enum PlaylistCommands {
    /// List all playlists
    List,
    /// View tracks in a playlist
    View { name: String },
    /// Create or add tracks to a playlist
    Add { name: String, tracks: Option<String> },
    /// Remove tracks from a playlist
    Remove { name: String, tracks: String },
    /// Delete a playlist
    Delete { name: String },
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter("error") // only show errors so CLI TUI remains clean
        .init();

    let cli = Cli::parse();

    match &cli.command {
        Commands::Login => {
            login::run_login_flow().await?;
        }
        Commands::Logout => {
            logout::run_logout_flow().await?;
        }
        Commands::Register => {
            register::run_register_flow().await?;
        }
        Commands::Ingest { query, youtube, local, gdrive, youtube_sso, youtube_playlist, playlists } => {
            ingest::run_ingest_flow(query.as_deref(), *youtube, *local, *gdrive, *youtube_sso, *youtube_playlist, *playlists).await?;
        }
        Commands::Library { search } => {
            library::run_library_flow(search.as_deref()).await?;
        }
        Commands::Playlist { command: playlist_cmd } => {
            match playlist_cmd {
                PlaylistCommands::List => {
                    playlist::run_playlist_list_flow().await?;
                }
                PlaylistCommands::View { name } => {
                    playlist::run_playlist_view_flow(name).await?;
                }
                PlaylistCommands::Add { name, tracks } => {
                    playlist::run_playlist_add_flow(name, tracks).await?;
                }
                PlaylistCommands::Remove { name, tracks } => {
                    playlist::run_playlist_remove_flow(name, tracks).await?;
                }
                PlaylistCommands::Delete { name } => {
                    playlist::run_playlist_delete_flow(name).await?;
                }
            }
        }
        Commands::Export { playlist_name, usb, path } => {
            export::run_export_flow(playlist_name, *usb, path.clone()).await?;
        }
    }

    Ok(())
}
