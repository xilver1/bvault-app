//! ANLZ file generation (.DAT, .EXT, .2EX)
//!
//! ANLZ files are **big-endian** and contain tagged sections:
//! - PMAI: File header
//! - PQTZ: Beat grid
//! - PWAV: Preview waveform (monochrome)
//! - PWV5: Detail waveform (color)
//! - PPTH: File path
//!
//! Reference: https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/anlz.html

use crate::error::Result;
use crate::track::{
    BeatGrid, CuePoint, CueType, HotCueColor, Waveform, WaveformColorPreview, WaveformDetail,
    WaveformPreview,
};

/// Section tags (4 bytes each)
const PMAI_TAG: &[u8; 4] = b"PMAI";
const PQTZ_TAG: &[u8; 4] = b"PQTZ";
const PWAV_TAG: &[u8; 4] = b"PWAV";
const PWV2_TAG: &[u8; 4] = b"PWV2"; // 100-column preview overview (deck strip)
const PWV3_TAG: &[u8; 4] = b"PWV3"; // 3-band waveform for NXS compatibility
const PWV4_TAG: &[u8; 4] = b"PWV4"; // Color preview waveform (1200×6 bytes)
const PWV5_TAG: &[u8; 4] = b"PWV5";
const PWV6_TAG: &[u8; 4] = b"PWV6"; // 3-band preview (.2EX)
const PWV7_TAG: &[u8; 4] = b"PWV7"; // 3-band scrolling detail (.2EX)
const PWVC_TAG: &[u8; 4] = b"PWVC"; // fixed 20-byte tag (.2EX)
const PPTH_TAG: &[u8; 4] = b"PPTH";
const PVBR_TAG: &[u8; 4] = b"PVBR"; // VBR seek index (.DAT)

/// Generate PVBR (VBR seek index). Golden layout, constant size 1620 bytes:
///   fourcc(4) len_header(4)=16 len_tag(4)=1620 unknown(4)=0
///   then 400 x u32 monotonic byte offsets into the audio file,
///   then 1 x u32 = total PCM sample count.
///
/// rekordbox writes this as the SECOND tag of every .DAT (right after PPTH).
/// Omitting it appears to make rekordbox abandon the rest of the .DAT.
fn generate_pvbr_section(file_size_bytes: u64, total_samples: u64) -> Vec<u8> {
    let mut buffer = Vec::with_capacity(1620);
    buffer.extend_from_slice(PVBR_TAG);
    buffer.extend_from_slice(&16u32.to_be_bytes()); // len_header
    buffer.extend_from_slice(&1620u32.to_be_bytes()); // len_tag (fixed)
    buffer.extend_from_slice(&0u32.to_be_bytes()); // unknown

    // 400 seek points spread across the file. For CBR this is exact; for VBR it
    // is a linear approximation (seek accuracy degrades, nothing breaks).
    for i in 0..400u64 {
        let off = (file_size_bytes.saturating_mul(i) / 400) as u32;
        buffer.extend_from_slice(&off.to_be_bytes());
    }
    // Final entry: total decoded sample count.
    buffer.extend_from_slice(&(total_samples as u32).to_be_bytes());

    debug_assert_eq!(buffer.len(), 1620);
    buffer
}

/// Write the 28-byte PMAI file header.
/// Bytes 12..28 are NOT zero in real rekordbox files - they are constant across
/// every golden .DAT/.EXT/.2EX:
///   00 00 00 01 | 00 01 00 00 | 00 01 00 00 | 00 00 00 00
/// These were previously written as zeros.
fn write_pmai_header(buffer: &mut Vec<u8>, total_size: usize) {
    buffer.extend_from_slice(PMAI_TAG);
    buffer.extend_from_slice(&28u32.to_be_bytes()); // len_header (offset of first tag)
    buffer.extend_from_slice(&(total_size as u32).to_be_bytes()); // len_file
    buffer.extend_from_slice(&0x0000_0001u32.to_be_bytes());
    buffer.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    buffer.extend_from_slice(&0x0001_0000u32.to_be_bytes());
    buffer.extend_from_slice(&0x0000_0000u32.to_be_bytes());
}
const PCOB_TAG: &[u8; 4] = b"PCOB"; // Cue/loop points (basic)
const PCO2_TAG: &[u8; 4] = b"PCO2"; // Extended cue points with colors (Nexus 2+)

