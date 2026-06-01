//! Page allocation for Pioneer DeviceSQL databases
//!
//! Pages are 4096 bytes with:
//! - Fixed header at offset 0x00-0x1F (32 bytes common header)
//! - For DATA pages: DataPageHeader at 0x20-0x27, heap from 0x28
//! - For INDEX pages: IndexHeader at 0x20-0x3B, index entries from 0x3C
//!
//! Every table requires:
//! 1. An INDEX page (flags 0x64) that points to the first data page
//! 2. One or more DATA pages (flags 0x24 or 0x34) with actual row content
//!
//! Row group structure (36 bytes per group, from rekordcrate):
//! - Bytes 0-31: row_offsets[0..16] (16 × u16, stored in REVERSE order)
//!   - row_offsets[15] = offset for row 0 (bit 0)
//!   - row_offsets[14] = offset for row 1 (bit 1)
//!   - etc.
//! - Bytes 32-33: presence_flags (u16 bitmask of which rows exist)
//! - Bytes 34-35: unknown/padding (u16)

use crate::error::{Error, Result};

/// Page size in bytes (always 4096 for Pioneer databases)
pub const PAGE_SIZE: usize = 4096;

/// Offset where heap data begins (for data pages)
pub const HEAP_START: usize = 0x28;

/// Size of each row group in the backward-growing index
/// 2 (padding) + 2 (flags) + 16*2 (offsets) = 36 bytes
pub const ROW_GROUP_SIZE: usize = 36;

/// Maximum rows per group
pub const ROWS_PER_GROUP: usize = 16;

/// Page flags
pub const PAGE_FLAGS_INDEX: u8 = 0x64;  // Index page
pub const PAGE_FLAGS_DATA: u8 = 0x24;   // Normal data page
pub const PAGE_FLAGS_DATA_TRACK: u8 = 0x34; // Data page (tracks use this)

/// Magic value for empty table index NextPage
pub const EMPTY_TABLE_MARKER: u32 = 0x03FFFFFF;

/// Page types (table types)
/// All 20 tables (types 0-19) must be present for rekordbox PC compatibility
/// Values from Kaitai struct spec: rekordbox_pdb.ksy
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageType {
    Tracks = 0,
    Genres = 1,
    Artists = 2,
    Albums = 3,
    Labels = 4,
    Keys = 5,
    Colors = 6,
    PlaylistTree = 7,
    PlaylistEntries = 8,
    Unknown9 = 9,
    Unknown10 = 10,
    HistoryPlaylists = 11,  // Was incorrectly 13
    HistoryEntries = 12,    // Was incorrectly 14
    Artwork = 13,           // Was incorrectly 15
    Unknown14 = 14,
    Unknown15 = 15,
    Columns = 16,           // Was incorrectly 17
    Unknown17 = 17,         // uk17 in spec
    Unknown18 = 18,
    History = 19,           // Was incorrectly Unknown19
}

impl PageType {
    /// Get all 20 table types in order (required for rekordbox PC)
    /// Order matches Kaitai spec: rekordbox_pdb.ksy
    pub fn all_types() -> &'static [PageType] {
        &[
            PageType::Tracks,           // 0
            PageType::Genres,           // 1
            PageType::Artists,          // 2
            PageType::Albums,           // 3
            PageType::Labels,           // 4
            PageType::Keys,             // 5
            PageType::Colors,           // 6
            PageType::PlaylistTree,     // 7
            PageType::PlaylistEntries,  // 8
            PageType::Unknown9,         // 9
            PageType::Unknown10,        // 10
            PageType::HistoryPlaylists, // 11
            PageType::HistoryEntries,   // 12
            PageType::Artwork,          // 13
            PageType::Unknown14,        // 14
            PageType::Unknown15,        // 15
            PageType::Columns,          // 16
            PageType::Unknown17,        // 17
            PageType::Unknown18,        // 18
            PageType::History,          // 19
        ]
    }
    
    /// Get all table types that should be included in a minimal export
    pub fn required_types() -> &'static [PageType] {
        Self::all_types()
    }
}

/// Index page builder - creates the required index page for each table
pub struct IndexPageBuilder {
    data: Vec<u8>,
    page_index: u32,
    page_type: PageType,
}

