//! Page-aware diff between two `export.pdb` files.
//!
//! This is the comparison half of the golden-file harness. Workflow:
//!   1. `generate_xml` -> import into rekordbox -> export USB  => *golden* PDB.
//!   2. `rekord-export` direct export of the same tracks       => *mine* PDB.
//!   3. `diff_pdb(golden, mine)` localises every difference to a page, labels
//!      the page by its DeviceSQL table type, and annotates known header
//!      fields (page_type, next_page, packed row counts, flags, ...).
//!
//! It does not try to be a structural parser of row data — the point is to
//! answer "which bytes differ and where do they sit" fast, so corruption
//! causes can be bisected by changing one input at a time and re-diffing.

use std::fmt::Write as _;

use crate::page::PAGE_SIZE;

/// Contiguous run of differing bytes within a page.
#[derive(Debug, Clone)]
pub struct ByteRange {
    /// Offset of the first differing byte within the page (0x00..0x1000).
    pub start: usize,
    /// Inclusive end offset.
    pub end: usize,
    pub golden: Vec<u8>,
    pub mine: Vec<u8>,
    /// Human label for `start` (e.g. "flags", "page_type", "heap").
    pub field: &'static str,
}

/// All differences found on a single page.
#[derive(Debug, Clone)]
pub struct PageDiff {
    pub index: u32,
    /// Table name derived from the page's own type byte at 0x08.
    pub table: String,
    pub ranges: Vec<ByteRange>,
}

/// Result of comparing two PDB files.
#[derive(Debug, Clone)]
pub struct PdbDiff {
    pub golden_pages: usize,
    pub mine_pages: usize,
    pub diffs: Vec<PageDiff>,
    /// Page indices present in one file but not the other.
    pub only_in_golden: Vec<u32>,
    pub only_in_mine: Vec<u32>,
}

impl PdbDiff {
    pub fn is_identical(&self) -> bool {
        self.diffs.is_empty()
            && self.only_in_golden.is_empty()
            && self.only_in_mine.is_empty()
    }

    /// Render a human-readable report.
    pub fn report(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "PDB diff: golden = {} pages, mine = {} pages",
            self.golden_pages, self.mine_pages
        );
        if self.golden_pages != self.mine_pages {
            let _ = writeln!(
                s,
                "  ! page count differs ({:+})",
                self.mine_pages as i64 - self.golden_pages as i64
            );
        }

        if self.is_identical() {
            s.push_str("\nFiles are byte-identical.\n");
            return s;
        }

        let compared = self.golden_pages.min(self.mine_pages);
        let identical = compared - self.diffs.len();
        let _ = writeln!(
            s,
            "\n{} pages compared, {} identical, {} differ\n",
            compared,
            identical,
            self.diffs.len()
        );

        for pd in &self.diffs {
            let _ = writeln!(
                s,
                "Page {} [{}] differs ({} range{}):",
                pd.index,
                pd.table,
                pd.ranges.len(),
                if pd.ranges.len() == 1 { "" } else { "s" }
            );
            for r in &pd.ranges {
                let span = if r.start == r.end {
                    format!("0x{:04X}", r.start)
                } else {
                    format!("0x{:04X}..0x{:04X}", r.start, r.end)
                };
                let _ = writeln!(
                    s,
                    "  {:<17} {:<14} golden={}  mine={}",
                    span,
                    r.field,
                    hex(&r.golden),
                    hex(&r.mine),
                );
            }
            s.push('\n');
        }

        if !self.only_in_golden.is_empty() {
            let _ = writeln!(s, "Pages only in golden: {:?}", self.only_in_golden);
        }
        if !self.only_in_mine.is_empty() {
            let _ = writeln!(s, "Pages only in mine: {:?}", self.only_in_mine);
        }

        // Per-table tally of differing pages.
        let mut tally: Vec<(String, usize)> = Vec::new();
        for pd in &self.diffs {
            if let Some(e) = tally.iter_mut().find(|(t, _)| *t == pd.table) {
                e.1 += 1;
            } else {
                tally.push((pd.table.clone(), 1));
            }
        }
        if !tally.is_empty() {
            let parts: Vec<String> =
                tally.iter().map(|(t, n)| format!("{}x{}", t, n)).collect();
            let _ = writeln!(s, "Differing pages by table: {}", parts.join(", "));
        }
        s
    }
}

/// Compare two PDB byte buffers.
pub fn diff_pdb(golden: &[u8], mine: &[u8]) -> PdbDiff {
    let gp = golden.len() / PAGE_SIZE;
    let mp = mine.len() / PAGE_SIZE;
    let compared = gp.min(mp);

    let mut diffs = Vec::new();
    for idx in 0..compared {
        let off = idx * PAGE_SIZE;
        let g = &golden[off..off + PAGE_SIZE];
        let m = &mine[off..off + PAGE_SIZE];
        if g == m {
            continue;
        }
        let ranges = diff_page(idx as u32, g, m);
        if !ranges.is_empty() {
            diffs.push(PageDiff {
                index: idx as u32,
                table: page_label(idx as u32, g),
                ranges,
            });
        }
    }

    let only_in_golden = (compared..gp).map(|i| i as u32).collect();
    let only_in_mine = (compared..mp).map(|i| i as u32).collect();

    PdbDiff {
        golden_pages: gp,
        mine_pages: mp,
        diffs,
        only_in_golden,
        only_in_mine,
    }
}

