# PDB Generation Status Report
## rekord-export Project - December 28, 2025

---

## Executive Summary

The project generates Pioneer rekordbox-compatible USB exports for CDJ players. The primary challenge has been creating valid `export.pdb` files that pass rekordbox PC's strict validation (the "Device library corrupted" error).

**Current Status:** Multiple critical bugs have been identified and fixed based on research from authoritative sources (Kaitai/DeepSymmetry spec, REX Go project). The latest version (v2.8) addresses the most critical issue (empty_candidate collisions). Testing is needed to confirm if this resolves all issues.

---

## Authoritative Sources Used

1. **Kaitai Specification (DeepSymmetry)**: `/home/claude/rex/rex/kaitai/rekordbox_pdb.ksy`
   - Definitive format documentation
   - Field layouts, enums, data types

2. **REX Go Project**: `/home/claude/rex/rex/pkg/rekordbox/`
   - Working implementation reference
   - `dbengine/db.go` - database construction
   - `page/page.go` - page structure
   - `page/index.go` - index page handling
   - `pdb/pdb.go` - file header

3. **Golden Reference File**: `/mnt/user-data/uploads/export_-_golden.pdb`
   - Real rekordbox 6.8.4 export with 8 tracks
   - Used for byte-by-byte comparison

---

## Fixed Issues (Verified)

### 1. Page Header Field Order (v2.7)
**Problem:** Offset 0x08 was being set to `page_index` instead of `table_type`.

**Kaitai Spec:**
```
0x00-0x03: gap (zeros)
0x04-0x07: page_index
0x08-0x0B: type (TABLE TYPE: 0=tracks, 1=genres, etc.)  ← WAS WRONG
0x0C-0x0F: next_page
0x10-0x13: sequence
```

**Fix:** Now correctly writes table type (0-19) at offset 0x08.

### 2. File Header Table Pointer Location (v2.7)
**Problem:** Table pointers started at 0x10 instead of 0x1C.

**Kaitai Spec:**
```
0x00-0x03: gap (zeros)
0x04-0x07: len_page (4096)
0x08-0x0B: num_tables (20)
0x0C-0x0F: next_unused_page
0x10-0x13: unknown (typically 5)
0x14-0x17: sequence (commit counter)
0x18-0x1B: gap (zeros)
0x1C+:     table pointers start HERE  ← WAS AT 0x10
```

**Fix:** Table pointers now written starting at offset 0x1C.

### 3. Table Pointer Field Order (v2.7)
**Problem:** Field order was (first, empty, last, type) instead of (type, empty, first, last).

**Kaitai Spec:**
```
Each table pointer is 16 bytes:
  0x00: type (u32)           - table type 0-19
  0x04: empty_candidate (u32) - next free slot for this table
  0x08: first_page (u32)     - first page (INDEX page)
  0x0C: last_page (u32)      - last page (last DATA page or INDEX if empty)
```

**Fix:** Now uses correct order.

### 4. Empty Candidate Collision (v2.8) - CRITICAL
**Problem:** Each table's `empty_candidate` pointed to a page used by the NEXT table.

**Before (broken):**
```
Table 0: empty=3  → Page 3 is Table 1's INDEX! ❌
Table 1: empty=5  → Page 5 is Table 2's INDEX! ❌
... (all 20 tables had collisions)
```

**Fix:** All `empty_candidate` values now point to `next_unused_page` (first truly free page). All DATA pages' `next_page` (0x0C) also patched to match.

### 5. Sequence Counter (v2.7)
**Problem:** INDEX and DATA pages had incorrect sequence values.

**Per REX:**
- INDEX pages: sequence = 1 (always)
- DATA pages: sequence = global counter, increments with each page

**Fix:** INDEX pages get sequence=1, DATA pages get incrementing global counter.

---

## Potentially Remaining Issues (Needs Testing)

### 1. DATA Page `next_page` Field
**Current Implementation:** All DATA pages have `next_page = final_next_unused_page`