/// Generate a complete ANLZ .DAT file
pub fn generate_dat_file(
    beat_grid: &BeatGrid,
    waveform: &Waveform,
    file_path: &str,
    audio_file_size: u64,
    total_samples: u64,
) -> Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(64 * 1024);

    // Golden .DAT tag set and ORDER: PPTH PVBR PQTZ PWAV PWV2 PCOB PCOB
    // PWV5 was previously written here; rekordbox keeps the color detail
    // waveform in the .EXT only. Emitting it in .DAT is not what rekordbox does.
    let ppth_section = generate_ppth_section(file_path);
    let pvbr_section = generate_pvbr_section(audio_file_size, total_samples);
    let pqtz_section = generate_pqtz_section(beat_grid);
    let pwav_section = generate_pwav_section(&waveform.preview);
    let pwv2_section = generate_pwv2_section(&waveform.preview);
    let pcob_hot = generate_pcob_empty(1);
    let pcob_mem = generate_pcob_empty(0);

    // Calculate total file size
    let sections_size = ppth_section.len()
        + pvbr_section.len()
        + pqtz_section.len()
        + pwav_section.len()
        + pwv2_section.len()
        + pcob_hot.len()
        + pcob_mem.len();
    let header_size = 28; // PMAI header
    let total_size = header_size + sections_size;

    write_pmai_header(&mut buffer, total_size);

    // Write sections in golden order
    buffer.extend_from_slice(&ppth_section);
    buffer.extend_from_slice(&pvbr_section);
    buffer.extend_from_slice(&pqtz_section);
    buffer.extend_from_slice(&pwav_section);
    buffer.extend_from_slice(&pwv2_section);
    buffer.extend_from_slice(&pcob_hot);
    buffer.extend_from_slice(&pcob_mem);

    Ok(buffer)
}

/// Generate PWV2 (100-column preview waveform) — the low-res overview strip
/// rekordbox draws above the main waveform. Golden layout:
///   fourcc | len_header=20 | len_tag=120 | len_entries=100 | 0x00010000 | 100 data bytes
/// Data is the 400-column PWAV downsampled to 100 by taking the max of each
/// group of 4 columns (preserves peaks so the overview looks right).
fn generate_pwv2_section(preview: &WaveformPreview) -> Vec<u8> {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(PWV2_TAG);
    let header_len = 20u32;
    let section_len = 20u32 + 100;
    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&section_len.to_be_bytes());
    buffer.extend_from_slice(&100u32.to_be_bytes()); // entry count
    buffer.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // constant in golden

    // Downsample 400 -> 100: max byte over each group of 4 source columns.
    for i in 0..100 {
        let mut peak = 0u8;
        for j in 0..4 {
            let idx = i * 4 + j;
            if idx < preview.columns.len() {
                let v = preview.columns[idx].to_byte();
                if v > peak {
                    peak = v;
                }
            }
        }
        buffer.push(peak);
    }
    buffer
}

/// Generate PQTZ (beat grid) section
fn generate_pqtz_section(beat_grid: &BeatGrid) -> Vec<u8> {
    let mut buffer = Vec::new();

    // Tag
    buffer.extend_from_slice(PQTZ_TAG);

    // Calculate section size
    // Header: fourcc(4) + len_header(4) + len_tag(4) + unk1(4) + unk2(4) + len_beats(4) = 24 bytes
    // Each beat: 8 bytes (beat_number u16, tempo u16, time_ms u32)
    // len_header is the header SIZE (24), NOT "length after tag". Golden stores 0x18.
    let header_len = 24u32;
    let beat_data_len = beat_grid.beats.len() * 8;
    let section_len = 24 + beat_data_len;

    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&(section_len as u32).to_be_bytes());

    // unk1 = 0, unk2 = 0x00080000 (constant in golden; marks a valid beat grid)
    buffer.extend_from_slice(&0u32.to_be_bytes());
    buffer.extend_from_slice(&0x0008_0000u32.to_be_bytes());

    // Beat count
    buffer.extend_from_slice(&(beat_grid.beats.len() as u32).to_be_bytes());

    // Write beat entries
    for beat in &beat_grid.beats {
        // Beat number (1-4) as u16
        buffer.extend_from_slice(&(beat.beat_number as u16).to_be_bytes());
        // Tempo as BPM × 100
        buffer.extend_from_slice(&beat.tempo_100.to_be_bytes());
        // Time in milliseconds as u32
        buffer.extend_from_slice(&(beat.time_ms as u32).to_be_bytes());
    }

    buffer
}

/// Generate PWAV (preview waveform) section - exactly 400 bytes of waveform data
fn generate_pwav_section(preview: &WaveformPreview) -> Vec<u8> {
    let mut buffer = Vec::new();

    // Tag
    buffer.extend_from_slice(PWAV_TAG);

    // Header structure
    // fourcc(4) + len_header(4) + len_tag(4) + len_entries(4) + unk(4) = 20 bytes
    // len_header is header SIZE (20). field4 = 0x00010000 in golden.
    let header_len = 20u32;
    let section_len = 20u32 + 400; // Header + 400 bytes waveform

    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&(section_len).to_be_bytes());

    // Entry count (400)
    buffer.extend_from_slice(&400u32.to_be_bytes());

    // unk = 0x00010000 (constant in golden)
    buffer.extend_from_slice(&0x0001_0000u32.to_be_bytes());

    // Waveform data - exactly 400 bytes
    for i in 0..400 {
        if i < preview.columns.len() {
            buffer.push(preview.columns[i].to_byte());
        } else {
            buffer.push(0);
        }
    }

    buffer
}

