//! bvault-analysis: pure audio-analysis front-end.
//!
//! Decodes one track and derives the intrinsic, playlist- and
//! export-independent analysis rekordbox needs: BPM, a constant-tempo beat grid,
//! and the three waveforms. The result is a [`TrackAnalysis`] whose
//! **export-time** fields (`id`, `file_path`) are left unset — those are
//! assigned by the export builder when it lays tracks out across a specific USB
//! export. Identity here is the content hash, nothing else.
//!
//! This crate does no directory walking, no caching, and no playlist logic;
//! those belong to the worker and gateway. Feed it bytes, get analysis back.

mod decode;
mod error;
mod tempo;
mod waveform;

pub use decode::{decode, DecodedAudio, Metadata};
pub use error::{Error, Result};
pub use tempo::{detect_bpm, detect_first_beat, BpmRange};
pub use waveform::WaveformGenerator;

use std::path::Path;

use symphonia::core::io::MediaSource;
use tracing::info;

use bvault_core::{BeatGrid, TrackAnalysis};

/// Tunables for a single analysis. Defaults reproduce the values validated
/// against the golden files.
#[derive(Debug, Clone)]
pub struct AnalyzeOptions {
    /// Cap on samples buffered for analysis (~50 MB at 4 bytes/sample). The full
    /// stream is still decoded for an accurate duration; only the analysis buffer
    /// is bounded, keeping peak memory predictable on the RAM-limited worker.
    pub max_samples: usize,
    /// BPM octave-folding range.
    pub bpm_range: BpmRange,
}

impl Default for AnalyzeOptions {
    fn default() -> Self {
        Self {
            max_samples: 12_500_000,
            bpm_range: BpmRange::default(),
        }
    }
}

/// Analyze an already-opened media source.
///
/// `file_size` and `file_hash` are supplied by the caller — the store already
/// knows them, and the hash *is* the identity. `fallback_title` is used only
/// when the stream carries no title tag (pass the ingestion-known name, or
/// `None`).
///
/// The returned [`TrackAnalysis`] has `id == 0` and an empty `file_path`: both
/// are assigned later by the export builder. See the module docs.
pub fn analyze_source(
    source: Box<dyn MediaSource>,
    hint_ext: Option<&str>,
    file_size: u64,
    file_hash: u64,
    fallback_title: Option<&str>,
    opts: &AnalyzeOptions,
) -> Result<TrackAnalysis> {
    let audio = decode::decode(source, hint_ext, opts.max_samples)?;

    let bpm = tempo::detect_bpm(&audio.samples, audio.sample_rate, &opts.bpm_range);
    let first_beat_ms = tempo::detect_first_beat(&audio.samples, audio.sample_rate);
    let beat_grid = BeatGrid::constant_tempo(bpm, first_beat_ms, audio.duration_secs * 1000.0);

    let waveform =
        WaveformGenerator::new(audio.sample_rate).generate(&audio.samples, audio.duration_secs);

    // Metadata fallbacks live here, not in the decoder: the decoder reports what
    // the tags say; the caller decides what "unknown" should read as.
    let title = audio
        .metadata
        .title
        .or_else(|| fallback_title.map(str::to_string))
        .unwrap_or_else(|| "Unknown Title".to_string());
    let artist = audio
        .metadata
        .artist
        .unwrap_or_else(|| "Unknown Artist".to_string());

    info!(bpm, duration = audio.duration_secs, "analyzed track");

    Ok(TrackAnalysis {
        // Export-time fields — assigned by the export builder, not here.
        id: 0,
        file_path: String::new(),

        title,
        artist,
        album: audio.metadata.album,
        genre: audio.metadata.genre,
        label: None,
        duration_secs: audio.duration_secs,
        sample_rate: audio.sample_rate,
        bit_depth: audio.bit_depth,
        bitrate: audio.bitrate,
        bpm,
        key: None, // key detection not yet implemented
        beat_grid,
        waveform,
        cue_points: Vec::new(),
        file_size,
        file_hash,
        year: audio.metadata.year,
        comment: None,
        track_number: audio.metadata.track_number,
        file_type: audio.file_type,
    })
}

/// Convenience wrapper: open a path, hash it for identity, and analyze.
///
/// Handy for the CLI and tests. The worker uses [`analyze_source`] with a reader
/// the store hands it, so it never needs a filesystem path.
pub fn analyze_path(path: &Path, opts: &AnalyzeOptions) -> Result<TrackAnalysis> {
    let mut file_for_hash = std::fs::File::open(path)?;
    let mut hasher = bvault_hash::ContentHasher::new();
    std::io::copy(&mut file_for_hash, &mut hasher).map_err(|e| Error::Hash(e.to_string()))?;
    let file_hash = hasher.finalize();
    let file_size = std::fs::metadata(path)?.len();
    let hint_ext = path.extension().and_then(|e| e.to_str());
    let fallback_title = path.file_stem().and_then(|s| s.to_str());
    let file = std::fs::File::open(path)?;
    analyze_source(
        Box::new(file),
        hint_ext,
        file_size,
        file_hash,
        fallback_title,
        opts,
    )
}

/// Whether an extension (without the dot) is a supported audio format.
/// Handy for the store/ingestion to filter inputs before enqueueing analysis.
pub fn is_supported_extension(ext: &str) -> bool {
    matches!(
        ext.to_lowercase().as_str(),
        "mp3" | "flac" | "wav" | "aiff" | "aif" | "m4a" | "aac"
    )
}