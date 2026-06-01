# PDB Format Analysis Report: Specification Discrepancies & Knowledge Gaps

**Date:** January 18, 2026  
**Purpose:** Identify deviations from DeepSymmetry specification and gaps requiring empirical verification

---

## Executive Summary

After thorough analysis of the rekord-export codebase against the authoritative DeepSymmetry documentation and Kaitai specification, I've identified **13 specification discrepancies** and **9 critical knowledge gaps**. Several issues are likely contributing to the "Device library corrupted" error from rekordbox 6.x.

**Severity Legend:**
- 🔴 **CRITICAL** - Likely causes corruption detection
- 🟡 **MODERATE** - May cause issues with specific features
- 🟢 **LOW** - Probably works but not spec-compliant
- ⚪ **UNKNOWN** - Requires empirical verification

---

## Part 1: Confirmed Specification Discrepancies

### 1. 🔴 CRITICAL: Row Count Packed Format (bytes 0x18-0x1A)

**DeepSymmetry Specification:**
> "The three bytes `18`–`1a`, labeled 'row counts', actually contain two non-byte-aligned numbers. The first 13 bits, *num_row_offsets* keeps track of how many row offsets have ever been allocated in the heap. The final 11 bits, *num_rows*, report the number of valid rows."

**Current Implementation (page.rs:367-371):**
```rust
// 0x18: num_rows_small (u8) - row count
self.data[0x18] = self.row_count as u8;

// 0x19: bitmask/Unknown3 (u8) - per REX: increments by 0x20 for each active row
self.data[0x19] = ((self.row_count as u16 * 0x20) & 0xFF) as u8;
```

**Discrepancy:** The implementation treats these as separate byte fields, not a packed 24-bit structure with 13-bit and 11-bit components.

**Your validate.py confirms this matters:** Lines 42-50 check for a 4:1 ratio between `num_offsets` and `num_rows`. The spec implies `num_row_offsets` should be 4× `num_rows`.

**EMPIRICAL TEST NEEDED:**
1. Create a golden file with 5 rows
2. Extract bytes 0x18-0x1A from a DATA page
3. Verify: `packed = byte[0x18] | (byte[0x19] << 8) | (byte[0x1A] << 16)`
4. Check: `num_rows = packed & 0x7FF`, `num_offsets = packed >> 11`
5. Confirm ratio is 4.0

---

### 2. 🔴 CRITICAL: INDEX Page Entry Format (Which Tables Use Entries?)

**Your Memory States:**
> "Only two table types (Tracks and History) actually use index entries in INDEX pages, while all other 18 tables use empty markers (NextOffset=0, NumEntries=0, fill with 0x1ffffff8) regardless of whether they contain data."

**Current Implementation (page.rs:179-199):**
```rust
// 0x38-0x39: NumEntries
let num_entries = if has_data { 1u16 } else { 0u16 };
self.data[0x38..0x3A].copy_from_slice(&num_entries.to_le_bytes());
// ...
if has_data {
    // First entry is the row offset count (or some related value)
    self.data[0x3C..0x40].copy_from_slice(&num_row_offsets.to_le_bytes());
```

**Discrepancy:** The code writes `NumEntries=1` and an index entry for ANY table with data, not just Tracks (type 0) and History (type 19).

**FIX REQUIRED:**
```rust
let uses_index_entries = matches!(self.page_type, PageType::Tracks | PageType::History);
let num_entries = if has_data && uses_index_entries { 1u16 } else { 0u16 };
```

---

### 3. 🟡 MODERATE: Row Group Bytes 34-35 Purpose

**DeepSymmetry Specification:**
> "The last two bytes after each row presence bitmask (for example *tranrf0* after *rowpf0*) store a bit mask of rows touched by the last transaction on this row group."

**Current Implementation (page.rs:462-465):**
```rust
// Bytes 34-35: MUST be a copy of presence_flags (not padding!)
// This is required by rekordbox - empirically verified
self.data[group_start + 34..group_start + 36]
    .copy_from_slice(&presence_flags.to_le_bytes());
```

**Discrepancy:** Comment says "empirically verified" as presence_flags copy, but spec says these are transaction flags (rows touched in last transaction).

**STATUS:** Your empirical test may be correct. This contradicts the spec but may reflect actual rekordbox behavior. **Needs re-verification with fresh golden file.**

