//! rekordbox XML (`DJ_PLAYLISTS`) export.
//!
//! This produces the *import* format that rekordbox PC reads via
//! "File > Import Collection in XML format". It is a text format and is
//! therefore immune to the binary-layout corruption issues that affect direct
//! `export.pdb` / ANLZ generation: when rekordbox imports this XML it builds
//! (and later re-exports) the PDB itself, so the PDB is always valid.
//!
//! Two intended uses:
//!   1. A "tier 1" export mode for users who own rekordbox PC.
//!   2. A golden-file harness: emit XML from the *same* [`TrackAnalysis`]
//!      structs the PDB/ANLZ writers consume, import into rekordbox, export a
//!      USB, then diff rekordbox's PDB/ANLZ against `rekord-export`'s direct
//!      output for the identical input.
//!
//! Format reference: Pioneer DJ "rekordbox xml format" (DJ_PLAYLISTS 1.0.0)
//! and the pyrekordbox documentation.
//!
//! IMPORTANT: `Location` is the only attribute rekordbox treats as essential,
//! and it must resolve to a real audio file **on the machine running
//! rekordbox**. See [`XmlExportOptions::music_root`].

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::Path;

use crate::track::{BeatGrid, CuePoint, CueType, FileType, TrackAnalysis};

/// Options controlling XML generation.
#[derive(Debug, Clone)]
pub struct XmlExportOptions {
    /// `PRODUCT@Name` — shown as the source name in rekordbox's tree.
    pub product_name: String,
    /// `PRODUCT@Version`.
    pub product_version: String,
    /// `PRODUCT@Company`.
    pub company: String,
    /// `TRACK@DateAdded` (`yyyy-mm-dd`). rekordbox accepts a fixed value.
    pub date_added: String,
    /// Emit `Red`/`Green`/`Blue` attributes on hot-cue `POSITION_MARK`s.
    pub include_cue_colors: bool,
    /// Base directory prepended to each track's relative `file_path` to build
    /// the absolute `Location` URI. If a track's `file_path` is already
    /// absolute it is used as-is. This must be a path that is valid on the
    /// computer that will import the XML into rekordbox.
    pub music_root: std::path::PathBuf,
}

impl Default for XmlExportOptions {
    fn default() -> Self {
        Self {
            product_name: "rekord-export".to_string(),
            product_version: "1.0.0".to_string(),
            company: "rekord-export".to_string(),
            date_added: "2025-01-01".to_string(),
            include_cue_colors: true,
            music_root: std::path::PathBuf::from("/"),
        }
    }
}

/// Generate a complete rekordbox XML document as a `String`.
///
/// * `tracks`    — the collection, emitted in slice order (deterministic).
/// * `playlists` — name -> track-id list. Emitted as flat playlists under
///   ROOT, sorted by name for deterministic output. Empty names are skipped.
pub fn generate_xml(
    tracks: &[TrackAnalysis],
    playlists: &HashMap<String, Vec<u32>>,
    opts: &XmlExportOptions,
) -> String {
    let mut out = String::with_capacity(4096 + tracks.len() * 512);

    out.push_str("<?xml version=\"1.0\" encoding=\"UTF-8\" ?>\n");
    let _ = writeln!(
        out,
        "<DJ_PLAYLISTS Version=\"1.0.0\">"
    );
    let _ = writeln!(
        out,
        "  <PRODUCT Name=\"{}\" Version=\"{}\" Company=\"{}\"/>",
        esc(&opts.product_name),
        esc(&opts.product_version),
        esc(&opts.company),
    );

    // ---- COLLECTION ------------------------------------------------------
    let _ = writeln!(out, "  <COLLECTION Entries=\"{}\">", tracks.len());
    for t in tracks {
        write_track(&mut out, t, opts);
    }
    out.push_str("  </COLLECTION>\n");

    // ---- PLAYLISTS -------------------------------------------------------
    // Deterministic order: sort by name, skip empties (mirrors export_usb).
    let mut names: Vec<&String> = playlists
        .keys()
        .filter(|n| !n.is_empty())
        .collect();
    names.sort();

    let _ = writeln!(
        out,
        "  <PLAYLISTS>\n    <NODE Type=\"0\" Name=\"ROOT\" Count=\"{}\">",
        names.len()
    );
    for name in names {
        let ids = &playlists[name];
        let _ = writeln!(
            out,
            "      <NODE Name=\"{}\" Type=\"1\" KeyType=\"0\" Entries=\"{}\">",
            esc(name),
            ids.len()
        );
        for id in ids {
            let _ = writeln!(out, "        <TRACK Key=\"{}\"/>", id);
        }
        out.push_str("      </NODE>\n");
    }
    out.push_str("    </NODE>\n  </PLAYLISTS>\n");

    out.push_str("</DJ_PLAYLISTS>\n");
    out
}

