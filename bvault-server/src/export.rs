//! USB Export generation
//!
//! Creates the complete Pioneer-compatible USB directory structure:
//! - PIONEER/rekordbox/export.pdb
//! - PIONEER/USBANLZ/Pxxx/[hex]/ANLZ0000.DAT
//! - PIONEER/DEVSETTING.DAT
//! - PIONEER/djprofile.nxs
//! - Contents/[audio files]

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::Write;
use std::path::Path;

use tracing::{info, debug, warn};
use walkdir::WalkDir;

use bvault_core::{
    PdbBuilder, TrackAnalysis,
    generate_dat_file, generate_ext_file, generate_2ex_file, generate_anlz_path,
    generate_devsetting, generate_djprofile,
    generate_xml, XmlExportOptions,
    build_export_library, build_devlib_backup_json, devlib_backup_filename,
    DeviceLibraryOptions, PlaylistSpec,
};

/// Export analyzed tracks to Pioneer USB format
pub fn export_usb(
    tracks: &[TrackAnalysis],
    playlists: &HashMap<String, Vec<u32>>,
    source_dir: &Path,
    output_dir: &Path,
) -> anyhow::Result<()> {
    export_usb_with_profile(tracks, playlists, source_dir, output_dir, "bvault")
}

/// Export analyzed tracks with custom DJ profile name
pub fn export_usb_with_profile(
    tracks: &[TrackAnalysis],
    playlists: &HashMap<String, Vec<u32>>,
    source_dir: &Path,
    output_dir: &Path,
    profile_name: &str,
) -> anyhow::Result<()> {
    info!("Exporting {} tracks in {} playlists to {:?}",
          tracks.len(), playlists.len(), output_dir);

    // Validate output directory
    validate_usb_target(output_dir)?;

    // Create directory structure
    
    let pioneer_dir = output_dir.join("PIONEER");
    let rekordbox_dir = pioneer_dir.join("rekordbox");
    let anlz_dir = pioneer_dir.join("USBANLZ");
    let contents_dir = output_dir.join("Contents");
    let artwork_dir = pioneer_dir.join("Artwork");
    let backup_dir = pioneer_dir.join("DeviceLibBackup");

    fs::create_dir_all(&rekordbox_dir)?;
    fs::create_dir_all(&anlz_dir)?;
    fs::create_dir_all(&contents_dir)?;
    fs::create_dir_all(&artwork_dir)?;
    fs::create_dir_all(&backup_dir)?;

    // Build PDB database
    let mut pdb_builder = PdbBuilder::new();

    for track in tracks {
        let anlz_path = generate_anlz_path(track.id);
        pdb_builder.add_track(track, &anlz_path);
    }

    // Add playlists
    let mut playlist_id = 1u32;
    for (name, track_ids) in playlists {
        if !name.is_empty() {
            pdb_builder.add_playlist(playlist_id, 0, name, track_ids.clone());
            playlist_id += 1;
        }
    }
    
    // Write export.pdb
    let pdb_data = pdb_builder.build()?;
    let pdb_path = rekordbox_dir.join("export.pdb");
    let mut pdb_file = File::create(&pdb_path)?;
    pdb_file.write_all(&pdb_data)?;
    info!("Wrote export.pdb ({} bytes, {} pages)", pdb_data.len(), pdb_data.len() / 4096);
    
    // Write rekordbox XML
     let xml_opts = XmlExportOptions {
        music_root: source_dir.to_path_buf(),
        ..Default::default()
    };
    let xml = generate_xml(tracks, playlists, &xml_opts);
    std::fs::write(rekordbox_dir.join("rekord-export.xml"), xml)?;
    info!("Wrote rekord-export.xml");

    // --- Device Library Plus (rekordbox 6.6.5+) ---------------------------
    // rekordbox PC validates this layer independently of export.pdb and
    // reports "Device library is corrupted" if it is missing. CDJs ignore it.
    write_device_library(tracks, playlists, output_dir, &rekordbox_dir, &backup_dir)?;

    // Write DEVSETTING.DAT
    let devsetting_data = generate_devsetting();
    let devsetting_path = pioneer_dir.join("DEVSETTING.DAT");
    let mut devsetting_file = File::create(&devsetting_path)?;
    devsetting_file.write_all(&devsetting_data)?;
    debug!("Wrote DEVSETTING.DAT ({} bytes)", devsetting_data.len());
    
    // Write djprofile.nxs
    let djprofile_data = generate_djprofile(profile_name);
    let djprofile_path = pioneer_dir.join("djprofile.nxs");
    let mut djprofile_file = File::create(&djprofile_path)?;
    djprofile_file.write_all(&djprofile_data)?;
    debug!("Wrote djprofile.nxs ({} bytes)", djprofile_data.len());
    
    // Generate ANLZ files for each track
    for track in tracks {
        let anlz_rel_path = generate_anlz_path(track.id);
        let anlz_full_path = output_dir.join(&anlz_rel_path);
        
        // Create parent directories
        if let Some(parent) = anlz_full_path.parent() {
            fs::create_dir_all(parent)?;
        }
        
        // The file path stored in ANLZ should be the USB-relative path
        let usb_file_path = track.file_path.clone();
        
        // Generate .DAT file
        let total_samples =
            (track.duration_secs * track.sample_rate.max(1) as f64).round() as u64;
        let dat_data = generate_dat_file(
            &track.beat_grid,
            &track.waveform,
            &usb_file_path,
            track.file_size,
            total_samples,
        )?;
        
        let mut dat_file = File::create(&anlz_full_path)?;
        dat_file.write_all(&dat_data)?;
        debug!("Wrote ANLZ for track {}: {} bytes", track.id, dat_data.len());
        
        // Also generate .EXT file for Nexus+ compatibility
        let ext_path = anlz_full_path.with_extension("EXT");
        let ext_data = generate_ext_file(
            &track.beat_grid,
            &track.waveform,
            &usb_file_path,
            &track.cue_points,
        )?;
        let mut ext_file = File::create(&ext_path)?;
        ext_file.write_all(&ext_data)?;

        // Also generate .2EX file for CDJ-3000 and newer hardware
        let two_ex_path = anlz_full_path.with_extension("2EX");
        let two_ex_data = generate_2ex_file(
            &track.beat_grid,
            &track.waveform,
            &usb_file_path,
            &track.cue_points,
        )?;
        let mut two_ex_file = File::create(&two_ex_path)?;
        two_ex_file.write_all(&two_ex_data)?;
    }
    
    // Copy audio files to Contents directory
    copy_audio_files(tracks, source_dir, &contents_dir)?;
    
    info!("Export complete: {} tracks, {} playlists", tracks.len(), playlists.len());
    
    Ok(())
}