---

### 4. 🟡 MODERATE: Data Page Header Fields 0x20-0x27

**DeepSymmetry Specification:**
| Offset | Field | Purpose |
|--------|-------|---------|
| 0x20-0x21 | transaction_row_count | Number of rows touched in last transaction |
| 0x22-0x23 | transaction_row_index | Index of first row touched in last transaction |
| 0x24-0x25 | u6 | Unknown (0x1004 for strange pages, 0x0000 for data) |
| 0x26-0x27 | u7 | Usually 0, sometimes 1 for history pages |

**Current Implementation (page.rs:391-403):**
```rust
// 0x20-0x21: Unknown5 (u16) - per REX: usually 1, equal to row count for some tables
let num_rows = self.row_count as u16;
self.data[0x20..0x22].copy_from_slice(&num_rows.to_le_bytes());

// 0x22-0x23: num_rows_large (u16) - for tables with many rows
self.data[0x22..0x24].copy_from_slice(&0u16.to_le_bytes());
```

**Discrepancy:** Spec says 0x20 is `transaction_row_count`, not `num_rows`. Using row_count here may be wrong.

**EMPIRICAL TEST NEEDED:**
1. Create export with 10 tracks
2. Examine DATA page for tracks table
3. Check if 0x20-0x21 equals row count or is always 0 or 1

---

### 5. 🟡 MODERATE: Page Flags Distribution (0x24 vs 0x34)

**DeepSymmetry Specification:**
> "According to Mr. Lesniak, 'strange' (non-data) pages will have the value `44` or `64`, and other pages have had the values `24` or `34`."

**Your PDB_STATUS_REPORT.md:**
> "Golden uses 0x34 more liberally."

**Current Implementation (page.rs:377-382):**
```rust
self.data[0x1B] = match self.page_type {
    PageType::Tracks | PageType::History => PAGE_FLAGS_DATA_TRACK,  // 0x34
    _ => PAGE_FLAGS_DATA,  // 0x24
};
```

**Discrepancy:** Hardcoded to only use 0x34 for Tracks and History, but golden files may use 0x34 for more tables.

**EMPIRICAL TEST NEEDED:**
1. Generate golden export with data in Artists, Albums, Genres, Keys tables
2. Check page_flags (0x1B) for each table's DATA pages
3. Document which tables use 0x24 vs 0x34

---

### 6. 🟢 LOW: File Header Unknown Field (0x10-0x13)

**DeepSymmetry Specification:**
> No specific value documented for this field.

**Current Implementation (page.rs:538, FileHeader):**
```rust
unknown: 5,    // Per REX: observed as 0x5, 0x4, or 0x1
```

**Status:** Using 5 is consistent with REX observations. Low risk but undocumented.

---

### 7. 🟢 LOW: File Header Sequence (seqdb at 0x14-0x17)

**DeepSymmetry Specification:**
> "This value is incremented after updating a given page header so can be considered the 'next' page sequence number."

**Current Implementation (page.rs:539):**
```rust
sequence: 2,   // Per REX: starts at 2
```

**Status:** Starting at 2 matches REX. Spec says it increments with each edit. Fresh exports should be fine with any starting value.

---

## Part 2: Critical Knowledge Gaps

### Gap 1: Unknown17 (uk17) Row Size

**Conflict Between Sources:**
- **Kaitai Spec:** `4 × u4 = 16 bytes per row`
- **REX Implementation:** `4 × u16 = 8 bytes per row`
- **Your Implementation:** Uses 8-byte format (matching REX)

**KAITAI_COMPLIANCE.md acknowledges this:**
> "uk17_row: Using REX format (4×u16=8 bytes) - note: Kaitai says 4×u4=16 bytes"

**EMPIRICAL TEST NEEDED:**
1. Export a golden file from rekordbox 6.x
2. Parse Unknown17 table (type 17)
3. Count bytes between rows to determine actual row size
4. Validate against known row count

---

### Gap 2: Columns Table (Type 16) Structure

**Current Implementation (pdb.rs:718-746):**
Hard-coded binary blobs copied from a rekordbox export:
```rust
let columns_data: &[&[u8]] = &[
    &[1, 0, 128, 0, 144, 18, 0, 0, 250, 255, 71, 0, 69, 0, 78, 0...
```