/// Generate PWV5 (detail color waveform) section
fn generate_pwv5_section(detail: &WaveformDetail) -> Vec<u8> {
    let mut buffer = Vec::new();

    // Tag
    buffer.extend_from_slice(PWV5_TAG);

    // Header (24 bytes / 0x18) per DeepSymmetry spec:
    //   fourcc(4) | len_header(4) | len_tag(4) | len_entry_bytes(4) | len_entries(4) | unknown(4)
    // then data. len_entry_bytes is ALWAYS 2 for PWV5. The previous code omitted
    // len_entry_bytes entirely (wrote a 20-byte header), which shifted the count
    // into the entry_bytes slot and made rekordbox read garbage -> hang.
    let header_len = 24u32;
    let len_entry_bytes = 2u32;
    let len_entries = detail.entries.len() as u32;
    let data_size = detail.entries.len() * 2; // 2 bytes per entry
    let section_len = 24 + data_size;

    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&(section_len as u32).to_be_bytes());
    buffer.extend_from_slice(&len_entry_bytes.to_be_bytes());
    buffer.extend_from_slice(&len_entries.to_be_bytes());

    // Unknown constant (bytes 14-17); spec: "may always have the value 00960305"
    buffer.extend_from_slice(&0x0096_0305u32.to_be_bytes());

    // Waveform entries (2 bytes each, big-endian)
    for entry in &detail.entries {
        buffer.extend_from_slice(&entry.to_bytes());
    }

    buffer
}

/// Generate PPTH (file path) section.
/// Golden layout: len_header=16, len_tag=16+len_path, len_path = BYTE length of
/// the NUL-terminated UTF-16BE path (the terminator is included in the count).
fn generate_ppth_section(file_path: &str) -> Vec<u8> {
    let mut buffer = Vec::new();

    // Tag
    buffer.extend_from_slice(PPTH_TAG);

    // Encode path as UTF-16BE, NUL-terminated (golden includes the terminator)
    let mut path_utf16: Vec<u16> = file_path.encode_utf16().collect();
    path_utf16.push(0);
    let path_bytes_len = path_utf16.len() * 2;

    let header_len = 16u32;
    let section_len = 16 + path_bytes_len;

    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&(section_len as u32).to_be_bytes());

    // Path length in BYTES, including the UTF-16 NUL terminator.
    // (Previously wrote the character count and omitted the terminator.)
    buffer.extend_from_slice(&(path_bytes_len as u32).to_be_bytes());

    // Path data (UTF-16BE)
    for ch in path_utf16 {
        buffer.extend_from_slice(&ch.to_be_bytes());
    }

    buffer
}

/// Generate PWV3 (monochrome scrolling detail waveform) for the .EXT file.
/// Golden layout (verified): len_header=24, len_entry_bytes=1, len_entries=N,
/// unknown=0x00960000, then N bytes. Each byte is `whiteness<<5 | height`,
/// the same encoding as PWAV. Entry count equals the PWV5 detail entry count.
fn generate_pwv3_section(detail: &WaveformDetail) -> Vec<u8> {
    let mut buffer = Vec::new();

    buffer.extend_from_slice(PWV3_TAG);

    let len_entry_bytes = 1u32;
    let len_entries = detail.entries.len() as u32;
    let section_len = 24 + detail.entries.len();

    buffer.extend_from_slice(&24u32.to_be_bytes()); // len_header
    buffer.extend_from_slice(&(section_len as u32).to_be_bytes());
    buffer.extend_from_slice(&len_entry_bytes.to_be_bytes());
    buffer.extend_from_slice(&len_entries.to_be_bytes());
    buffer.extend_from_slice(&0x0096_0000u32.to_be_bytes());

    for entry in &detail.entries {
        let whiteness =
            (((entry.red as u16 + entry.green as u16 + entry.blue as u16) / 3) as u8).min(7);
        let height = entry.height & 0x1F;
        buffer.push((whiteness << 5) | height);
    }

    buffer
}