// ---------------------------------------------------------------------------
// TRACK
// ---------------------------------------------------------------------------

fn write_track(out: &mut String, t: &TrackAnalysis, opts: &XmlExportOptions) {
    let location = file_uri(&abs_path(&t.file_path, &opts.music_root));
    let kind = kind_string(t.file_type, &t.file_path);
    let tonality = t.key.map(|k| k.name()).unwrap_or_default();

    let has_children = !t.beat_grid.is_empty()
        || t.bpm > 0.0
        || !t.cue_points.is_empty();

    let _ = write!(
        out,
        "    <TRACK TrackID=\"{id}\" Name=\"{name}\" Artist=\"{artist}\" Composer=\"\" \
Album=\"{album}\" Grouping=\"\" Genre=\"{genre}\" Kind=\"{kind}\" Size=\"{size}\" \
TotalTime=\"{total}\" DiscNumber=\"0\" TrackNumber=\"{trackno}\" Year=\"{year}\" \
AverageBpm=\"{bpm:.2}\" DateAdded=\"{added}\" BitRate=\"{bitrate}\" \
SampleRate=\"{srate}\" Comments=\"{comments}\" PlayCount=\"0\" Rating=\"0\" \
Location=\"{location}\" Remixer=\"\" Tonality=\"{tonality}\" Label=\"{label}\" Mix=\"\"",
        id = t.id,
        name = esc(&t.title),
        artist = esc(&t.artist),
        album = esc(t.album.as_deref().unwrap_or("")),
        genre = esc(t.genre.as_deref().unwrap_or("")),
        kind = esc(&kind),
        size = t.file_size,
        total = t.duration_secs.round() as u64,
        trackno = t.track_number.unwrap_or(0),
        year = t.year.unwrap_or(0),
        bpm = t.bpm,
        added = esc(&opts.date_added),
        bitrate = t.bitrate,
        srate = t.sample_rate,
        comments = esc(t.comment.as_deref().unwrap_or("")),
        location = location, // already percent-encoded ASCII; no XML-special chars
        tonality = esc(&tonality),
        label = esc(t.label.as_deref().unwrap_or("")),
    );

    if !has_children {
        out.push_str("/>\n");
        return;
    }

    out.push_str(">\n");
    write_tempos(out, &t.beat_grid, t.bpm);
    for cue in &t.cue_points {
        write_position_mark(out, cue, opts.include_cue_colors);
    }
    out.push_str("    </TRACK>\n");
}

/// Emit `<TEMPO>` markers. The grid model is piecewise-constant (each [`Beat`]
/// carries its own tempo), so we emit a marker at the first beat and wherever
/// the tempo changes — exactly reconstructing the grid. For a constant-tempo
/// grid this yields a single TEMPO, which is how rekordbox stores static grids.
fn write_tempos(out: &mut String, grid: &BeatGrid, fallback_bpm: f64) {
    if grid.beats.is_empty() {
        if fallback_bpm > 0.0 {
            let _ = writeln!(
                out,
                "      <TEMPO Inizio=\"{:.3}\" Bpm=\"{:.2}\" Metro=\"4/4\" Battito=\"1\"/>",
                grid.first_beat_ms / 1000.0,
                fallback_bpm,
            );
        }
        return;
    }

    let mut prev_tempo: Option<u16> = None;
    for beat in &grid.beats {
        if prev_tempo != Some(beat.tempo_100) {
            let _ = writeln!(
                out,
                "      <TEMPO Inizio=\"{:.3}\" Bpm=\"{:.2}\" Metro=\"4/4\" Battito=\"{}\"/>",
                beat.time_ms / 1000.0,
                beat.tempo_100 as f64 / 100.0,
                beat.beat_number.clamp(1, 4),
            );
            prev_tempo = Some(beat.tempo_100);
        }
    }
}