/// Generate the Device Library Plus layer: the encrypted `exportLibrary.db`
/// and its `DeviceLibBackup/rbDevLibBaInfo_<id>.json` companion.
fn write_device_library(
    tracks: &[TrackAnalysis],
    playlists: &HashMap<String, Vec<u32>>,
    output_dir: &Path,
    rekordbox_dir: &Path,
    backup_dir: &Path,
) -> anyhow::Result<()> {
    // Deterministic device identity derived from the export contents, so the
    // same library reproduces the same ids across runs. `master_db_id` is a
    // 31-bit positive int (matches golden's magnitude); `uuid` is 32 hex chars.
    let seed = device_seed(tracks, playlists);
    let master_db_id = (seed & 0x7fff_ffff) as i64;
    let uuid = format_uuid(seed);
    let created_date = today_iso();

    let opts = DeviceLibraryOptions {
        master_db_id,
        uuid,
        created_date,
        device_name: String::new(),
    };

    // Build a stable, ordered playlist list (HashMap iteration order is not
    // stable, so sort by name for determinism). Playlist ids are 1-based.
    let mut names: Vec<&String> = playlists.keys().filter(|n| !n.is_empty()).collect();
    names.sort();
    let specs: Vec<PlaylistSpec> = names
        .iter()
        .enumerate()
        .map(|(i, name)| PlaylistSpec {
            id: (i as i64) + 1,
            name: name.as_str(),
            content_ids: &playlists[*name],
        })
        .collect();

    // exportLibrary.db
    let db_path = rekordbox_dir.join("exportLibrary.db");
    build_export_library(&db_path, tracks, &specs, &opts)
        .map_err(|e| anyhow::anyhow!("building exportLibrary.db: {e}"))?;
    info!(
        "Wrote exportLibrary.db ({} bytes)",
        fs::metadata(&db_path).map(|m| m.len()).unwrap_or(0)
    );

    // DeviceLibBackup/rbDevLibBaInfo_<id>.json
    let json = build_devlib_backup_json(&opts)
        .map_err(|e| anyhow::anyhow!("building device backup json: {e}"))?;
    let json_path = backup_dir.join(devlib_backup_filename(&opts));
    fs::write(&json_path, json)?;
    debug!("Wrote {}", json_path.display());

    // `output_dir` is currently unused here but kept in the signature for when
    // artwork/image paths (which are USB-root relative) get wired in.
    let _ = output_dir;
    Ok(())
}

