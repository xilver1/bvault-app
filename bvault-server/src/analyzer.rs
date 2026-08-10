//! Audio analysis pipeline
//!
//! Memory-efficient audio processing using Symphonia for decoding.

use std::collections::HashMap;
use std::path::Path;
use std::fs::File;

use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tracing::{info, warn, debug};
use walkdir::WalkDir;

use bvault_core::{
    AnalysisCache, compute_file_hash,
    TrackAnalysis, BeatGrid, FileType,
};
use crate::config::Config;
use crate::navidrome::{NavidromeClient, build_path_to_playlist_map};
use crate::waveform::WaveformGenerator;

/// Result of directory analysis
pub struct AnalysisResult {
    /// Analyzed tracks
    pub tracks: Vec<TrackAnalysis>,
    /// Playlist name -> track IDs
    pub playlists: HashMap<String, Vec<u32>>,
}

/// Analyze all audio files in a directory
pub async fn analyze_directory(
    config: &Config,
    cache: &AnalysisCache,
) -> anyhow::Result<AnalysisResult> {
    // Try to fetch playlists from Navidrome if configured
    let navidrome_playlists = if let Some(ref nav_config) = config.navidrome {
        match fetch_navidrome_playlists(nav_config).await {
            Ok(playlists) => {
                info!("Loaded {} playlists from Navidrome", playlists.len());
                Some(playlists)
            }
            Err(e) => {
                warn!("Failed to fetch Navidrome playlists: {}. Falling back to folder-based detection.", e);
                None
            }
        }
    } else {
        None
    };

    // Build path-to-playlist map from Navidrome data
    let path_to_playlist: HashMap<String, String> = navidrome_playlists
        .as_ref()
        .map(|p| build_path_to_playlist_map(p))
        .unwrap_or_default();

    let mut results = Vec::new();
    let mut playlists: HashMap<String, Vec<u32>> = HashMap::new();
    let mut track_id = 1u32;

    // Scan music directory
    for entry in WalkDir::new(&config.music_dir)
        .follow_links(true)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Check if audio file
        if !is_audio_file(path) {
            continue;
        }

        // Determine playlist name
        let playlist_name = determine_playlist_name(
            path,
            &config.music_dir,
            &path_to_playlist,
        );

        // Compute file hash for cache lookup
        let file_hash = match compute_file_hash(path) {
            Ok(h) => h,
            Err(e) => {
                warn!("Failed to hash {:?}: {}", path, e);
                continue;
            }
        };

        // Check cache first
        if let Some(mut cached) = cache.get(file_hash) {
            debug!("Cache hit for {:?}", path);
            cached.id = track_id;

            if let Some(ref name) = playlist_name {
                playlists.entry(name.clone()).or_default().push(track_id);
            }
            results.push(cached);
            track_id += 1;
            continue;
        }

        info!("Analyzing: {:?}", path);

        // Analyze track
        match analyze_track(path, track_id, file_hash) {
            Ok(analysis) => {
                // Cache the result
                if let Err(e) = cache.put(&analysis) {
                    warn!("Failed to cache analysis: {}", e);
                }

                if let Some(ref name) = playlist_name {
                    playlists.entry(name.clone()).or_default().push(track_id);
                }
                results.push(analysis);
                track_id += 1;
            }
            Err(e) => {
                warn!("Failed to analyze {:?}: {}", path, e);
            }
        }
    }

    info!(
        "Analyzed {} tracks in {} playlists",
        results.len(),
        playlists.len()
    );

    Ok(AnalysisResult {
        tracks: results,
        playlists,
    })
}

/// Fetch playlists from Navidrome
async fn fetch_navidrome_playlists(
    config: &crate::config::NavidromeConfig,
) -> anyhow::Result<HashMap<String, Vec<crate::navidrome::PlaylistTrack>>> {
    let client = NavidromeClient::new(&config.url, &config.user, &config.pass);

    // Test connection first
    if !client.ping().await? {
        anyhow::bail!("Failed to connect to Navidrome");
    }

    client.get_all_playlist_tracks().await
}