impl IndexPageBuilder {
    /// Create a new index page
    pub fn new(page_index: u32, page_type: PageType) -> Self {
        let data = vec![0u8; PAGE_SIZE];
        Self {
            data,
            page_index,
            page_type,
        }
    }
    
    /// Finalize the index page
    /// - data_page_index: the data page that follows (or EMPTY_TABLE_MARKER if empty)
    /// - has_data: whether there's actual data in the data page
    /// - num_row_offsets: number of row offsets in the data page (for index entry)
    pub fn finalize(mut self, data_page_index: u32, has_data: bool, num_row_offsets: u32) -> Vec<u8> {
        // Only Tracks (0) and History (19) use actual index entries
        // All other tables have NumEntries=0 even when they contain data
        let uses_index_entries = matches!(self.page_type, PageType::Tracks | PageType::History);
        
        // Page header per Kaitai/DeepSymmetry spec (0x00-0x1F = 32 bytes)
        
        // 0x00-0x03: gap (zeros, already zero)
        
        // 0x04-0x07: page_index
        self.data[0x04..0x08].copy_from_slice(&self.page_index.to_le_bytes());
        
        // 0x08-0x0B: type (TABLE TYPE - 0=tracks, 1=genres, etc.)
        self.data[0x08..0x0C].copy_from_slice(&(self.page_type as u32).to_le_bytes());
        
        // 0x0C-0x0F: next_page - points to the DATA page
        self.data[0x0C..0x10].copy_from_slice(&data_page_index.to_le_bytes());
        
        // 0x10-0x13: sequence/transaction - always 1 for INDEX pages (per REX)
        self.data[0x10..0x14].copy_from_slice(&1u32.to_le_bytes());
        
        // 0x14-0x17: zeros (already zero)
        
        // 0x18-0x1A: row counts (0 for INDEX pages)
        // 0x1B: page_flags = 0x64 for INDEX pages
        self.data[0x1B] = PAGE_FLAGS_INDEX;
        
        // 0x1C-0x1F: free_size and used_size (0 for INDEX pages, already zero)
        
        // Index header starts at 0x20 (per REX: IndexHeaderSize = 28 + HeaderSize = 28 + 32 = 60)
        // But looking at REX's structure, the IndexHeader is right after the common header
        
        // 0x20-0x21: Unknown1 (0x1fff per REX)
        self.data[0x20..0x22].copy_from_slice(&0x1fffu16.to_le_bytes());
        
        // 0x22-0x23: Unknown2 (0x1fff per REX)
        self.data[0x22..0x24].copy_from_slice(&0x1fffu16.to_le_bytes());
        
        // 0x24-0x25: Unknown3 (0x03ec per REX)
        self.data[0x24..0x26].copy_from_slice(&0x03ecu16.to_le_bytes());
        
        // 0x26-0x27: NextOffset - 1 when table has indexed entries, 0 otherwise
        // Per empirical analysis: Tracks/History with data have NextOffset=1
        let next_offset: u16 = if has_data && uses_index_entries { 1 } else { 0 };
        self.data[0x26..0x28].copy_from_slice(&next_offset.to_le_bytes());
        
        // 0x28-0x2B: PageIndex (self-reference per REX)
        self.data[0x28..0x2C].copy_from_slice(&self.page_index.to_le_bytes());
        
        // 0x2C-0x2F: NextPage in IndexHeader - DATA page or EMPTY_TABLE_MARKER (0x03ffffff)
        let index_next_page = if has_data { data_page_index } else { EMPTY_TABLE_MARKER };
        self.data[0x2C..0x30].copy_from_slice(&index_next_page.to_le_bytes());
        
        // 0x30-0x33: Unknown5 (0x03ffffff per REX)
        self.data[0x30..0x34].copy_from_slice(&0x03FFFFFFu32.to_le_bytes());
        
        // 0x34-0x37: Unknown6 (0, already zero)

        // 0x38-0x39: NumEntries
        let num_entries = if has_data && uses_index_entries { 1u16 } else { 0u16 };
        self.data[0x38..0x3A].copy_from_slice(&num_entries.to_le_bytes());
        
        // 0x3A-0x3B: FirstEmptyEntry (0x1fff per REX)
        self.data[0x3A..0x3C].copy_from_slice(&0x1fffu16.to_le_bytes());
        
        // 0x3C+: Index entries
        if has_data && uses_index_entries {
            // First entry is the row offset count (or some related value)
            self.data[0x3C..0x40].copy_from_slice(&num_row_offsets.to_le_bytes());
            // Fill rest with 0x1ffffff8
            for i in (0x40..PAGE_SIZE - 20).step_by(4) {
                self.data[i..i+4].copy_from_slice(&0x1FFFFFF8u32.to_le_bytes());
            }
        } else {
            // Empty tables: fill with 0x1ffffff8
            for i in (0x3C..PAGE_SIZE - 20).step_by(4) {
                self.data[i..i+4].copy_from_slice(&0x1FFFFFF8u32.to_le_bytes());
            }
        }
        // Last 20 bytes stay zero (per REX)
        
        self.data
    }
}