/// Emit a single `<POSITION_MARK>`.
///
/// `Num`: memory cue = -1, hot cue A/B/C... = 0/1/2... (`hot_cue - 1`).
/// `Type`: rekordbox numbering (Cue=0, Fade-In=1, Fade-Out=2, Load=3, Loop=4),
/// which differs from this crate's [`CueType`] (which starts at 1).
fn write_position_mark(out: &mut String, cue: &CuePoint, include_colors: bool) {
    let is_loop = matches!(cue.cue_type, CueType::Loop) || cue.loop_ms > 0.0;
    let rb_type = if is_loop { 4 } else { cue_type_to_rb(cue.cue_type) };
    let num: i32 = if cue.hot_cue == 0 {
        -1
    } else {
        cue.hot_cue as i32 - 1
    };

    let name = esc(cue.comment.as_deref().unwrap_or(""));
    let start = cue.time_ms / 1000.0;

    let _ = write!(
        out,
        "      <POSITION_MARK Name=\"{name}\" Type=\"{ty}\" Start=\"{start:.3}\"",
        name = name,
        ty = rb_type,
        start = start,
    );

    if is_loop {
        let end = (cue.time_ms + cue.loop_ms) / 1000.0;
        let _ = write!(out, " End=\"{:.3}\"", end);
    }

    let _ = write!(out, " Num=\"{}\"", num);

    if include_colors && cue.hot_cue != 0 {
        if let Some(c) = cue.color {
            let _ = write!(
                out,
                " Red=\"{}\" Green=\"{}\" Blue=\"{}\"",
                c.red, c.green, c.blue
            );
        }
    }

    out.push_str("/>\n");
}

fn cue_type_to_rb(t: CueType) -> u8 {
    match t {
        CueType::Cue => 0,
        CueType::FadeIn => 1,
        CueType::FadeOut => 2,
        CueType::Load => 3,
        CueType::Loop => 4,
    }
}