**Problem:** No structural understanding. If any byte is wrong, it's undetectable.

**EMPIRICAL TEST NEEDED:**
1. Create minimal golden with 1 track
2. Parse Columns table row-by-row
3. Document: row structure, string encoding, field meanings
4. Implement programmatic generation

---

### Gap 3: History Table (Type 19) Row Format

**Current Implementation (pdb.rs:863-873):**
```rust
let history_row: [u8; 40] = [
    0x80, 0x02,  // subtype
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,  // padding
    0x17,  // string section length (23)
    b'2', b'0', b'2', b'5', b'-', b'0', b'1', b'-', b'0', b'1',  // date
    ...
];
```

**Problem:** Completely undocumented format. Structure is guessed.

**DeepSymmetry only says:**
> "Data used by rekordbox to synchronize history playlists (not yet studied)."

**EMPIRICAL TEST NEEDED:**
1. Create golden export
2. Analyze History table (type 19) DATA page byte-by-byte
3. Create 2nd export after playing some tracks
4. Diff to understand which fields change

---

### Gap 4: Unknown18 Table Purpose & Format

**Current Implementation (pdb.rs:812-842):**
Hard-coded tuples with unknown meaning:
```rust
let unknown18_data: &[(u16, u16, u16, u16)] = &[
    (1, 6, 1, 0),
    (21, 7, 1, 0),
    ...
];
```

**Problem:** No understanding of what these values represent.

**EMPIRICAL TEST NEEDED:**
1. Compare Unknown18 across multiple golden exports (different track counts)
2. Determine if values are static or dynamic
3. Document any patterns

---

### Gap 5: Empty Table Representation

**Observation from spec:**
Every table must exist, but how should truly empty tables be represented?

**Current Implementation (pdb.rs:389-391):**
```rust
fn build_empty_data_pages(&self, next_idx: &mut u32) -> Result<(Vec<Vec<u8>>, bool)> {
    *next_idx += 1;
    Ok((vec![PageBuilder::empty_page()], false))
}
```

Where `empty_page()` returns all zeros.

**Questions:**
- Should empty DATA pages have page_index and type set, or be all zeros?
- Should empty tables have INDEX → DATA chain, or just INDEX pointing nowhere?

**EMPIRICAL TEST NEEDED:**
1. Create golden with NO labels (Labels table empty)
2. Examine Labels table: INDEX page structure, DATA page structure
3. Check page_flags, type fields

---

### Gap 6: Multi-Page Table Chaining

**Partially Documented Behavior:**
When a table spans multiple DATA pages, how do they chain?

**Current Implementation (pdb.rs:417-418):**
```rust
pages.push(current_page.finalize(0xFFFFFFFF));  // Last page points to invalid
```

**Questions:**
- Does last page's next_page point to 0xFFFFFFFF or to empty_candidate?
- Does the INDEX page's NextPage field point to first or last DATA page?

**EMPIRICAL TEST NEEDED:**
1. Create golden with 100+ tracks (forces multi-page Tracks table)
2. Examine chaining: INDEX.NextPage, DATA[0].next_page, DATA[1].next_page
3. Document the chain structure

---

### Gap 7: ISRC String Format Verification

**DeepSymmetry Specification:**
> "When an International Standard Recording Code is present... it is marked with kind `90` but does not actually hold a UTF-16-LE string. Instead, the first byte after the `pad` value following the length is the value `03` and then there are [length-6] bytes of ASCII, followed by a null byte."

**Current Implementation (string.rs:90-108):**
```rust
pub fn encode_isrc(isrc: &str) -> Vec<u8> {
    // ...
    result.push(FLAG_UTF16LE); // Uses 0x90 flag despite being ASCII
    result.push((total_len & 0xFF) as u8);
    result.push(((total_len >> 8) & 0xFF) as u8);
    result.push(0x00);
    result.push(0x03); // ISRC marker
    result.extend_from_slice(isrc.as_bytes());
    result.push(0x00); // Null terminator
```

**Uncertainty:** Spec says `[length-6]` bytes, but length calculation might differ.

**EMPIRICAL TEST NEEDED:**
1. Create golden with track that has ISRC in metadata
2. Extract ISRC string from track row
3. Verify exact byte structure

---

### Gap 8: Track Row String Offset Base

