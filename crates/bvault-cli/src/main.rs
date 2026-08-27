use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber;

mod client;
mod download;
mod export;
mod ingest;
mod library;
mod login;
mod logout;
mod playlist;
mod register;
mod status;
mod tui;

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
    /// Check background job status
    Status {
        #[command(subcommand)]
        command: Option<StatusCommands>,
    },
    /// Ingest a track or directory
    Ingest {
        #[command(subcommand)]
        command: IngestCommands,
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
    /// Download raw audio files
    Download {
        /// Playlist name or comma-separated track names
        query: String,
        /// Whether the query is a playlist name
        #[arg(long)]
        playlist: bool,
        /// Output directory
        #[arg(long, default_value = ".")]
        out: String,
    },
}

#[derive(Subcommand)]
enum PlaylistCommands {
    /// List all playlists
    List,
    /// View tracks in a playlist
    View { name: String },
    /// Create or add tracks to a playlist
    Add {
        name: String,
        tracks: Option<String>,
    },
    /// Remove tracks from a playlist
    Remove { name: String, tracks: String },
    /// Delete a playlist
    Delete { name: String },
}

#[derive(Subcommand)]
pub enum StatusCommands {
    /// Monitor ingest queue progress
    Ingest,
    /// Monitor analysis queue progress
    Analysis,
}

#[derive(Subcommand)]
pub enum IngestCommands {
    /// Ingest from YouTube
    Youtube {
        /// Query URL of single video or playlist
        query: Option<String>,
        /// Execute SSO authorization flow and store credentials
        #[arg(long, exclusive = true)]
        login: bool,
        /// List your YouTube playlists
        #[arg(long, conflicts_with = "login")]
        playlists: bool,
        /// Do not wait for jobs to finish, queue them in the background
        #[arg(long, conflicts_with = "login")]
        bg: bool,
    },
    /// Ingest from local filesystem
    Local {
        /// Directory or file path
        path: String,
        /// Make each top-level subfolder a playlist
        #[arg(long)]
        playlists: bool,
        /// Do not wait for jobs to finish, queue them in the background
        #[arg(long)]
        bg: bool,
    },
    /// Ingest from Google Drive
    Gdrive {
        /// Google Drive folder path
        path: String,
        /// Do not wait for jobs to finish, queue them in the background
        #[arg(long)]
        bg: bool,
    },
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
        Commands::Status { command } => {
            status::run_status_flow(command.as_ref()).await?;
        }
        Commands::Ingest { command } => match command {
            IngestCommands::Youtube {
                query,
                login,
                playlists,
                bg,
            } => {
                ingest::run_youtube_ingest(query.as_deref(), *login, *playlists, *bg).await?;
            }
            IngestCommands::Local {
                path,
                playlists,
                bg,
            } => {
                ingest::run_local_ingest_cmd(path, *playlists, *bg).await?;
            }
            IngestCommands::Gdrive { path, bg } => {
                ingest::run_gdrive_ingest(path, *bg).await?;
            }
        },
        Commands::Library { search } => {
            library::run_library_flow(search.as_deref()).await?;
        }
        Commands::Playlist {
            command: playlist_cmd,
        } => match playlist_cmd {
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
        },
        Commands::Export {
            playlist_name,
            usb,
            path,
        } => {
            export::run_export_flow(playlist_name, *usb, path.clone()).await?;
        }
        Commands::Download {
            query,
            playlist,
            out,
        } => {
            download::run_download_flow(query, *playlist, out).await?;
        }
    }

    Ok(())
}
