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
    /// Create a playlist
    Playlist {
        name: String,
        #[arg(long)]
        add: Option<String>, // comma separated
    },
    /// Export a playlist to USB
    Export {
        playlist_name: String,
        #[arg(long)]
        usb: bool,
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
        Commands::Register => {
            register::run_register_flow().await?;
        }
        Commands::Ingest { url, youtube, local, gdrive } => {
            ingest::run_ingest_flow(url, *youtube, *local, *gdrive).await?;
        }
        Commands::Library => {
            library::run_library_flow().await?;
        }
        Commands::Playlist { name, add } => {
            playlist::run_playlist_flow(name, add).await?;
        }
        Commands::Export { playlist_name, usb: _ } => {
            export::run_export_flow(playlist_name).await?;
        }
    }

    Ok(())
}