/// Find runs of differing bytes, merging runs separated by <=3 equal bytes so
/// closely-spaced differences read as one annotated range.
fn diff_page(index: u32, g: &[u8], m: &[u8]) -> Vec<ByteRange> {
    const GAP: usize = 3;
    let mut raw: Vec<(usize, usize)> = Vec::new();
    let mut i = 0;
    while i < PAGE_SIZE {
        if g[i] != m[i] {
            let start = i;
            let mut end = i;
            i += 1;
            // extend, tolerating small equal gaps
            while i < PAGE_SIZE {
                if g[i] != m[i] {
                    end = i;
                    i += 1;
                } else {
                    // look ahead for another diff within GAP
                    let mut j = i;
                    let mut found = false;
                    while j < PAGE_SIZE && j - i < GAP {
                        if g[j] != m[j] {
                            found = true;
                            break;
                        }
                        j += 1;
                    }
                    if found {
                        i = j;
                    } else {
                        break;
                    }
                }
            }
            raw.push((start, end));
        } else {
            i += 1;
        }
    }

    raw.into_iter()
        .map(|(start, end)| ByteRange {
            start,
            end,
            golden: clip(&g[start..=end]),
            mine: clip(&m[start..=end]),
            field: annotate(index, start),
        })
        .collect()
}

/// Cap a byte slice for display.
fn clip(b: &[u8]) -> Vec<u8> {
    const MAX: usize = 16;
    if b.len() > MAX {
        b[..MAX].to_vec()
    } else {
        b.to_vec()
    }
}

fn hex(b: &[u8]) -> String {
    let mut s = String::new();
    for (i, byte) in b.iter().enumerate() {
        if i > 0 {
            s.push(' ');
        }
        let _ = write!(s, "{:02X}", byte);
    }
    if b.len() == 16 {
        s.push_str(" ...");
    }
    s
}

/// Label a page by reading its own type byte (file header is page 0).
fn page_label(index: u32, page: &[u8]) -> String {
    if index == 0 {
        return "File Header".to_string();
    }
    let ty = u32::from_le_bytes([page[0x08], page[0x09], page[0x0A], page[0x0B]]);
    page_type_name(ty)
}

fn page_type_name(ty: u32) -> String {
    let name = match ty {
        0 => "Tracks",
        1 => "Genres",
        2 => "Artists",
        3 => "Albums",
        4 => "Labels",
        5 => "Keys",
        6 => "Colors",
        7 => "PlaylistTree",
        8 => "PlaylistEntries",
        9 => "Unknown9",
        10 => "Unknown10",
        11 => "HistoryPlaylists",
        12 => "HistoryEntries",
        13 => "Artwork",
        14 => "Unknown14",
        15 => "Unknown15",
        16 => "Columns",
        17 => "Unknown17",
        18 => "Unknown18",
        19 => "History",
        _ => return format!("type{}", ty),
    };
    name.to_string()
}

/// Map a within-page offset to a known header field name.
fn annotate(index: u32, off: usize) -> &'static str {
    if index == 0 {
        // File header (page 0) layout.
        return match off {
            0x00..=0x03 => "fh:zero",
            0x04..=0x07 => "fh:page_size",
            0x08..=0x0B => "fh:num_tables",
            0x0C..=0x0F => "fh:next_unused_page",
            0x10..=0x13 => "fh:unknown",
            0x14..=0x17 => "fh:sequence",
            0x18..=0x1B => "fh:gap",
            _ => "fh:table_pointers",
        };
    }
    match off {
        0x00..=0x03 => "zero",
        0x04..=0x07 => "page_index",
        0x08..=0x0B => "page_type",
        0x0C..=0x0F => "next_page",
        0x10..=0x13 => "sequence",
        0x14..=0x17 => "unknown",
        0x18..=0x1A => "packed_counts",
        0x1B => "flags",
        0x1C..=0x1D => "free_size",
        0x1E..=0x1F => "used_size",
        0x20..=0x27 => "page_hdr_tail",
        _ => "heap/row-data",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn blank_page(index: u32, ty: u32, flags: u8) -> Vec<u8> {
        let mut p = vec![0u8; PAGE_SIZE];
        p[0x04..0x08].copy_from_slice(&index.to_le_bytes());
        p[0x08..0x0C].copy_from_slice(&ty.to_le_bytes());
        p[0x1B] = flags;
        p
    }

    #[test]
    fn detects_flag_difference_and_labels_table() {
        // Page 0 = file header (left identical), page 1 = Tracks with flag diff.
        let mut golden = vec![0u8; PAGE_SIZE]; // file header
        let mut mine = golden.clone();

        let g_tracks = blank_page(1, 0, 0x34);
        let m_tracks = blank_page(1, 0, 0x24); // wrong flag
        golden.extend_from_slice(&g_tracks);
        mine.extend_from_slice(&m_tracks);

        let d = diff_pdb(&golden, &mine);
        assert_eq!(d.golden_pages, 2);
        assert_eq!(d.diffs.len(), 1);
        let pd = &d.diffs[0];
        assert_eq!(pd.index, 1);
        assert_eq!(pd.table, "Tracks");
        assert_eq!(pd.ranges.len(), 1);
        assert_eq!(pd.ranges[0].start, 0x1B);
        assert_eq!(pd.ranges[0].field, "flags");
        assert_eq!(pd.ranges[0].golden, vec![0x34]);
        assert_eq!(pd.ranges[0].mine, vec![0x24]);
        assert!(d.report().contains("Page 1 [Tracks] differs"));
    }

    #[test]
    fn identical_files() {
        let p = vec![0u8; PAGE_SIZE * 3];
        let d = diff_pdb(&p, &p);
        assert!(d.is_identical());
        assert!(d.report().contains("byte-identical"));
    }

    #[test]
    fn page_count_mismatch_reported() {
        let g = vec![0u8; PAGE_SIZE * 3];
        let m = vec![0u8; PAGE_SIZE * 2];
        let d = diff_pdb(&g, &m);
        assert_eq!(d.only_in_golden, vec![2]);
        assert!(d.report().contains("page count differs"));
    }
}