/// Generate PWV6 (3-band preview, 1200 columns) for the .2EX file.
/// Golden: len_header=20, len_entry_bytes=3, len_entries=1200, no magic field.
fn generate_pwv6_section(color_preview: &WaveformColorPreview) -> Vec<u8> {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(PWV6_TAG);

    let data_size = 1200 * 3;
    buffer.extend_from_slice(&20u32.to_be_bytes()); // len_header
    buffer.extend_from_slice(&((20 + data_size) as u32).to_be_bytes());
    buffer.extend_from_slice(&3u32.to_be_bytes()); // len_entry_bytes
    buffer.extend_from_slice(&1200u32.to_be_bytes()); // len_entries

    for i in 0..1200 {
        if i < color_preview.columns.len() {
            let c = &color_preview.columns[i];
            // low / mid / high bands
            buffer.push(c.blue & 0x7F);
            buffer.push(c.red & 0x7F);
            buffer.push(c.green & 0x7F);
        } else {
            buffer.extend_from_slice(&[0u8; 3]);
        }
    }
    buffer
}

/// Generate PWV7 (3-band scrolling detail) for the .2EX file.
/// Golden: len_header=24, len_entry_bytes=3, len_entries=N, unknown=0x00960000.
fn generate_pwv7_section(detail: &WaveformDetail) -> Vec<u8> {
    let mut buffer = Vec::new();
    buffer.extend_from_slice(PWV7_TAG);

    let len_entries = detail.entries.len() as u32;
    let data_size = detail.entries.len() * 3;

    buffer.extend_from_slice(&24u32.to_be_bytes());
    buffer.extend_from_slice(&((24 + data_size) as u32).to_be_bytes());
    buffer.extend_from_slice(&3u32.to_be_bytes()); // len_entry_bytes
    buffer.extend_from_slice(&len_entries.to_be_bytes());
    buffer.extend_from_slice(&0x0096_0000u32.to_be_bytes());

    for e in &detail.entries {
        // Scale each 3-bit band by the 5-bit height into the observed 0..~127 range.
        let scale = |ch: u8| -> u8 {
            ((ch as u32 & 0x07) * (e.height as u32 & 0x1F) * 127 / (7 * 31)).min(127) as u8
        };
        buffer.push(scale(e.red)); // low
        buffer.push(scale(e.green)); // mid
        buffer.push(scale(e.blue)); // high
    }
    buffer
}

/// Generate PWVC tag (.2EX). Golden is a 20-byte tag with len_header=14.
/// Payload after len_tag is: u16 zero, then three u16 band gains (low/mid/high).
/// Golden values vary per track (e.g. 0x50/0xd0/0x150); one golden file uses
/// 100/100/100, which is neutral scaling and the safe default here.
fn generate_pwvc_section() -> Vec<u8> {
    let mut buffer = Vec::with_capacity(20);
    buffer.extend_from_slice(PWVC_TAG);
    buffer.extend_from_slice(&14u32.to_be_bytes()); // len_header
    buffer.extend_from_slice(&20u32.to_be_bytes()); // len_tag
    buffer.extend_from_slice(&0u16.to_be_bytes()); // padding to len_header
    buffer.extend_from_slice(&100u16.to_be_bytes()); // low band gain
    buffer.extend_from_slice(&100u16.to_be_bytes()); // mid band gain
    buffer.extend_from_slice(&100u16.to_be_bytes()); // high band gain
    buffer
}

/// Generate an EMPTY PCOB tag (24 bytes) exactly as rekordbox does.
/// Golden: len_header=24, len_tag=24, type, unk=0, len_cues=0, memory_count=0xffffffff.
/// rekordbox always writes two of these (type 1 then type 0), even with no cues.
fn generate_pcob_empty(cue_type: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(24);
    b.extend_from_slice(PCOB_TAG);
    b.extend_from_slice(&24u32.to_be_bytes());
    b.extend_from_slice(&24u32.to_be_bytes());
    b.extend_from_slice(&cue_type.to_be_bytes());
    b.extend_from_slice(&0u16.to_be_bytes()); // unknown
    b.extend_from_slice(&0u16.to_be_bytes()); // len_cues
    b.extend_from_slice(&0xffff_ffffu32.to_be_bytes()); // memory_count
    b
}

/// Generate an EMPTY PCO2 tag (20 bytes). Golden: len_header=20, len_tag=20.
fn generate_pco2_empty(cue_type: u32) -> Vec<u8> {
    let mut b = Vec::with_capacity(20);
    b.extend_from_slice(PCO2_TAG);
    b.extend_from_slice(&20u32.to_be_bytes());
    b.extend_from_slice(&20u32.to_be_bytes());
    b.extend_from_slice(&cue_type.to_be_bytes());
    b.extend_from_slice(&0u16.to_be_bytes()); // len_cues
    b.extend_from_slice(&0u16.to_be_bytes()); // unknown
    b
}