/// Determine playlist name for a track
///
/// Priority:
/// 1. Navidrome playlist (if path matches)
/// 2. Folder name (if not in music_dir root)
/// 3. None (standalone track)
fn determine_playlist_name(
    path: &Path,
    music_dir: &Path,
    path_to_playlist: &HashMap<String, String>,
) -> Option<String> {
    // Try to get relative path from music_dir
    let relative_path = path.strip_prefix(music_dir).ok()?;
    let relative_str = relative_path.to_str()?;

    // Normalize path separators for matching
    let normalized = relative_str.replace('\\', "/");

    // Check Navidrome playlist first
    if let Some(playlist_name) = path_to_playlist.get(&normalized) {
        return Some(playlist_name.clone());
    }

    // Fall back to folder-based detection
    // If track is directly in music_dir, it's a standalone track (no playlist)
    let parent = path.parent()?;
    if parent == music_dir {
        return None; // Standalone track
    }

    // Use immediate parent folder as playlist name
    parent
        .file_name()
        .and_then(|n| n.to_str())
        .map(|s| s.to_string())
}

/// Analyze a single audio track
fn analyze_track(
    path: &Path,
    track_id: u32,
    file_hash: u64,
) -> anyhow::Result<TrackAnalysis> {
    // Open audio file
    let file = File::open(path)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    
    // Probe format
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    
    let probed = symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )?;
    
    let mut format = probed.format;
    
    // Get track info - extract what we need before mutable borrows
    let (codec_track_id, sample_rate, bit_depth, bitrate, codec_params) = {
        let track = format.default_track()
            .ok_or_else(|| anyhow::anyhow!("No default track"))?;
        let sample_rate = track.codec_params.sample_rate
            .ok_or_else(|| anyhow::anyhow!("Unknown sample rate"))?;
        let bit_depth = track.codec_params.bits_per_sample.unwrap_or(16) as u16;
        // Extract bitrate in kbps, default to 320 if not available
        let bitrate = track.codec_params.bits_per_coded_sample
            .map(|bps| (bps * sample_rate / 1000) as u32)
            .or_else(|| {
                // For lossless formats, estimate from sample rate and bit depth
                match bit_depth {
                    16 => Some(sample_rate * 16 * 2 / 1000), // stereo 16-bit
                    24 => Some(sample_rate * 24 * 2 / 1000), // stereo 24-bit
                    _ => None,
                }
            })
            .unwrap_or(320);
        (track.id, sample_rate, bit_depth, bitrate, track.codec_params.clone())
    };

    // Create decoder
    let mut decoder = symphonia::default::get_codecs().make(
        &codec_params,
        &DecoderOptions::default(),
    )?;

    // Extract metadata
    let (title, artist, album, genre, year, track_number) = extract_metadata(&mut format, path);
    
    // Get file type
    let file_type = path.extension()
        .and_then(|e| e.to_str())
        .map(FileType::from_extension)
        .unwrap_or_default();
    
    // Collect samples for analysis (downsample to mono float)
    let mut samples: Vec<f32> = Vec::new();
    let mut total_samples = 0u64;
    
    // Memory limit: ~50MB of samples
    const MAX_SAMPLES: usize = 12_500_000;
    
    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            Err(symphonia::core::errors::Error::IoError(ref e)) 
                if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(e.into()),
        };
        
        if packet.track_id() != codec_track_id {
            continue;
        }
        
        let decoded = decoder.decode(&packet)?;
        total_samples += decoded.frames() as u64;
        
        if samples.len() < MAX_SAMPLES {
            append_as_mono_f32(&decoded, &mut samples);
        }
    }
    
    let duration_secs = total_samples as f64 / sample_rate as f64;
    debug!("Decoded {} samples, duration: {:.1}s", total_samples, duration_secs);
    
    // BPM detection
    let bpm = detect_bpm(&samples, sample_rate)?;
    info!("Detected BPM: {:.1}", bpm);
    
    // Key detection (TODO: implement properly)
    let key = None;
    
    // Generate beat grid
    let first_beat_ms = detect_first_beat(&samples, sample_rate);
    let beat_grid = BeatGrid::constant_tempo(bpm, first_beat_ms, duration_secs * 1000.0);
    
    // Generate waveforms
    let waveform_gen = WaveformGenerator::new(sample_rate);
    let waveform = waveform_gen.generate(&samples, duration_secs);
    
    // Build relative file path for database
    let file_name = path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown");
    let file_path = format!("/Contents/{}", file_name);
    
    let file_size = std::fs::metadata(path)?.len();
    
    Ok(TrackAnalysis {
        id: track_id,
        file_path,
        title,
        artist,
        album,
        genre,
        label: None, // Could be extracted from metadata if available
        duration_secs,
        sample_rate,
        bit_depth,
        bitrate,
        bpm,
        key,
        beat_grid,
        waveform,
        cue_points: Vec::new(), // No cue points detected yet (can be added from Navidrome)
        file_size,
        file_hash,
        year,
        comment: None,
        track_number,
        file_type,
    })
}