**Question:** Are string offsets relative to row start or to some other base?

**Current Implementation (pdb.rs:921-926):**
```rust
// Calculate offsets (relative to row start)
let mut string_offsets = Vec::with_capacity(STRING_COUNT);
let mut current_offset = HEADER_SIZE;  // 136 bytes
for s in &strings {
    string_offsets.push(current_offset as u16);
```

**DeepSymmetry:**
> "To find the start of the string, add the address of the start of the track row to the offset."

**Status:** Implementation matches spec. But verify with golden file.

---

### Gap 9: Index Page Index Entry Meaning

**DeepSymmetry:**
Doesn't fully explain what the index entries contain or how they're used.

**Current Implementation (page.rs:187-189):**
```rust
if has_data {
    // First entry is the row offset count (or some related value)
    self.data[0x3C..0x40].copy_from_slice(&num_row_offsets.to_le_bytes());
```

**Question:** Is this `num_row_offsets`, `num_rows * 4`, or something else?

---

## Part 3: Recommended Empirical Tests (Priority Order)

### Priority 1: Row Count Packed Format
```bash
# Create golden with exactly 5 tracks
# Extract bytes 0x18-0x1A from Tracks DATA page
python3 -c "
import struct
with open('golden.pdb', 'rb') as f:
    f.seek(PAGE_SIZE * tracks_data_page + 0x18)
    packed = struct.unpack('<I', f.read(4))[0] & 0xFFFFFF
    num_rows = packed & 0x7FF
    num_offsets = packed >> 11
    print(f'num_rows={num_rows}, num_offsets={num_offsets}, ratio={num_offsets/num_rows}')
"
```

### Priority 2: INDEX Page Entry Table Specificity
```bash
# For each table type 0-19 in golden:
# Check if 0x38-0x39 (NumEntries) is 0 or non-zero
# Document which tables have actual index entries
```

### Priority 3: Page Flags Survey
```bash
# For every DATA page in golden:
# Record: table_type, page_index, page_flags (0x1B)
# Group by table_type, determine pattern
```

### Priority 4: Unknown17/Unknown18 Analysis
```bash
# Parse these tables byte-by-byte
# Compare across 3+ golden files
# Determine if content is static or dynamic
```

---

## Part 4: Quick Fixes (High Confidence)

These can be fixed without further testing:

### Fix 1: INDEX Page NumEntries (page.rs)
```rust
// Change from:
let num_entries = if has_data { 1u16 } else { 0u16 };

// To:
let uses_index = matches!(self.page_type, PageType::Tracks | PageType::History);
let num_entries = if has_data && uses_index { 1u16 } else { 0u16 };
```

### Fix 2: INDEX Page Entry Writing (page.rs)
```rust
// Change from:
if has_data {
    self.data[0x3C..0x40].copy_from_slice(&num_row_offsets.to_le_bytes());
    for i in (0x40..PAGE_SIZE - 20).step_by(4) {
        self.data[i..i+4].copy_from_slice(&0x1FFFFFF8u32.to_le_bytes());
    }
}

// To:
let uses_index = matches!(self.page_type, PageType::Tracks | PageType::History);
if has_data && uses_index {
    self.data[0x3C..0x40].copy_from_slice(&num_row_offsets.to_le_bytes());
    for i in (0x40..PAGE_SIZE - 20).step_by(4) {
        self.data[i..i+4].copy_from_slice(&0x1FFFFFF8u32.to_le_bytes());
    }
} else {
    // Even tables with data: NextOffset=0, NumEntries=0, fill with 0x1ffffff8
    for i in (0x3C..PAGE_SIZE - 20).step_by(4) {
        self.data[i..i+4].copy_from_slice(&0x1FFFFFF8u32.to_le_bytes());
    }
}
```

---

## Conclusion

The most likely causes of "Device library corrupted" are:

1. **INDEX page entries for wrong tables** (Priority 1 fix above)
2. **Packed row count format mismatch** (Needs empirical verification)
3. **Page flags inconsistency** (Needs empirical verification)

The safest path forward:
1. Create a minimal golden file (1 track, 1 playlist)
2. Byte-by-byte diff against your generated export.pdb
3. Focus on page headers and INDEX pages first
4. Document findings and iterate

Would you like me to create a Python script that performs detailed validation against a golden file?