/// A single data page being built
pub struct PageBuilder {
    /// Raw page data
    data: Vec<u8>,
    /// Current heap write position (offset from page start)
    heap_pos: usize,
    /// Number of rows written
    row_count: usize,
    /// Page index in file
    page_index: u32,
    /// Page/table type
    page_type: PageType,
    /// Row offsets (relative to HEAP_START)
    row_offsets: Vec<u16>,
}

impl PageBuilder {
    /// Create a new data page
    pub fn new(page_index: u32, page_type: PageType) -> Self {
        let data = vec![0u8; PAGE_SIZE];
        
        Self {
            data,
            heap_pos: HEAP_START,
            row_count: 0,
            page_index,
            page_type,
            row_offsets: Vec::new(),
        }
    }
    
    /// Create an empty data page (all zeros, used for tables with no content)
    pub fn empty_page() -> Vec<u8> {
        vec![0u8; PAGE_SIZE]
    }
    
    /// Create an empty placeholder page with specific page index
    /// Empty pages in rekordbox are completely zeros (type=0, flags=0x00)
    pub fn empty_page_with_index(_page_index: u32) -> Vec<u8> {
        // Empty/placeholder pages are completely zeros
        vec![0u8; PAGE_SIZE]
    }
    
    /// Calculate how much space is available for new data
    fn available_space(&self) -> usize {
        let num_groups = (self.row_count / ROWS_PER_GROUP) + 1;
        let index_size = num_groups * ROW_GROUP_SIZE;
        let index_start = PAGE_SIZE - index_size;
        
        if self.heap_pos >= index_start {
            0
        } else {
            index_start - self.heap_pos
        }
    }
    
    /// Check if adding data of given size would overflow
    pub fn would_overflow(&self, data_size: usize) -> bool {
        // Account for potential new row group if we're at a boundary
        let new_row_count = self.row_count + 1;
        let num_groups = (new_row_count / ROWS_PER_GROUP) + 1;
        let index_size = num_groups * ROW_GROUP_SIZE;
        let index_start = PAGE_SIZE - index_size;
        
        self.heap_pos + data_size > index_start
    }
    
    /// Write raw bytes to the heap, returns offset relative to HEAP_START
    pub fn write_heap(&mut self, data: &[u8]) -> Result<u16> {
        if self.would_overflow(data.len()) {
            return Err(Error::PageOverflow(format!(
                "Cannot write {} bytes, only {} available",
                data.len(),
                self.available_space()
            )));
        }
        
        let offset = (self.heap_pos - HEAP_START) as u16;
        self.data[self.heap_pos..self.heap_pos + data.len()].copy_from_slice(data);
        self.heap_pos += data.len();
        
        Ok(offset)
    }
    
    /// Add a row to the page
    /// The row data should already be written to the heap
    /// This just records the offset in the row index
    pub fn add_row(&mut self, heap_offset: u16) -> Result<()> {
        self.row_offsets.push(heap_offset);
        self.row_count += 1;
        Ok(())
    }
    
    /// Write row data and add to index in one step
    /// Rows are padded to 4-byte alignment
    pub fn write_row(&mut self, data: &[u8]) -> Result<u16> {
        let offset = self.write_heap(data)?;
        self.add_row(offset)?;
        
        // Pad to 4-byte alignment
        let current_pos = self.heap_pos - HEAP_START;
        let padding = (4 - (current_pos % 4)) % 4;
        if padding > 0 && !self.would_overflow(padding) {
            self.heap_pos += padding;  // Skip padding bytes (already zero)
        }
        
        Ok(offset)
    }
    