**Golden File Observation:** DATA pages have `next_page` = their table's `empty_candidate`

**Status:** Should be correct now since we set both to the same value, but needs verification.

### 2. Sequence Counter Values
**Current:** Starts at 2, increments with each DATA page.

**Golden:** Values like 44, 22, 41, 23... (not strictly sequential)

**Analysis:** Golden file was created over time with edits. Our fresh export should have sequential values, which should be valid.

### 3. INDEX Page Sequence
**Current:** Always 1 for INDEX pages.

**Golden:** Tracks INDEX (page 1) has sequence=32, others have 1.

**Analysis:** Per REX, INDEX sequence=1 is correct for fresh tables. The 32 in golden suggests the tracks table was modified 32 times. Fresh export with 1 should be valid.

### 4. Row Index Structure
**Current Implementation:**
```
End of page (36 bytes per row group):
  - 16 × u16 offsets (reversed order)
  - u16 presence bitmask
  - u16 padding/unknown
```

**Confidence:** HIGH - matches golden file structure.

### 5. Unknown Header Fields
**0x10 (unknown):** We use 5 (per REX). Golden also has 5. ✓

**0x19 (bitmask):** We calculate as `row_count * 0x20`. Golden varies.

**0x1A (Unknown4):** We use 0. Golden also 0 for most tables. ✓

**0x20-0x21 (Unknown5):** We use row_count. Golden varies (sometimes 1, sometimes row_count).

**0x22-0x23 (num_rows_large):** We use 0. Golden uses this for tables with many rows.

---

## Known Unknowns (Not Yet Investigated)

### 1. Multi-Page Tables
**Issue:** When a table spans multiple DATA pages, how should they chain?

**Current:** Not fully implemented - we allocate pages but linking may be wrong.

**Golden Example:** Tracks table has only 1 DATA page in our test file.

**Risk:** LOW for small libraries, HIGH for large libraries.

### 2. Index Page Entries
**Current:** 
- Empty tables: fill with 0x1ffffff8
- Active tables: first entry = num_rows, rest = 0x1ffffff8

**Per REX:** More complex index structure for lookups.

**Risk:** MEDIUM - CDJs may not use index for basic playback.

### 3. Page Flags
**Current:**
- 0x64 for INDEX pages
- 0x24 for most DATA pages  
- 0x34 for Tracks (type 0) and History (type 19)

**Observation:** Golden uses 0x34 more liberally.

**Risk:** LOW - flags seem informational.

### 4. String Encoding
**Current Implementation:**
```
0x40 prefix: long ASCII (length in next 2 bytes)
0x90 prefix: long UTF-16LE
Odd byte: short ASCII (length = byte >> 1)
```

**Confidence:** HIGH - matches Kaitai spec.

### 5. Row Alignment
**Current:** Rows aligned to 4-byte boundaries.

**Confidence:** HIGH - per REX implementation.

---

## File Structure Summary

### Page Layout (4096 bytes)
```
Offset  Size  Field
------  ----  -----
0x00    4     gap (zeros)
0x04    4     page_index
0x08    4     type (table type for this page)
0x0C    4     next_page
0x10    4     sequence
0x14    4     zeros
0x18    1     num_rows_small
0x19    1     bitmask (row_count * 0x20)
0x1A    1     Unknown4 (0)
0x1B    1     page_flags (0x64=INDEX, 0x24/0x34=DATA)
0x1C    2     free_size
0x1E    2     used_size
0x20    2     Unknown5
0x22    2     num_rows_large
0x24    2     Unknown6 (0)
0x26    2     Unknown7 (0)
0x28    ...   heap data (for DATA pages)
        ...   
end-36  36    row index (per 16-row group)
```

