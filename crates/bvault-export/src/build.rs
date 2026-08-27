//! The build orchestration: a faithful port of the legacy `export_usb` /
//! `write_device_library`, changed only where the new architecture demands it —
//! it *reads* stored analyses instead of analyzing, streams audio instead of
//! copying it, and emits a manifest. Every format byte is still produced by
//! bvault-core; nothing here reimplements a transformation.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use bvault_core::{
    build_devlib_backup_json, build_export_library, devlib_backup_filename, generate_2ex_file,
    generate_anlz_path, generate_dat_file, generate_devsetting, generate_djprofile,
    generate_ext_file, DeviceLibraryOptions, PdbBuilder, PlaylistSpec, TrackAnalysis,
};
use bvault_store::{ArtifactStore, RawStore};

use crate::manifest::{ExportInput, Manifest, ManifestEntry, PlaylistInput, Source};
use crate::{Error, Result};

/// Render the export's brain into `staging` and return the manifest of the full
/// USB tree. Audio is not copied — audio entries point back into the raw store.
pub fn build_export(
    input: &ExportInput<'_>,
    artifacts: &ArtifactStore,
    raw: &RawStore,
    staging: &Path,
) -> Result<Manifest> {
    let mut entries: Vec<ManifestEntry> = Vec::new();

    // 1. Load each analysis, assign a 1-based id and its Contents path. The id
    //    ties together the PDB row, the ANLZ path, and the device-library row;
    //    the Contents path is what ANLZ's PPTH and the PDB bake in.
    let mut analyses: Vec<TrackAnalysis> = Vec::with_capacity(input.tracks.len());
    let mut refs: Vec<TrackRef> = Vec::with_capacity(input.tracks.len());
    let mut id_by_hash: HashMap<&str, u32> = HashMap::new();

    for (i, (hash, raw_location)) in input.tracks.iter().enumerate() {
        let id = (i as u32) + 1;
        let bytes = artifacts.get(hash, "analysis.json")?;
        let mut analysis: TrackAnalysis = serde_json::from_slice(&bytes)
            .map_err(|e| Error::Decode(format!("{hash}/analysis.json: {e}")))?;

        let ext = Path::new(raw_location)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("bin");
        let contents_usb = format!("Contents/{hash}.{ext}");

        analysis.id = id;
        analysis.file_path = format!("/{contents_usb}"); // leading slash: PDB/ANLZ path form
        id_by_hash.insert(hash.as_str(), id);
        analyses.push(analysis);
        refs.push(TrackRef {
            hash: hash.clone(),
            raw_location: raw_location.clone(),
            contents_usb,
        });
    }

    // 2. Playlists in a stable (name-sorted) order; ids 1-based, shared verbatim
    //    by the PDB and the device library so the two agree.
    let mut playlists: Vec<&PlaylistInput> = input
        .playlists
        .iter()
        .filter(|p| !p.name.is_empty())
        .collect();
    playlists.sort_by(|a, b| a.name.cmp(&b.name));
    let content_ids: Vec<Vec<u32>> = playlists
        .iter()
        .map(|p| {
            p.hashes
                .iter()
                .filter_map(|h| id_by_hash.get(h.as_str()).copied())
                .collect()
        })
        .collect();

    // 3. export.pdb
    let mut pdb = PdbBuilder::new();
    for a in &analyses {
        pdb.add_track(a, &generate_anlz_path(a.id));
    }
    for (i, p) in playlists.iter().enumerate() {
        pdb.add_playlist((i as u32) + 1, 0, &p.name, content_ids[i].clone());
    }
    let pdb_bytes = pdb.build()?;
    write_staged(
        staging,
        "PIONEER/rekordbox/export.pdb",
        &pdb_bytes,
        &mut entries,
    )?;

    // 4. Device Library Plus: encrypted exportLibrary.db + its backup json.
    write_device_library(&analyses, &playlists, &content_ids, staging, &mut entries)?;

    // 5. Aux files.
    write_staged(
        staging,
        "PIONEER/DEVSETTING.DAT",
        &generate_devsetting(),
        &mut entries,
    )?;
    write_staged(
        staging,
        "PIONEER/djprofile.nxs",
        &generate_djprofile(input.profile_name),
        &mut entries,
    )?;

    // 6. ANLZ per track (.DAT/.EXT/.2EX), each embedding the track's Contents path.
    for a in &analyses {
        let dat_rel = generate_anlz_path(a.id);
        let stem = dat_rel.strip_suffix(".DAT").unwrap_or(&dat_rel);
        let total_samples = (a.duration_secs * a.sample_rate.max(1) as f64).round() as u64;

        let dat = generate_dat_file(
            &a.beat_grid,
            &a.waveform,
            &a.file_path,
            a.file_size,
            total_samples,
        )?;
        write_staged(staging, &dat_rel, &dat, &mut entries)?;

        let ext = generate_ext_file(&a.beat_grid, &a.waveform, &a.file_path, &a.cue_points)?;
        write_staged(staging, &format!("{stem}.EXT"), &ext, &mut entries)?;

        let two = generate_2ex_file(&a.beat_grid, &a.waveform, &a.file_path, &a.cue_points)?;
        write_staged(staging, &format!("{stem}.2EX"), &two, &mut entries)?;
    }

    // 7. Audio: not copied. Each track is a manifest entry pointing at the raw
    //    store by hash; the transfer streams it straight to the USB.
    for r in &refs {
        let size = raw.resolve(&r.raw_location)?.metadata()?.len();
        entries.push(ManifestEntry {
            usb_path: r.contents_usb.clone(),
            size,
            source: Source::Raw {
                hash: r.hash.clone(),
            },
        });
    }

    Ok(Manifest { entries })
}