    /// Get current heap position (for calculating string offsets within a row)
    pub fn heap_position(&self) -> usize {
        self.heap_pos
    }
    
    /// Finalize the page and return the complete data
    pub fn finalize(mut self, next_page: u32) -> Vec<u8> {
        // Write page header with default values
        self.write_header_with_info(next_page, 0, 0);
        
        // Write row index (backwards from end)
        self.write_row_index();
        
        self.data
    }
    
    /// Finalize the page with table-specific info
    /// - next_page: the next page in the chain (or 0xFFFFFFFF if last)
    /// - table_first: the "first" value from the table pointer (stored in unk1 at 0x0C)
    /// - table_sequence: the table's sequence number (stored in next field at 0x08)
    pub fn finalize_with_table_info(mut self, table_first: u32, table_sequence: u32) -> Vec<u8> {
        // For DATA pages, next = table_sequence (not 0xFFFFFFFF!)
        self.write_header_with_info(table_sequence, table_first, table_sequence);
        
        // Write row index (backwards from end)
        self.write_row_index();
        
        self.data
    }
    
    fn write_header_with_info(&mut self, next_page: u32, sequence: u32, _table_sequence: u32) {
        // Page header per Kaitai/DeepSymmetry spec
        // Total common header: 0x00-0x27 (40 bytes)
        
        // 0x00-0x03: gap (zeros, already zero)
        
        // 0x04-0x07: page_index - the page number
        self.data[0x04..0x08].copy_from_slice(&self.page_index.to_le_bytes());
        
        // 0x08-0x0B: type - the TABLE TYPE (0=tracks, 1=genres, etc.)
        self.data[0x08..0x0C].copy_from_slice(&(self.page_type as u32).to_le_bytes());
        
        // 0x0C-0x0F: next_page - points to next page in chain (or past end of file)
        self.data[0x0C..0x10].copy_from_slice(&next_page.to_le_bytes());
        
        // 0x10-0x13: sequence (transaction counter)
        // Per REX: for DATA pages, this is the global sequence counter
        self.data[0x10..0x14].copy_from_slice(&sequence.to_le_bytes());
        
        // 0x14-0x17: always zero (already zero)
        
        // 0x18-0x1A: Packed row count (24-bit little-endian)
        // Format: packed = (num_row_offsets << 11) | num_rows
        // - Lower 11 bits: num_rows (actual row count)
        // - Upper 13 bits: num_row_offsets (always row_count * 4 for 4:1 ratio)
        // Empirically verified: most tables follow 4:1 ratio
        let num_rows = self.row_count as u32;
        let num_row_offsets = num_rows * 4;  // 4:1 ratio as per empirical findings
        let packed = (num_row_offsets << 11) | num_rows;
        self.data[0x18] = (packed & 0xFF) as u8;
        self.data[0x19] = ((packed >> 8) & 0xFF) as u8;
        self.data[0x1A] = ((packed >> 16) & 0xFF) as u8;
        
        // 0x1B: page_flags (u8)
        // 0x34 for Tracks (type 0) and History (type 19) data pages
        // 0x24 for other data pages
        self.data[0x1B] = match self.page_type {
            PageType::Tracks | PageType::History => PAGE_FLAGS_DATA_TRACK,  // 0x34
            _ => PAGE_FLAGS_DATA,  // 0x24
        };
        
        // 0x1C-0x1D: free_size (u16)
        let free_size = self.available_space() as u16;
        self.data[0x1C..0x1E].copy_from_slice(&free_size.to_le_bytes());
        
        // 0x1E-0x1F: used_size (u16) - bytes used in heap
        let used_size = (self.heap_pos - HEAP_START) as u16;
        self.data[0x1E..0x20].copy_from_slice(&used_size.to_le_bytes());
        
        // 0x20-0x21: Unknown5 (u16) - per REX: usually 1, equal to row count for some tables
        let num_rows = self.row_count as u16;
        self.data[0x20..0x22].copy_from_slice(&num_rows.to_le_bytes());
        
        // 0x22-0x23: num_rows_large (u16) - for tables with many rows
        // Per Kaitai: used when too many rows for num_rows_small
        // Usually 0, or 0x1fff for pages with deleted rows
        self.data[0x22..0x24].copy_from_slice(&0u16.to_le_bytes());
        
        // 0x24-0x25: Unknown6 (u16) - always 0
        // 0x26-0x27: Unknown7 (u16) - always 0 except 1 for history pages
        // Already zero
        
        // Heap starts at 0x28 (40 bytes header)
    }
    