/// Convert decoded audio to mono f32
fn append_as_mono_f32(buffer: &AudioBufferRef, output: &mut Vec<f32>) {
    match buffer {
        AudioBufferRef::F32(buf) => {
            let channels = buf.spec().channels.count();
            for frame in 0..buf.frames() {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    sum += buf.chan(ch)[frame];
                }
                output.push(sum / channels as f32);
            }
        }
        AudioBufferRef::S16(buf) => {
            let channels = buf.spec().channels.count();
            for frame in 0..buf.frames() {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    sum += buf.chan(ch)[frame] as f32 / 32768.0;
                }
                output.push(sum / channels as f32);
            }
        }
        AudioBufferRef::S32(buf) => {
            let channels = buf.spec().channels.count();
            for frame in 0..buf.frames() {
                let mut sum = 0.0f32;
                for ch in 0..channels {
                    sum += buf.chan(ch)[frame] as f32 / 2147483648.0;
                }
                output.push(sum / channels as f32);
            }
        }
        _ => {
            debug!("Unsupported sample format, skipping");
        }
    }
}

/// Detect BPM using autocorrelation
fn detect_bpm(samples: &[f32], sample_rate: u32) -> anyhow::Result<f64> {
    if samples.is_empty() {
        return Ok(120.0); // Default
    }
    
    // Use first ~30 seconds for BPM detection
    let analysis_samples = std::cmp::min(samples.len(), (sample_rate * 30) as usize);
    let samples = &samples[..analysis_samples];
    
    // Onset detection via envelope following.
    // 4ms hops (250 Hz) instead of 10ms: finer envelope resolution means the
    // autocorrelation lag maps to BPM more precisely, so parabolic interpolation
    // can resolve hundredths (golden stores e.g. 138.97) rather than coarse steps.
    let hop_size = (sample_rate as usize / 250).max(1);
    let mut envelope = Vec::new();
    
    for chunk in samples.chunks(hop_size) {
        let rms: f32 = (chunk.iter().map(|s| s * s).sum::<f32>() / chunk.len() as f32).sqrt();
        envelope.push(rms);
    }
    
    if envelope.is_empty() {
        return Ok(120.0);
    }
    
    // Normalize envelope
    let max_env = envelope.iter().cloned().fold(0.0f32, f32::max);
    if max_env > 0.0 {
        for e in &mut envelope {
            *e /= max_env;
        }
    }
    
    // Autocorrelation for tempo detection. Envelope sample rate now ~250 Hz.
    let env_rate = sample_rate as f64 / hop_size as f64;
    let min_lag = (env_rate * 60.0 / 200.0) as usize; // 200 BPM
    let max_lag = (env_rate * 60.0 / 60.0) as usize;  // 60 BPM
    
    let mut best_lag = min_lag;
    let mut best_correlation = 0.0f32;

    // Store correlations so we can interpolate around the peak.
    let hi = max_lag.min(envelope.len().saturating_sub(1));
    let mut corr = vec![0.0f32; hi + 1];

    for lag in min_lag..=hi {
        let mut correlation = 0.0f32;
        let count = envelope.len() - lag;
        for i in 0..count {
            correlation += envelope[i] * envelope[i + lag];
        }
        correlation /= count as f32;
        corr[lag] = correlation;

        if correlation > best_correlation {
            best_correlation = correlation;
            best_lag = lag;
        }
    }

    // Parabolic interpolation around the integer peak to recover sub-lag
    // precision. Integer lags alone quantize BPM coarsely (e.g. lag 43 -> 139.53,
    // lag 44 -> 136.36 near 140 BPM), which is why exact values like 138.97 were
    // unreachable and the old "round to 0.5" made it worse.
    let refined_lag = if best_lag > min_lag && best_lag < hi {
        let y0 = corr[best_lag - 1];
        let y1 = corr[best_lag];
        let y2 = corr[best_lag + 1];
        let denom = y0 - 2.0 * y1 + y2;
        if denom.abs() > 1e-9 {
            let delta = 0.5 * (y0 - y2) / denom; // in [-1, 1]
            best_lag as f64 + delta as f64
        } else {
            best_lag as f64
        }
    } else {
        best_lag as f64
    };

    let mut best_bpm = env_rate * 60.0 / refined_lag;

    // Octave correction: autocorrelation often locks onto the half-tempo (bar)
    // lag, giving e.g. 70.5 instead of 141. Fold into a canonical DJ range.
    // Techno/house/most 4-on-the-floor material sits ~90-180 BPM.
    while best_bpm < 90.0 {
        best_bpm *= 2.0;
    }
    while best_bpm > 180.0 {
        best_bpm /= 2.0;
    }

    // No 0.5 quantization - rekordbox stores hundredths (e.g. 138.97).
    Ok(best_bpm)
}