/// Generate PWV4 (color preview waveform) section
/// 1200 fixed columns, 6 bytes per entry
fn generate_pwv4_section(color_preview: &WaveformColorPreview) -> Vec<u8> {
    let mut buffer = Vec::new();

    // Tag
    buffer.extend_from_slice(PWV4_TAG);

    // Header (24 bytes / 0x18) per DeepSymmetry spec:
    //   fourcc(4) | len_header(4) | len_tag(4) | len_entry_bytes(4) | len_entries(4) | unknown(4)
    // len_entry_bytes is ALWAYS 6 for PWV4; len_entries = 1200. Data (7200 bytes)
    // begins at byte 24. Previous 20-byte header omitted len_entry_bytes and hung rekordbox.
    let header_len = 24u32;
    let len_entry_bytes = 6u32;
    let len_entries = 1200u32;
    let data_size = 1200 * 6; // Always 1200 entries, 6 bytes each
    let section_len = 24 + data_size;

    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&(section_len as u32).to_be_bytes());
    buffer.extend_from_slice(&len_entry_bytes.to_be_bytes());
    buffer.extend_from_slice(&len_entries.to_be_bytes());

    // Unknown constant (bytes 14-17)
    buffer.extend_from_slice(&0u32.to_be_bytes());

    // Write exactly 1200 color preview entries
    for i in 0..1200 {
        let entry = if i < color_preview.columns.len() {
            color_preview.columns[i].to_bytes()
        } else {
            [0u8; 6]
        };
        buffer.extend_from_slice(&entry);
    }

    buffer
}

/// Generate PCO2 (extended cue points with colors) section
/// Used by CDJ-2000NXS2 and later for hot cue colors
fn generate_pco2_section(cue_points: &[CuePoint]) -> Vec<u8> {
    if cue_points.is_empty() {
        return Vec::new();
    }

    let mut buffer = Vec::new();

    // Separate memory cues and hot cues
    let hot_cues: Vec<_> = cue_points.iter().filter(|c| c.hot_cue > 0).collect();
    let memory_cues: Vec<_> = cue_points.iter().filter(|c| c.hot_cue == 0).collect();

    // Generate hot cue entries
    if !hot_cues.is_empty() {
        let section = generate_pco2_entries(&hot_cues, true);
        buffer.extend_from_slice(&section);
    }

    // Generate memory cue entries
    if !memory_cues.is_empty() {
        let section = generate_pco2_entries(&memory_cues, false);
        buffer.extend_from_slice(&section);
    }

    buffer
}

/// Generate PCO2 entries for a specific cue type
fn generate_pco2_entries(cues: &[&CuePoint], is_hot_cue: bool) -> Vec<u8> {
    let mut buffer = Vec::new();

    // PCO2 section header
    buffer.extend_from_slice(PCO2_TAG);

    // Calculate entry sizes
    // Each extended entry is at least 56 bytes for hot cues (with color)
    let base_entry_size = if is_hot_cue { 56usize } else { 40usize };
    let entries_size: usize = cues
        .iter()
        .map(|cue| {
            let comment_len = cue.comment.as_ref().map(|c| c.len() + 4).unwrap_or(0);
            base_entry_size + comment_len
        })
        .sum();

    // Header: 4 (tag) + 4 (header_len) + 4 (section_len) + 4 (type) + 2 (unknown) + 2 (count) = 20 bytes
    let header_len = 20u32 - 4;
    let section_len = 20 + entries_size;

    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&(section_len as u32).to_be_bytes());

    // Type: 0 = memory cues, 1 = hot cues
    buffer.extend_from_slice(&(if is_hot_cue { 1u32 } else { 0u32 }).to_be_bytes());

    // Unknown (2 bytes) + count (2 bytes)
    buffer.extend_from_slice(&0u16.to_be_bytes());
    buffer.extend_from_slice(&(cues.len() as u16).to_be_bytes());

    // Write cue entries
    for cue in cues {
        // Entry tag "PCP2"
        buffer.extend_from_slice(b"PCP2");

        // Calculate entry length
        let comment_len = cue.comment.as_ref().map(|c| c.len() + 4).unwrap_or(0);
        let entry_len = if is_hot_cue {
            56 + comment_len
        } else {
            40 + comment_len
        };
        buffer.extend_from_slice(&((entry_len - 4) as u32).to_be_bytes());

        // Hot cue number (0 for memory, 1-8 for hot cue A-H)
        buffer.extend_from_slice(&(cue.hot_cue as u32).to_be_bytes());

        // Type: 1=cue, 2=loop, 3=fade-in, etc.
        let cue_type_byte: u32 = match cue.cue_type {
            CueType::Cue => 1,
            CueType::Loop => 2,
            CueType::FadeIn => 3,
            CueType::FadeOut => 4,
            CueType::Load => 5,
        };
        buffer.extend_from_slice(&cue_type_byte.to_be_bytes());

        // Time position in milliseconds
        buffer.extend_from_slice(&(cue.time_ms as u32).to_be_bytes());

        // Loop end time (0xFFFFFFFF if not a loop)
        if cue.loop_ms > 0.0 {
            buffer.extend_from_slice(&((cue.time_ms + cue.loop_ms) as u32).to_be_bytes());
        } else {
            buffer.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());
        }

        // Color ID for memory cues (4 bytes) - default to 0
        buffer.extend_from_slice(&0u32.to_be_bytes());

        // Unknown bytes (8 bytes padding)
        buffer.extend_from_slice(&[0u8; 8]);

        // Comment (if present)
        if let Some(ref comment) = cue.comment {
            // Comment length including null terminator
            buffer.extend_from_slice(&((comment.len() + 1) as u32).to_be_bytes());
            buffer.extend_from_slice(comment.as_bytes());
            buffer.push(0); // Null terminator
        }

        // Hot cue color data (for hot cues only)
        if is_hot_cue {
            let color = cue
                .color
                .unwrap_or_else(|| HotCueColor::default_for_slot(cue.hot_cue));

            // Color palette index (1 byte)
            buffer.push(color.palette_index);

            // RGB values (3 bytes)
            buffer.push(color.red);
            buffer.push(color.green);
            buffer.push(color.blue);

            // Padding to align
            buffer.extend_from_slice(&[0u8; 4]);
        }
    }

    buffer
}

