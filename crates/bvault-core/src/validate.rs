//! PDB validation utilities
//!
//! Validates Pioneer DeviceSQL (export.pdb) files against the binary structure
//! documented at https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/
//!
//! File Header (page 0):
//! - Bytes 4-7: page_size (must be 4096)
//! - Bytes 8-11: num_tables
//! - Bytes 12-15: next_unused_page
//! - Bytes 20-23: sequence
//! - Bytes 28+: Table pointers (16 bytes each)
//!
//! Data Page Header:
//! - Bytes 4-7: page_index
//! - Bytes 8-11: page_type (table type)
//! - Bytes 12-15: next_page (0xFFFFFFFF if none)
//! - Bytes 24-26: packed row counts
//! - Byte 27: page_flags

use crate::error::{Error, Result};
use crate::page::{PAGE_SIZE, HEAP_START};

/// Statistics about a PDB file
#[derive(Debug, Default, Clone)]
pub struct PdbStats {
    pub total_pages: u32,
    pub track_count: u32,
    pub artist_count: u32,
    pub album_count: u32,
    pub genre_count: u32,
    pub key_count: u32,
    pub playlist_count: u32,
    pub playlist_entry_count: u32,
}

/// Result of validating a PDB file
#[derive(Debug)]
pub struct ValidationResult {
    pub valid: bool,
    pub stats: PdbStats,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl ValidationResult {
    fn new() -> Self {
        Self {
            valid: true,
            stats: PdbStats::default(),
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }

    fn add_error(&mut self, msg: impl Into<String>) {
        self.valid = false;
        self.errors.push(msg.into());
    }

    fn add_warning(&mut self, msg: impl Into<String>) {
        self.warnings.push(msg.into());
    }
}

/// Validate a PDB file and return detailed results
///
/// Checks:
/// - File size is at least one page (4096 bytes)
/// - File size is page-aligned
/// - Header page_size field matches expected 4096
/// - Table pointers reference valid pages
/// - Data page indices match their position in file
/// - Page flags are valid
pub fn validate_pdb(data: &[u8]) -> ValidationResult {
    let mut result = ValidationResult::new();

    // Check minimum size (at least header page)
    if data.len() < PAGE_SIZE {
        result.add_error(format!(
            "File too small: {} bytes (minimum {} bytes for header page)",
            data.len(),
            PAGE_SIZE
        ));
        return result;
    }

    // Check page alignment
    if data.len() % PAGE_SIZE != 0 {
        result.add_error(format!(
            "File size {} is not a multiple of page size {}",
            data.len(),
            PAGE_SIZE
        ));
        return result;
    }

    let actual_pages = (data.len() / PAGE_SIZE) as u32;

    // Parse header (page 0)
    // Header structure from page.rs FileHeader::to_page():
    //   Bytes 0-3: zero
    //   Bytes 4-7: page_size
    //   Bytes 8-11: num_tables
    //   Bytes 12-15: next_unused_page
    //   Bytes 16-19: unknown
    //   Bytes 20-23: sequence
    //   Bytes 24-27: unknown
    //   Bytes 28+: table pointers
    let header = &data[0..PAGE_SIZE];

    // Validate page_size field (bytes 4-7)
    let page_size = u32::from_le_bytes([header[4], header[5], header[6], header[7]]);
    if page_size != PAGE_SIZE as u32 {
        result.add_error(format!(
            "Invalid page_size in header: {} (expected {})",
            page_size,
            PAGE_SIZE
        ));
        return result;
    }

    // Get num_tables (bytes 8-11)
    let num_tables = u32::from_le_bytes([header[8], header[9], header[10], header[11]]);

    // Sanity check - Pioneer DBs typically have < 20 table types
    if num_tables > 20 {
        result.add_warning(format!(
            "Unusually high table count: {} (expected < 20)",
            num_tables
        ));
    }

    // Get next_unused_page (bytes 12-15)
    let next_unused_page = u32::from_le_bytes([header[12], header[13], header[14], header[15]]);
    result.stats.total_pages = next_unused_page;

    if next_unused_page > actual_pages {
        result.add_error(format!(
            "Header next_unused_page ({}) exceeds actual page count ({})",
            next_unused_page,
            actual_pages
        ));
    }

    // Parse table pointers starting at byte 28
    // TablePointer structure from page.rs:
    //   Bytes 0-3: table_type
    //   Bytes 4-7: empty_candidate
    //   Bytes 8-11: first_page
    //   Bytes 12-15: last_page
    for i in 0..num_tables {
        let ptr_offset = 28 + (i as usize) * 16;

        if ptr_offset + 16 > PAGE_SIZE {
            result.add_error(format!(
                "Table pointer {} at offset {} extends beyond header page",
                i, ptr_offset
            ));
            break;
        }

        let table_type = u32::from_le_bytes([
            header[ptr_offset],
            header[ptr_offset + 1],
            header[ptr_offset + 2],
            header[ptr_offset + 3],
        ]);

        let first_page = u32::from_le_bytes([
            header[ptr_offset + 8],
            header[ptr_offset + 9],
            header[ptr_offset + 10],
            header[ptr_offset + 11],
        ]);

        let last_page = u32::from_le_bytes([
            header[ptr_offset + 12],
            header[ptr_offset + 13],
            header[ptr_offset + 14],
            header[ptr_offset + 15],
        ]);

        // Validate page references
        if first_page != 0 && first_page >= actual_pages {
            result.add_error(format!(
                "Table {} (type {}) first_page {} exceeds page count {}",
                i, table_type, first_page, actual_pages
            ));
            continue;
        }

        if last_page != 0xFFFFFFFF && last_page >= actual_pages {
            result.add_error(format!(
                "Table {} (type {}) last_page {} exceeds page count {}",
                i, table_type, last_page, actual_pages
            ));
            continue;
        }

        // Count rows in this table by walking the page chain
        if first_page > 0 && first_page < actual_pages {
            let row_count = count_table_rows(data, first_page, actual_pages);

            // Map table_type to stats field
            // From page.rs PageType enum:
            //   Tracks = 0, Genres = 1, Artists = 2, Albums = 3,
            //   Labels = 4, Keys = 5, Colors = 6,
            //   PlaylistTree = 7, PlaylistEntries = 8
            match table_type {
                0 => result.stats.track_count = row_count,
                1 => result.stats.genre_count = row_count,
                2 => result.stats.artist_count = row_count,
                3 => result.stats.album_count = row_count,
                5 => result.stats.key_count = row_count,
                7 => result.stats.playlist_count = row_count,
                8 => result.stats.playlist_entry_count = row_count,
                _ => {}
            }
        }
    }

    // Validate each data page
    for page_idx in 1..actual_pages {
        let page_start = (page_idx as usize) * PAGE_SIZE;
        let page = &data[page_start..page_start + PAGE_SIZE];

        if let Err(e) = validate_data_page(page, page_idx) {
            result.add_warning(format!("Page {}: {}", page_idx, e));
        }
    }

    result
}

/// Count rows across all pages of a table by following the page chain
fn count_table_rows(data: &[u8], first_page: u32, max_pages: u32) -> u32 {
    let mut total = 0;
    let mut current_page = first_page;
    let mut visited = std::collections::HashSet::new();

    while current_page < max_pages && current_page != 0xFFFFFFFF {
        // Detect circular references
        if visited.contains(&current_page) {
            break;
        }
        visited.insert(current_page);

        let page_start = (current_page as usize) * PAGE_SIZE;
        let page = &data[page_start..page_start + PAGE_SIZE];

        // Extract row count from packed header bytes 24-26
        // From page.rs PageBuilder::write_header():
        //   let packed = (num_row_offsets & 0x1FFF) | ((num_rows & 0x7FF) << 13);
        // So num_rows is the upper 11 bits (bits 13-23)
        let packed = (page[24] as u32) | ((page[25] as u32) << 8) | ((page[26] as u32) << 16);
        let num_rows = (packed >> 13) & 0x7FF;
        total += num_rows;

        // Get next_page pointer (bytes 12-15)
        current_page = u32::from_le_bytes([page[12], page[13], page[14], page[15]]);
    }

    total
}

/// Validate a single data page
fn validate_data_page(page: &[u8], expected_idx: u32) -> Result<()> {
    // Data page header structure from page.rs PageBuilder::write_header():
    //   Bytes 0-3: zero
    //   Bytes 4-7: page_index
    //   Bytes 8-11: page_type
    //   Bytes 12-15: next_page
    //   Bytes 16-19: version (1)
    //   Bytes 20-23: unknown2
    //   Bytes 24-26: packed row counts
    //   Byte 27: page_flags (0x34 for data)
    //   Bytes 28-29: free_size
    //   Bytes 30-31: used_size

    // Verify page_index matches position in file
    let stored_idx = u32::from_le_bytes([page[4], page[5], page[6], page[7]]);
    if stored_idx != expected_idx {
        return Err(Error::Validation(format!(
            "page_index mismatch: stored {} vs position {}",
            stored_idx, expected_idx
        )));
    }

    // Check page_flags (byte 27)
    // 0x34 = normal data page (from page.rs)
    // 0x00 = sometimes seen for empty/unused pages
    // 0x24, 0x64 = variations seen in real databases
    let flags = page[27];
    if flags != 0x34 && flags != 0x00 && flags != 0x24 && flags != 0x64 {
        return Err(Error::Validation(format!(
            "unexpected page_flags: 0x{:02X}",
            flags
        )));
    }

    // Verify used_size doesn't exceed available heap space
    let used_size = u16::from_le_bytes([page[30], page[31]]) as usize;
    let max_heap = PAGE_SIZE - HEAP_START;
    if used_size > max_heap {
        return Err(Error::Validation(format!(
            "used_size {} exceeds max heap {}",
            used_size, max_heap
        )));
    }

    Ok(())
}

/// Validate a PDB file and print results to stdout
pub fn validate_and_print(data: &[u8]) -> bool {
    let result = validate_pdb(data);

    println!("PDB Validation Results");
    println!("======================");
    println!();

    println!("Status: {}", if result.valid { "VALID" } else { "INVALID" });
    println!();

    println!("Statistics:");
    println!("  Total pages: {}", result.stats.total_pages);
    println!("  Tracks: {}", result.stats.track_count);
    println!("  Artists: {}", result.stats.artist_count);
    println!("  Albums: {}", result.stats.album_count);
    println!("  Genres: {}", result.stats.genre_count);
    println!("  Keys: {}", result.stats.key_count);
    println!("  Playlists: {}", result.stats.playlist_count);
    println!("  Playlist entries: {}", result.stats.playlist_entry_count);
    println!();

    if !result.errors.is_empty() {
        println!("Errors:");
        for err in &result.errors {
            println!("  - {}", err);
        }
        println!();
    }

    if !result.warnings.is_empty() {
        println!("Warnings:");
        for warn in &result.warnings {
            println!("  - {}", warn);
        }
        println!();
    }

    result.valid
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_empty() {
        let result = validate_pdb(&[]);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("too small")));
    }

    #[test]
    fn test_validate_too_small() {
        let data = vec![0u8; 100];
        let result = validate_pdb(&data);
        assert!(!result.valid);
    }

    #[test]
    fn test_validate_misaligned() {
        let data = vec![0u8; PAGE_SIZE + 100];
        let result = validate_pdb(&data);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("not a multiple")));
    }

    #[test]
    fn test_validate_bad_page_size() {
        let mut data = vec![0u8; PAGE_SIZE];
        // Set page_size to wrong value
        data[4..8].copy_from_slice(&1000u32.to_le_bytes());
        let result = validate_pdb(&data);
        assert!(!result.valid);
        assert!(result.errors.iter().any(|e| e.contains("page_size")));
    }

    #[test]
    fn test_validate_minimal_valid() {
        use crate::page::FileHeader;

        let header = FileHeader::new();
        let data = header.to_page();
        let result = validate_pdb(&data);

        // Should be valid with just header
        assert!(result.valid, "Errors: {:?}", result.errors);
    }
}