/// Find first beat position in milliseconds
fn detect_first_beat(samples: &[f32], sample_rate: u32) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    
    // Look for first significant onset in first few seconds
    let search_samples = std::cmp::min(samples.len(), (sample_rate * 5) as usize);
    let hop_size = sample_rate as usize / 200; // 5ms hops
    
    let mut onset_strength = Vec::new();
    let mut prev_energy = 0.0f32;
    
    for chunk in samples[..search_samples].chunks(hop_size) {
        let energy: f32 = chunk.iter().map(|s| s * s).sum();
        let onset = (energy - prev_energy).max(0.0);
        onset_strength.push(onset);
        prev_energy = energy;
    }
    
    if onset_strength.is_empty() {
        return 0.0;
    }
    
    // Find first strong onset
    let threshold = onset_strength.iter().cloned().fold(0.0f32, f32::max) * 0.3;
    
    for (i, &strength) in onset_strength.iter().enumerate() {
        if strength > threshold {
            let sample_pos = i * hop_size;
            return sample_pos as f64 / sample_rate as f64 * 1000.0;
        }
    }
    
    0.0
}

/// Extract metadata from audio file
fn extract_metadata(
    format: &mut Box<dyn symphonia::core::formats::FormatReader>,
    path: &Path,
) -> (String, String, Option<String>, Option<String>, Option<u16>, Option<u32>) {
    let mut title = path.file_stem()
        .and_then(|n| n.to_str())
        .unwrap_or("Unknown")
        .to_string();
    let mut artist = "Unknown Artist".to_string();
    let mut album = None;
    let mut genre = None;
    let mut year = None;
    let mut track_number = None;
    
    // Try to get metadata from format
    if let Some(metadata) = format.metadata().current() {
        for tag in metadata.tags() {
            match tag.std_key {
                Some(symphonia::core::meta::StandardTagKey::TrackTitle) => {
                    title = tag.value.to_string();
                }
                Some(symphonia::core::meta::StandardTagKey::Artist) => {
                    artist = tag.value.to_string();
                }
                Some(symphonia::core::meta::StandardTagKey::Album) => {
                    album = Some(tag.value.to_string());
                }
                Some(symphonia::core::meta::StandardTagKey::Genre) => {
                    genre = Some(tag.value.to_string());
                }
                Some(symphonia::core::meta::StandardTagKey::Date) => {
                    // Try to parse year
                    if let Ok(y) = tag.value.to_string().get(..4).unwrap_or("").parse::<u16>() {
                        year = Some(y);
                    }
                }
                Some(symphonia::core::meta::StandardTagKey::TrackNumber) => {
                    if let Ok(n) = tag.value.to_string().parse::<u32>() {
                        track_number = Some(n);
                    }
                }
                _ => {}
            }
        }
    }
    
    (title, artist, album, genre, year, track_number)
}

/// Check if path is a supported audio file
fn is_audio_file(path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    
    let ext = path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_lowercase());
    
    matches!(ext.as_deref(), Some("mp3" | "flac" | "wav" | "aiff" | "aif" | "m4a" | "aac"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    use std::fs::File;

    #[test]
    fn test_is_audio_file() {
        let tmp = TempDir::new().unwrap();

        // Create test files
        let mp3_path = tmp.path().join("test.mp3");
        File::create(&mp3_path).unwrap();
        let flac_path = tmp.path().join("TEST.FLAC");
        File::create(&flac_path).unwrap();
        let txt_path = tmp.path().join("test.txt");
        File::create(&txt_path).unwrap();
        let no_ext_path = tmp.path().join("test");
        File::create(&no_ext_path).unwrap();

        assert!(is_audio_file(&mp3_path));
        assert!(is_audio_file(&flac_path));
        assert!(!is_audio_file(&txt_path));
        assert!(!is_audio_file(&no_ext_path));
        // Non-existent file should return false
        assert!(!is_audio_file(Path::new("nonexistent.mp3")));
    }
}