/// Generate PCOB (cue/loop points) section
fn generate_pcob_section(cue_points: &[CuePoint]) -> Vec<u8> {
    let mut buffer = Vec::new();

    // Tag
    buffer.extend_from_slice(PCOB_TAG);

    // PCOB header (24 bytes, per golden):
    //   tag(4) len_header(4) len_tag(4) type(4) unknown(2) len_cues(2) memory_count(4)
    let header_len = 24u32;

    // Each cue entry is 24 bytes (for memory cues) or 36 bytes (for hot cues with extended data)
    // We'll use the simpler 24-byte format for maximum compatibility
    let entry_size = 24usize;
    let entries_size = cue_points.len() * entry_size;
    let section_len = 24 + entries_size;

    buffer.extend_from_slice(&header_len.to_be_bytes());
    buffer.extend_from_slice(&(section_len as u32).to_be_bytes());

    // Cue list type (0 = memory cues, 1 = hot cues)
    buffer.extend_from_slice(&1u32.to_be_bytes());

    // Unknown (2 bytes) + entry count (2 bytes)
    buffer.extend_from_slice(&0u16.to_be_bytes());
    buffer.extend_from_slice(&(cue_points.len() as u16).to_be_bytes());

    // memory_count
    buffer.extend_from_slice(&0xffff_ffffu32.to_be_bytes());

    // Write cue entries
    for cue in cue_points {
        // Entry header (4 bytes): "PCP1" for cue entry or similar marker
        buffer.extend_from_slice(b"PCP\x01");

        // Header length after tag (4 bytes)
        buffer.extend_from_slice(&(entry_size as u32 - 4).to_be_bytes());

        // Hot cue number (4 bytes) - 0 for memory cues, 1-8 for hot cues
        buffer.extend_from_slice(&(cue.hot_cue as u32).to_be_bytes());

        // Status/type (4 bytes)
        let status: u32 = match cue.cue_type {
            CueType::Cue => 0,
            CueType::FadeIn => 1,
            CueType::FadeOut => 2,
            CueType::Load => 3,
            CueType::Loop => 4,
        };
        buffer.extend_from_slice(&status.to_be_bytes());

        // Time position in milliseconds (4 bytes)
        buffer.extend_from_slice(&(cue.time_ms as u32).to_be_bytes());

        // Loop end time in ms (4 bytes) - 0xFFFFFFFF if not a loop
        if cue.loop_ms > 0.0 {
            buffer.extend_from_slice(&((cue.time_ms + cue.loop_ms) as u32).to_be_bytes());
        } else {
            buffer.extend_from_slice(&0xFFFFFFFFu32.to_be_bytes());
        }
    }

    buffer
}

/// Generate the ANLZ directory path for a track
/// Format: PIONEER/USBANLZ/Pnnn/xxxxxxxx/ANLZ0000.DAT
pub fn generate_anlz_path(track_id: u32) -> String {
    // Directory structure based on track ID
    let dir1 = format!("P{:03}", (track_id / 256) % 1000);
    let dir2 = format!("{:08X}", track_id);
    format!("PIONEER/USBANLZ/{}/{}/ANLZ0000.DAT", dir1, dir2)
}