    fn write_row_index(&mut self) {
        // Row group structure (36 bytes, from rekordcrate):
        // - Bytes 0-31: row_offsets[0..16] (16 × u16, stored in REVERSE order)
        // - Bytes 32-33: presence_flags (u16)
        // - Bytes 34-35: unknown/padding (u16)
        //
        // Row offsets are stored in reverse: row_offsets[15] = offset for row 0 (bit 0)
        //                                    row_offsets[14] = offset for row 1 (bit 1)
        //                                    etc.
        
        // Always write at least one row group, even for empty pages
        let num_groups = if self.row_offsets.is_empty() {
            1
        } else {
            (self.row_offsets.len() + ROWS_PER_GROUP - 1) / ROWS_PER_GROUP
        };
        
        for group_idx in 0..num_groups {
            let group_start = PAGE_SIZE - (group_idx + 1) * ROW_GROUP_SIZE;
            
            let first_row = group_idx * ROWS_PER_GROUP;
            let rows_in_group = if first_row >= self.row_offsets.len() {
                0
            } else {
                std::cmp::min(
                    ROWS_PER_GROUP,
                    self.row_offsets.len() - first_row
                )
            };
            
            // Presence flags: bits 0..(N-1) set for N rows
            let presence_flags: u16 = if rows_in_group > 0 {
                ((1u32 << rows_in_group) - 1) as u16
            } else {
                0
            };
            
            // Write row offsets in REVERSE order
            // row_offsets[15] = offset for row 0 (bit 0)
            // row_offsets[14] = offset for row 1 (bit 1)
            // etc.
            for i in 0..rows_in_group {
                let row_idx = first_row + i;
                // Store in reverse: row i goes to array position (15 - i)
                let array_pos = ROWS_PER_GROUP - 1 - i;
                let offset_pos = group_start + array_pos * 2;
                self.data[offset_pos..offset_pos + 2]
                    .copy_from_slice(&self.row_offsets[row_idx].to_le_bytes());
            }
            
            // Write presence_flags at byte 32
            self.data[group_start + 32..group_start + 34]
                .copy_from_slice(&presence_flags.to_le_bytes());
            
            // Bytes 34-35: MUST be a copy of presence_flags (not padding!)
            // This is required by rekordbox - empirically verified
            self.data[group_start + 34..group_start + 36]
                .copy_from_slice(&presence_flags.to_le_bytes());
        }
    }
    
    /// Get number of rows in this page
    pub fn row_count(&self) -> usize {
        self.row_count
    }
    
    /// Get page index
    pub fn page_index(&self) -> u32 {
        self.page_index
    }
}

/// Table pointer in file header
/// Format per Kaitai/DeepSymmetry spec: (type, empty_candidate, first_page, last_page)
#[derive(Debug, Clone, Copy, Default)]
pub struct TablePointer {
    pub table_type: u32,       // Table type (0-19)
    pub empty_candidate: u32,  // Next empty slot for this table
    pub first_page: u32,       // First page (usually the INDEX page)
    pub last_page: u32,        // Last page (last DATA page, or same as first if empty)
}

impl TablePointer {
    /// Create a new table pointer
    /// Per Kaitai spec, order is: (type, empty_candidate, first_page, last_page)
    pub fn new(table_type: PageType, empty_candidate: u32, first_page: u32, last_page: u32) -> Self {
        Self {
            table_type: table_type as u32,
            empty_candidate,
            first_page,
            last_page,
        }
    }
    
    /// Serialize to bytes - format: (type, empty_candidate, first_page, last_page)
    pub fn to_bytes(&self) -> [u8; 16] {
        let mut bytes = [0u8; 16];
        bytes[0..4].copy_from_slice(&self.table_type.to_le_bytes());
        bytes[4..8].copy_from_slice(&self.empty_candidate.to_le_bytes());
        bytes[8..12].copy_from_slice(&self.first_page.to_le_bytes());
        bytes[12..16].copy_from_slice(&self.last_page.to_le_bytes());
        bytes
    }
}

