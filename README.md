# BeatVault

BeatVault is a robust backend ecosystem and command-line interface designed to streamline DJ library management, track analysis, and USB exports for Rekordbox-compatible hardware.

## `bvault-cli`
The `bvault-cli` is the primary interface for interacting with the BeatVault server.

### Installation
You can build the CLI from source using cargo:
```bash
cargo build --release -p bvault-cli
```
The compiled binary will be located in `target/release/bvault`.

### Commands Overview

#### Authentication
- **`bvault login`**: Interactively login to BeatVault.
- **`bvault logout`**: Logout from BeatVault and delete your local session.
- **`bvault register`**: Interactively register a new account on the BeatVault server.

#### Ingestion
BeatVault supports multiple ingestion paths. Newly ingested tracks are automatically queued for background analysis (FFT, waveforms, beatgrids). All ingest commands support the `--bg` flag to queue the jobs in the background without waiting.
- **`bvault ingest <query> --youtube`**: Download and extract audio from a single YouTube URL.
- **`bvault ingest <query> --youtube-playlist`**: Ingest an entire YouTube playlist.
- **`bvault ingest --youtube-sso`**: Interactive YouTube single sign-on flow for authenticated downloads.
- **`bvault ingest <path> --local`**: Ingest local audio files or a directory.
  - Optional flag: `--playlists` will automatically create BeatVault playlists based on the top-level subfolders of your local directory.
- **`bvault ingest <folder_id> --gdrive`**: Import audio files directly from a Google Drive folder.

#### Library Management
- **`bvault library`**: View all tracks in your library. It dynamically displays accurate track duration, BPM, and file sizes.
  - Optional flag: `--search <title>` to filter the library by track title.

#### Playlists
- **`bvault playlist list`**: List all of your playlists.
- **`bvault playlist view <name>`**: View the tracks inside a specific playlist.
- **`bvault playlist add <name> [tracks]`**: Create a new playlist or add tracks to an existing one. `[tracks]` can be a comma-separated list of track hashes.
- **`bvault playlist remove <name> <tracks>`**: Remove specific tracks from a playlist.
- **`bvault playlist delete <name>`**: Delete a playlist.

#### Exporting
- **`bvault export <playlist_name> --usb`**: Automatically locate your Pioneer DJ USB drive, generate an `export.pdb` and `ANLZ` files statelessly, and transfer the necessary files directly to the USB.
- **`bvault export <playlist_name> --path <path>`**: Export the database, tracks, and analysis files to a specific local directory instead of auto-detecting a USB.

*Note: Currently, the CLI `export` command only supports exporting a single playlist at a time. Exporting a new playlist to a USB will overwrite its previous `export.pdb` database.*

#### Downloading
- **`bvault download "<playlist_name>" --playlist --out <dir>`**: Download all raw MP3/FLAC files from a playlist into a local directory.
- **`bvault download "<track_name1>, <track_name2>"`**: Download specific tracks (comma-separated list) to your current directory using fuzzy search.

#### Background Jobs
- **`bvault status`**: Show aggregate background job status across all queues.
- **`bvault status ingest`**: Monitor the live progress of your last background ingest run.
- **`bvault status analysis`**: Monitor the live progress of the global analysis queue.