/// Generate .EXT file (extended analysis for Nexus+ players)
/// Golden tag set / order: PPTH PWV3 PCOB PCOB PCO2 PCO2 [PQT2] PWV5 PWV4
/// Note: PQTZ and PWAV do NOT belong in .EXT (they live in .DAT).
/// PQT2 (extended beat grid) is not generated: its 56-byte header contains a
/// per-track value we have not derived, and a wrong PQT2 is worse than none.
pub fn generate_ext_file(
    _beat_grid: &BeatGrid,
    waveform: &Waveform,
    file_path: &str,
    cue_points: &[CuePoint],
) -> Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(128 * 1024);

    let ppth_section = generate_ppth_section(file_path);
    let pwv3_section = generate_pwv3_section(&waveform.detail);
    let pwv4_section = generate_pwv4_section(&waveform.color_preview);
    let pwv5_section = generate_pwv5_section(&waveform.detail);

    // rekordbox always writes both cue lists (hot=1 then memory=0), even empty.
    let (pcob_hot, pcob_mem) = if cue_points.is_empty() {
        (generate_pcob_empty(1), generate_pcob_empty(0))
    } else {
        (generate_pcob_section(cue_points), generate_pcob_empty(0))
    };
    let (pco2_hot, pco2_mem) = if cue_points.is_empty() {
        (generate_pco2_empty(1), generate_pco2_empty(0))
    } else {
        (generate_pco2_section(cue_points), generate_pco2_empty(0))
    };

    let sections_size = ppth_section.len()
        + pwv3_section.len()
        + pcob_hot.len()
        + pcob_mem.len()
        + pco2_hot.len()
        + pco2_mem.len()
        + pwv5_section.len()
        + pwv4_section.len();
    let header_size = 28; // PMAI header
    let total_size = header_size + sections_size;

    write_pmai_header(&mut buffer, total_size);

    buffer.extend_from_slice(&ppth_section);
    buffer.extend_from_slice(&pwv3_section);
    buffer.extend_from_slice(&pcob_hot);
    buffer.extend_from_slice(&pcob_mem);
    buffer.extend_from_slice(&pco2_hot);
    buffer.extend_from_slice(&pco2_mem);
    buffer.extend_from_slice(&pwv5_section);
    buffer.extend_from_slice(&pwv4_section);

    Ok(buffer)
}