/// Derive a stable 64-bit seed from the export's track ids and playlist names.
fn device_seed(tracks: &[TrackAnalysis], playlists: &HashMap<String, Vec<u32>>) -> u64 {
    // FNV-1a over track ids and sorted playlist names — no extra deps.
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let fnv = |h: &mut u64, byte: u8| {
        *h ^= byte as u64;
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    for t in tracks {
        for b in t.id.to_le_bytes() {
            fnv(&mut hash, b);
        }
    }
    let mut names: Vec<&String> = playlists.keys().collect();
    names.sort();
    for n in names {
        for b in n.as_bytes() {
            fnv(&mut hash, *b);
        }
    }
    // Avoid a zero seed for empty exports.
    if hash == 0 { 0x1234_5678_9abc_def0 } else { hash }
}

/// Format a 64-bit seed as a 32-hex-char UUID (seed repeated to fill 128 bits).
fn format_uuid(seed: u64) -> String {
    let hi = seed;
    let lo = seed.rotate_left(17) ^ 0xa5a5_a5a5_a5a5_a5a5;
    format!("{hi:016x}{lo:016x}")
}

/// Current date as `YYYY-MM-DD` (UTC). Uses only std.
fn today_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    // Days since epoch → civil date (Howard Hinnant's algorithm).
    let days = (secs / 86_400) as i64;
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}")
}

/// Validate USB filesystem requirements
pub fn validate_usb_target(path: &Path) -> anyhow::Result<()> {
    if !path.exists() {
        anyhow::bail!("Target path does not exist: {:?}", path);
    }
    
    if !path.is_dir() {
        anyhow::bail!("Target path is not a directory: {:?}", path);
    }
    
    // Try to create a test file
    let test_file = path.join(".rekordbox_test");
    match File::create(&test_file) {
        Ok(_) => {
            fs::remove_file(&test_file)?;
        }
        Err(e) => {
            anyhow::bail!("Cannot write to target directory: {}", e);
        }
    }
    
    Ok(())
}

/// Copy audio files to Contents directory with hierarchical structure
/// Creates both:
/// - Contents/filename.ext (flat, at root)
/// - Contents/Artist/Album/filename.ext (hierarchical by metadata)
fn copy_audio_files(
    tracks: &[TrackAnalysis],
    source_dir: &Path,
    contents_dir: &Path,
) -> anyhow::Result<()> {
    use std::collections::HashSet;
    
    // Track which files we've already copied to avoid duplicates
    let mut copied_files: HashSet<String> = HashSet::new();
    
    for track in tracks {
        // Extract filename from USB path
        let filename = Path::new(&track.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("");
        
        if filename.is_empty() {
            warn!("Track {} has no filename", track.id);
            continue;
        }
        
        // Find source file
        let mut source_path = None;
        for entry in WalkDir::new(source_dir)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if entry.file_name().to_str() == Some(filename) {
                source_path = Some(entry.path().to_path_buf());
                break;
            }
        }
        
        let source = match source_path {
            Some(p) => p,
            None => {
                warn!("Source file not found for track {}: {}", track.id, filename);
                continue;
            }
        };
        
        // 1. Copy to flat Contents/ directory (root level)
        let flat_dest = contents_dir.join(filename);
        if !flat_dest.exists() {
            fs::copy(&source, &flat_dest)?;
            debug!("Copied to flat: {:?} -> {:?}", source, flat_dest);
        }
        
        // 2. Copy to hierarchical Artist/Album/ structure
        let artist = sanitize_path_component(&track.artist);
        let album = track.album.as_ref()
            .map(|a| sanitize_path_component(a))
            .unwrap_or_else(|| "Unknown Album".to_string());
        
        if !artist.is_empty() {
            // Create artist directory
            let artist_dir = contents_dir.join(&artist);
            fs::create_dir_all(&artist_dir)?;
            
            // Create album directory inside artist
            let album_dir = artist_dir.join(&album);
            fs::create_dir_all(&album_dir)?;
            
            // Copy file to album directory
            let hier_dest = album_dir.join(filename);
            let hier_key = format!("{}/{}/{}", artist, album, filename);
            
            if !copied_files.contains(&hier_key) && !hier_dest.exists() {
                fs::copy(&source, &hier_dest)?;
                copied_files.insert(hier_key);
                debug!("Copied to hierarchy: {:?} -> {:?}", source, hier_dest);
            }
        }
    }
    
    Ok(())
}

/// Sanitize a string for use as a path component
/// Removes/replaces characters that are invalid in file/folder names
fn sanitize_path_component(name: &str) -> String {
    if name.is_empty() {
        return "Unknown".to_string();
    }
    
    // Replace invalid characters with underscores
    let sanitized: String = name
        .chars()
        .map(|c| {
            match c {
                '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
                '\0' => '_',
                _ => c,
            }
        })
        .collect();
    
    // Trim whitespace and dots from start/end
    let trimmed = sanitized.trim().trim_matches('.');
    
    if trimmed.is_empty() {
        "Unknown".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;
    
    #[test]
    fn test_validate_writable() {
        let tmp = TempDir::new().unwrap();
        assert!(validate_usb_target(tmp.path()).is_ok());
    }
    
    #[test]
    fn test_validate_nonexistent() {
        let result = validate_usb_target(Path::new("/nonexistent/path"));
        assert!(result.is_err());
    }
}