### Table Types (20 total)
```
0  = Tracks
1  = Genres
2  = Artists
3  = Albums
4  = Labels
5  = Keys
6  = Colors
7  = PlaylistTree
8  = PlaylistEntries
9  = Unknown9
10 = Unknown10
11 = HistoryPlaylists
12 = HistoryEntries
13 = Artwork
14 = Unknown14
15 = Unknown15
16 = Columns
17 = Unknown17
18 = Unknown18
19 = History
```

### Required Tables
All 20 tables MUST exist, even if empty. Each table has:
- 1 INDEX page (flags 0x64)
- 1+ DATA pages (flags 0x24/0x34) or 1 empty page (all zeros)

---

## Validation Approach

### Python Validation Script
```python
def validate_pdb(filepath):
    # Check header
    # - len_page = 4096
    # - num_tables = 20
    # - gap at 0x18 = 0
    
    # Check table pointers (at 0x1C)
    # - type matches index (0-19)
    # - empty_candidate >= next_unused_page
    # - first_page and last_page within file
    
    # Check each page
    # - page_index matches position
    # - type is valid (0-19 for DATA, matches table for INDEX)
    # - flags are valid (0x64, 0x24, 0x34, or 0x00 for empty)
```

### Key Validation Points
1. **No empty_candidate collisions** - each must point to unused page
2. **Consistent page_index** - must match physical position
3. **Valid table types** - 0-19 only
4. **20 tables present** - all must exist

---

## Testing Checklist

### Immediate Tests
- [ ] Rebuild with v2.8 and test in rekordbox PC
- [ ] If still fails, upload new export.pdb for analysis
- [ ] Test on actual CDJ hardware

### If Still Failing
1. Run validation script on new export
2. Compare byte-by-byte with golden
3. Check for remaining field mismatches
4. Focus on DATA page content (row structure)

### If Passes rekordbox
1. Test all CDJ features (browse, play, cue points)
2. Test with larger library (50+ tracks)
3. Test playlist functionality
4. Test artwork display

---

## Code Locations

### Key Files
- `rekordbox-core/src/pdb.rs` - Main PDB builder
- `rekordbox-core/src/page.rs` - Page structures (FileHeader, TablePointer, PageBuilder, IndexPageBuilder)
- `rekordbox-core/src/string.rs` - DeviceSQL string encoding

### Critical Functions
- `PdbBuilder::build()` - Main entry point, builds all 20 tables
- `PdbBuilder::build_table_with_sequence()` - Builds INDEX + DATA pages
- `PageBuilder::finalize_with_table_info()` - Writes DATA page header
- `IndexPageBuilder::finalize()` - Writes INDEX page
- `FileHeader::to_page()` - Writes file header with table pointers

---

## Reference Commands

### Validate PDB File
```bash
python3 -c "
import struct
with open('export.pdb', 'rb') as f:
    data = f.read()
# ... validation code
"
```

### Compare Two PDB Files
```bash
xxd file1.pdb > /tmp/1.hex
xxd file2.pdb > /tmp/2.hex
diff /tmp/1.hex /tmp/2.hex | head -100
```

### Check Table Pointers
```bash
hexdump -C export.pdb | head -30
# Table pointers start at 0x1C
```

---

## Version History

| Version | Date | Changes |
|---------|------|---------|
| 2.8.0 | 2025-12-28 | Fixed empty_candidate collisions (CRITICAL) |
| 2.7.0 | 2025-12-28 | Fixed page header format, table pointer location and order |
| 2.6.0 | 2025-12-28 | Various field fixes (superseded) |

---

## Next Steps

1. **Test v2.8** - Does it pass rekordbox validation?
2. **If fails** - Upload new export.pdb for further analysis
3. **If passes** - Move to CDJ hardware testing
4. **Document** - Update this report with findings

---

## Contact/Context

This project reverse-engineers Pioneer's USB export format. Key resources:
- DeepSymmetry documentation: https://github.com/Deep-Symmetry/crate-digger
- REX project: Go implementation reference
- rekordcrate: Rust library (read-focused, write support incomplete)

The goal is CDJ compatibility without requiring rekordbox software, enabling mobile DJ workflows.