struct TrackRef {
    hash: String,
    raw_location: String,
    contents_usb: String,
}

/// Port of the legacy `write_device_library`: deterministic device identity
/// derived from the export's contents, then core builds the encrypted db.
fn write_device_library(
    analyses: &[TrackAnalysis],
    playlists: &[&PlaylistInput],
    content_ids: &[Vec<u32>],
    staging: &Path,
    entries: &mut Vec<ManifestEntry>,
) -> Result<()> {
    let seed = device_seed(analyses, playlists);
    let opts = DeviceLibraryOptions {
        master_db_id: (seed & 0x7fff_ffff) as i64, // 31-bit positive, matches golden magnitude
        uuid: format_uuid(seed),
        created_date: today_iso(),
        device_name: String::new(),
    };

    let specs: Vec<PlaylistSpec> = playlists
        .iter()
        .enumerate()
        .map(|(i, p)| PlaylistSpec {
            id: (i as i64) + 1,
            name: p.name.as_str(),
            content_ids: &content_ids[i],
        })
        .collect();

    let db_rel = "PIONEER/rekordbox/exportLibrary.db";
    let db_path = staging.join(db_rel);
    if let Some(parent) = db_path.parent() {
        fs::create_dir_all(parent)?;
    }
    build_export_library(&db_path, analyses, &specs, &opts)?;
    let db_size = fs::metadata(&db_path)?.len();
    entries.push(ManifestEntry {
        usb_path: db_rel.to_string(),
        size: db_size,
        source: Source::Staging,
    });

    let json = build_devlib_backup_json(&opts)?;
    let backup_rel = format!("PIONEER/DeviceLibBackup/{}", devlib_backup_filename(&opts));
    write_staged(staging, &backup_rel, json.as_bytes(), entries)?;
    Ok(())
}

/// Write a rendered file into staging at a USB-relative path and record it as a
/// `Staging` manifest entry. Forward slashes in `usb_path` are fine on Windows —
/// `Path::join` treats them as separators.
fn write_staged(
    staging: &Path,
    usb_path: &str,
    bytes: &[u8],
    entries: &mut Vec<ManifestEntry>,
) -> Result<()> {
    let full = staging.join(usb_path);
    if let Some(parent) = full.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(&full, bytes)?;
    entries.push(ManifestEntry {
        usb_path: usb_path.to_string(),
        size: bytes.len() as u64,
        source: Source::Staging,
    });
    Ok(())
}

// --- deterministic device identity (verbatim port; std only) ----------------

/// Stable 64-bit seed from the export's track ids and sorted playlist names, so
/// the same library reproduces the same device identity across runs.
fn device_seed(analyses: &[TrackAnalysis], playlists: &[&PlaylistInput]) -> u64 {
    fn fnv(h: &mut u64, byte: u8) {
        *h ^= byte as u64;
        *h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for a in analyses {
        for b in a.id.to_le_bytes() {
            fnv(&mut hash, b);
        }
    }
    let mut names: Vec<&str> = playlists.iter().map(|p| p.name.as_str()).collect();
    names.sort();
    for n in names {
        for b in n.as_bytes() {
            fnv(&mut hash, *b);
        }
    }
    if hash == 0 {
        0x1234_5678_9abc_def0
    } else {
        hash
    }
}

/// Format a 64-bit seed as 32 hex chars (128 bits, seed folded into the low half).
fn format_uuid(seed: u64) -> String {
    let hi = seed;
    let lo = seed.rotate_left(17) ^ 0xa5a5_a5a5_a5a5_a5a5;
    format!("{hi:016x}{lo:016x}")
}

/// Current UTC date as `YYYY-MM-DD` (Howard Hinnant's civil-date algorithm).
fn today_iso() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
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