/// Generate .2EX file (CDJ-3000 3-band analysis).
/// Golden tag set / order: PPTH PWV7 PWV6 PWVC. This is NOT a copy of .EXT.
pub fn generate_2ex_file(
    _beat_grid: &BeatGrid,
    waveform: &Waveform,
    file_path: &str,
    _cue_points: &[CuePoint],
) -> Result<Vec<u8>> {
    let mut buffer = Vec::with_capacity(128 * 1024);

    let ppth_section = generate_ppth_section(file_path);
    let pwv7_section = generate_pwv7_section(&waveform.detail);
    let pwv6_section = generate_pwv6_section(&waveform.color_preview);
    let pwvc_section = generate_pwvc_section();

    let sections_size =
        ppth_section.len() + pwv7_section.len() + pwv6_section.len() + pwvc_section.len();
    let header_size = 28;
    let total_size = header_size + sections_size;

    write_pmai_header(&mut buffer, total_size);

    buffer.extend_from_slice(&ppth_section);
    buffer.extend_from_slice(&pwv7_section);
    buffer.extend_from_slice(&pwv6_section);
    buffer.extend_from_slice(&pwvc_section);

    Ok(buffer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{Beat, WaveformColorEntry, WaveformColumn};

    #[test]
    fn test_anlz_path_generation() {
        assert_eq!(
            generate_anlz_path(1),
            "PIONEER/USBANLZ/P000/00000001/ANLZ0000.DAT"
        );
        assert_eq!(
            generate_anlz_path(256),
            "PIONEER/USBANLZ/P001/00000100/ANLZ0000.DAT"
        );
        assert_eq!(
            generate_anlz_path(0x1234),
            "PIONEER/USBANLZ/P018/00001234/ANLZ0000.DAT"
        );
    }

    #[test]
    fn test_pqtz_section() {
        let grid = BeatGrid {
            bpm: 128.0,
            first_beat_ms: 100.0,
            beats: vec![
                Beat {
                    beat_number: 1,
                    time_ms: 100.0,
                    tempo_100: 12800,
                },
                Beat {
                    beat_number: 2,
                    time_ms: 568.75,
                    tempo_100: 12800,
                },
            ],
        };

        let section = generate_pqtz_section(&grid);

        // Check tag
        assert_eq!(&section[0..4], b"PQTZ");

        // Check beat count (at offset 20, after header fields)
        let count = u32::from_be_bytes([section[20], section[21], section[22], section[23]]);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_pwav_section() {
        let preview = WaveformPreview {
            columns: vec![
                WaveformColumn {
                    height: 15,
                    whiteness: 3,
                },
                WaveformColumn {
                    height: 20,
                    whiteness: 5,
                },
            ],
        };

        let section = generate_pwav_section(&preview);

        // Check tag
        assert_eq!(&section[0..4], b"PWAV");

        // Section should be header (20) + 400 bytes
        let section_len = u32::from_be_bytes([section[8], section[9], section[10], section[11]]);
        assert_eq!(section_len, 420);
    }

    #[test]
    fn test_ppth_section() {
        let section = generate_ppth_section("/Contents/test.mp3");

        // Check tag
        assert_eq!(&section[0..4], b"PPTH");

        // Path length is in BYTES: (18 chars + NUL) * 2 (UTF-16) = 38
        let path_len = u32::from_be_bytes([section[12], section[13], section[14], section[15]]);
        assert_eq!(path_len, 38);
        let len_header = u32::from_be_bytes([section[4], section[5], section[6], section[7]]);
        assert_eq!(len_header, 16);
    }

    #[test]
    fn test_complete_dat_file() {
        let grid = BeatGrid::constant_tempo(128.0, 0.0, 5000.0);
        let waveform = Waveform::default();

        let data =
            generate_dat_file(&grid, &waveform, "/Contents/test.mp3", 5_000_000, 220_500).unwrap();

        // Should start with PMAI
        assert_eq!(&data[0..4], b"PMAI");

        // File should be reasonable size
        assert!(data.len() > 100);
    }

    #[test]
    fn test_pwv3_section() {
        let detail = WaveformDetail {
            entries: vec![
                WaveformColorEntry {
                    red: 5,
                    green: 3,
                    blue: 7,
                    height: 20,
                },
                WaveformColorEntry {
                    red: 2,
                    green: 6,
                    blue: 4,
                    height: 15,
                },
            ],
        };

        let section = generate_pwv3_section(&detail);

        // Check tag
        assert_eq!(&section[0..4], b"PWV3");

        // len_header = 24, len_entry_bytes = 1 at offset 12, len_entries = 2 at offset 16
        let len_header = u32::from_be_bytes([section[4], section[5], section[6], section[7]]);
        assert_eq!(len_header, 24);
        let entry_bytes = u32::from_be_bytes([section[12], section[13], section[14], section[15]]);
        assert_eq!(entry_bytes, 1);
        let count = u32::from_be_bytes([section[16], section[17], section[18], section[19]]);
        assert_eq!(count, 2);

        // Section length = 24 (header) + 2 entries (1 byte each)
        let section_len = u32::from_be_bytes([section[8], section[9], section[10], section[11]]);
        assert_eq!(section_len, 26);
    }

    #[test]
    fn test_pcob_section() {
        let cues = vec![
            CuePoint {
                hot_cue: 1,
                cue_type: CueType::Cue,
                time_ms: 5000.0,
                loop_ms: 0.0,
                comment: None,
                color: None,
            },
            CuePoint {
                hot_cue: 2,
                cue_type: CueType::Loop,
                time_ms: 10000.0,
                loop_ms: 4000.0,
                comment: None,
                color: None,
            },
        ];

        let section = generate_pcob_section(&cues);

        // Check tag
        assert_eq!(&section[0..4], b"PCOB");

        // Entry count (at offset 18-19, u16)
        let count = u16::from_be_bytes([section[18], section[19]]);
        assert_eq!(count, 2);
    }

    #[test]
    fn test_ext_file_differs_from_dat() {
        let grid = BeatGrid::constant_tempo(128.0, 0.0, 5000.0);
        let waveform = Waveform::default();
        let cues: Vec<CuePoint> = Vec::new();

        let dat_data =
            generate_dat_file(&grid, &waveform, "/Contents/test.mp3", 5_000_000, 220_500).unwrap();
        let ext_data = generate_ext_file(&grid, &waveform, "/Contents/test.mp3", &cues).unwrap();

        // EXT should be larger than DAT (includes PWV3)
        assert!(ext_data.len() > dat_data.len());

        // Both should start with PMAI
        assert_eq!(&dat_data[0..4], b"PMAI");
        assert_eq!(&ext_data[0..4], b"PMAI");
    }

    #[test]
    fn test_ext_file_with_cues() {
        let grid = BeatGrid::constant_tempo(128.0, 0.0, 5000.0);
        let waveform = Waveform::default();
        let cues = vec![CuePoint {
            hot_cue: 1,
            cue_type: CueType::Cue,
            time_ms: 1000.0,
            loop_ms: 0.0,
            comment: None,
            color: None,
        }];

        let ext_data = generate_ext_file(&grid, &waveform, "/Contents/test.mp3", &cues).unwrap();

        // Should contain PCOB section somewhere in the file
        let ext_str = String::from_utf8_lossy(&ext_data);
        assert!(ext_str.contains("PCOB"));
    }
}