/// File header builder
/// Format per Kaitai/DeepSymmetry spec:
/// - 0x00-0x03: zero padding
/// - 0x04-0x07: len_page (4096)
/// - 0x08-0x0B: num_tables
/// - 0x0C-0x0F: next_unused_page
/// - 0x10-0x13: track_count (empirically verified: matches number of tracks in database)
/// - 0x14-0x17: sequence (commit counter)
/// - 0x18-0x1B: gap (zeros)
/// - 0x1C+: table pointers (20 entries × 16 bytes)
pub struct FileHeader {
    pub page_size: u32,
    pub num_tables: u32,
    pub next_unused_page: u32,
    pub track_count: u32,   // Field at 0x10 - empirically verified to be track count
    pub sequence: u32,      // Commit counter
    pub tables: Vec<TablePointer>,
}

impl FileHeader {
    pub fn new() -> Self {
        Self {
            page_size: PAGE_SIZE as u32,
            num_tables: 0,
            next_unused_page: 1,
            track_count: 0,  // Will be set based on actual track count
            sequence: 2,     // Per REX: starts at 2
            tables: Vec::new(),
        }
    }
    
    pub fn add_table(&mut self, pointer: TablePointer) {
        self.tables.push(pointer);
        self.num_tables = self.tables.len() as u32;
    }
    
    pub fn to_page(&self) -> Vec<u8> {
        let mut page = vec![0u8; PAGE_SIZE];
        
        // 0x00-0x03: zero padding (already zero)
        
        // 0x04-0x07: len_page
        page[0x04..0x08].copy_from_slice(&self.page_size.to_le_bytes());
        
        // 0x08-0x0B: num_tables
        page[0x08..0x0C].copy_from_slice(&self.num_tables.to_le_bytes());
        
        // 0x0C-0x0F: next_unused_page
        page[0x0C..0x10].copy_from_slice(&self.next_unused_page.to_le_bytes());
        
        // 0x10-0x13: track_count (empirically verified)
        page[0x10..0x14].copy_from_slice(&self.track_count.to_le_bytes());
        
        // 0x14-0x17: sequence
        page[0x14..0x18].copy_from_slice(&self.sequence.to_le_bytes());
        
        // 0x18-0x1B: gap (already zero)
        
        // 0x1C+: table pointers
        let mut offset = 0x1C;
        for table in &self.tables {
            let bytes = table.to_bytes();
            page[offset..offset + 16].copy_from_slice(&bytes);
            offset += 16;
        }
        
        page
    }
}

impl Default for FileHeader {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_page_builder_basic() {
        let mut page = PageBuilder::new(1, PageType::Artists);
        
        // Write some test data
        let data = b"test row data";
        let offset = page.write_row(data).unwrap();
        
        assert_eq!(offset, 0);
        assert_eq!(page.row_count(), 1);
    }
    
    #[test]
    fn test_page_overflow_detection() {
        let page = PageBuilder::new(1, PageType::Artists);
        
        // Should not overflow for small data
        assert!(!page.would_overflow(100));
        
        // Should overflow for data larger than page
        assert!(page.would_overflow(PAGE_SIZE));
    }
    
    #[test]
    fn test_row_index_structure() {
        let mut page = PageBuilder::new(1, PageType::Artists);
        
        // Add 3 rows
        for i in 0..3 {
            let data = format!("row{}", i);
            page.write_row(data.as_bytes()).unwrap();
        }
        
        let finalized = page.finalize(0xFFFFFFFF);
        
        // Row group structure (36 bytes from end):
        // - Bytes 0-31: row_offsets[0..16]
        // - Bytes 32-33: presence_flags
        // - Bytes 34-35: padding
        let group_start = PAGE_SIZE - ROW_GROUP_SIZE;
        
        // Check presence flags at byte 32 of the group
        let flags = u16::from_le_bytes([
            finalized[group_start + 32],
            finalized[group_start + 33],
        ]);
        
        // 3 rows = bits 0, 1, 2 set = 0b111 = 7
        assert_eq!(flags, 0x0007);
        
        // Check row offsets are in reverse order
        // row_offsets[15] = row 0, row_offsets[14] = row 1, row_offsets[13] = row 2
        let offset_0 = u16::from_le_bytes([
            finalized[group_start + 30], // position 15 * 2
            finalized[group_start + 31],
        ]);
        assert_eq!(offset_0, 0); // Row 0 at heap offset 0
    }
}