fn kind_string(ft: FileType, file_path: &str) -> String {
    match ft {
        FileType::Mp3 => "MP3 File".to_string(),
        FileType::M4a => "M4A File".to_string(),
        FileType::Flac => "FLAC File".to_string(),
        FileType::Wav => "WAV File".to_string(),
        FileType::Aiff => "AIFF File".to_string(),
        FileType::Unknown => {
            let ext = Path::new(file_path)
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_uppercase())
                .unwrap_or_default();
            if ext.is_empty() {
                String::new()
            } else {
                format!("{} File", ext)
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Path / URI helpers
// ---------------------------------------------------------------------------

fn abs_path(file_path: &str, music_root: &Path) -> String {
    let p = Path::new(file_path);
    if p.is_absolute() {
        file_path.to_string()
    } else {
        music_root.join(p).to_string_lossy().into_owned()
    }
}

/// Build a `file://localhost/...` URI with rekordbox-style percent-encoding.
/// Forward slashes and the Windows drive colon are kept literal; everything
/// else outside the URI unreserved set is percent-encoded (e.g. space -> %20).
fn file_uri(abs: &str) -> String {
    // Normalise Windows backslashes to forward slashes.
    let normalised = abs.replace('\\', "/");
    let encoded = percent_encode_path(&normalised);
    if encoded.starts_with('/') {
        // Unix absolute path: file://localhost + /Users/...
        format!("file://localhost{}", encoded)
    } else {
        // Windows path like C:/...: file://localhost/ + C:/...
        format!("file://localhost/{}", encoded)
    }
}

fn percent_encode_path(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        let keep = b.is_ascii_alphanumeric()
            || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':');
        if keep {
            out.push(b as char);
        } else {
            let _ = write!(out, "%{:02X}", b);
        }
    }
    out
}

/// XML attribute-value escaping.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::Key;

    fn sample_track() -> TrackAnalysis {
        let mut grid = BeatGrid::constant_tempo(128.0, 100.0, 4000.0);
        // Force a tempo change partway to exercise multi-TEMPO emission.
        if grid.beats.len() > 4 {
            for b in grid.beats.iter_mut().skip(4) {
                b.tempo_100 = 13000;
            }
        }
        TrackAnalysis {
            id: 1,
            file_path: "House/Track One.mp3".to_string(),
            title: "Track \"One\" & Two".to_string(),
            artist: "DJ <Test>".to_string(),
            album: Some("Album".to_string()),
            genre: Some("House".to_string()),
            label: Some("Label".to_string()),
            duration_secs: 215.7,
            sample_rate: 44100,
            bit_depth: 16,
            bitrate: 320,
            bpm: 128.0,
            key: Some(Key::new(9, false)), // Am
            beat_grid: grid,
            waveform: Default::default(),
            cue_points: vec![
                CuePoint {
                    hot_cue: 0,
                    cue_type: CueType::Cue,
                    time_ms: 100.0,
                    loop_ms: 0.0,
                    comment: None,
                    color: None,
                },
                CuePoint {
                    hot_cue: 1,
                    cue_type: CueType::Cue,
                    time_ms: 2000.0,
                    loop_ms: 0.0,
                    comment: Some("Drop".to_string()),
                    color: Some(crate::track::HotCueColor::RED),
                },
                CuePoint {
                    hot_cue: 0,
                    cue_type: CueType::Loop,
                    time_ms: 8000.0,
                    loop_ms: 1875.0,
                    comment: None,
                    color: None,
                },
            ],
            file_size: 8_650_000,
            file_hash: 0,
            year: Some(2023),
            comment: None,
            track_number: Some(3),
            file_type: FileType::Mp3,
        }
    }

    #[test]
    fn well_formed_and_mapped() {
        let mut pls = HashMap::new();
        pls.insert("My Set".to_string(), vec![1]);
        pls.insert(String::new(), vec![1]); // should be skipped
        let opts = XmlExportOptions {
            music_root: std::path::PathBuf::from("/Users/dj/Music"),
            ..Default::default()
        };
        let xml = generate_xml(&[sample_track()], &pls, &opts);

        assert!(xml.starts_with("<?xml"));
        assert!(xml.contains("<DJ_PLAYLISTS Version=\"1.0.0\">"));
        assert!(xml.contains("Entries=\"1\""));
        // XML escaping in attributes.
        assert!(xml.contains("Name=\"Track &quot;One&quot; &amp; Two\""));
        assert!(xml.contains("Artist=\"DJ &lt;Test&gt;\""));
        // Location URI: percent-encoded space, file://localhost prefix.
        assert!(xml.contains("Location=\"file://localhost/Users/dj/Music/House/Track%20One.mp3\""));
        assert!(xml.contains("Kind=\"MP3 File\""));
        assert!(xml.contains("Tonality=\"Am\""));
        // Two TEMPO markers (tempo changes once).
        assert_eq!(xml.matches("<TEMPO ").count(), 2);
        assert!(xml.contains("Bpm=\"128.00\""));
        assert!(xml.contains("Bpm=\"130.00\""));
        // Memory cue -> Num=-1, Type=0.
        assert!(xml.contains("Type=\"0\" Start=\"0.100\" Num=\"-1\""));
        // Hot cue A -> Num=0 with color.
        assert!(xml.contains("Num=\"0\" Red=\"230\" Green=\"40\" Blue=\"40\""));
        // Loop -> Type=4 with End.
        assert!(xml.contains("Type=\"4\" Start=\"8.000\" End=\"9.875\" Num=\"-1\""));
        // Playlist node (empty name skipped -> Count=1).
        assert!(xml.contains("Name=\"ROOT\" Count=\"1\""));
        assert!(xml.contains("<NODE Name=\"My Set\" Type=\"1\" KeyType=\"0\" Entries=\"1\">"));
        assert!(xml.contains("<TRACK Key=\"1\"/>"));
    }
}