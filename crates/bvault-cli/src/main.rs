use clap::{Parser, Subcommand};
use anyhow::Result;
use tracing_subscriber;

mod client;
mod tui;
mod export;
mod login;
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
    /// Register a new account
    Register,
    /// Ingest a track (YouTube)
    Ingest {
        url: String,
        #[arg(long, conflicts_with_all = ["local", "gdrive"])]
        youtube: bool,
        #[arg(long, conflicts_with_all = ["youtube", "gdrive"])]
        local: bool,
        #[arg(long, conflicts_with_all = ["youtube", "local"])]
        gdrive: bool,
    },
    /// List library
    Library,
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
        Commands::Register => {
            register::run_register_flow().await?;
        }
        Commands::Ingest { url, youtube, local, gdrive } => {
            ingest::run_ingest_flow(url, *youtube, *local, *gdrive).await?;
        }
        Commands::Library => {
            library::run_library_flow().await?;
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
