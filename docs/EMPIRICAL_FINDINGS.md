# Empirical Findings from Golden PDB Analysis

**Date:** January 18, 2026  
**Golden File:** rekordbox 6.x export with 5 tracks  
**Methodology:** Byte-level comparison and structural analysis

---

## Executive Summary

Analysis of a rekordbox-generated `export.pdb` file confirms several critical hypotheses and reveals new insights about format nuances. Key findings:

1. ✅ **INDEX Page NumEntries Fix VERIFIED** - Only Tracks (0) and History (19) have NumEntries > 0
2. ✅ **Page Flags Pattern VERIFIED** - 0x34 for Tracks/History, 0x24 for others
3. ✅ **Unknown17 Row Size VERIFIED** - 8 bytes (4×u16), NOT 16 bytes
4. ⚠️ **Row Group Bytes 34-35** - NOT always a copy of presence flags
5. ⚠️ **Packed Row Count Anomalies** - Tracks and History don't follow 4:1 ratio

---

## TEST 1: INDEX Page NumEntries (CRITICAL FIX VERIFIED ✅)

### Hypothesis
Only Tracks (type 0) and History (type 19) should have NumEntries > 0 in their INDEX pages. All other 18 tables should have NumEntries=0 and fill index area with 0x1FFFFFF8, **even when they contain data**.

### Golden File Results

| Type | Table | Page | NumEntries | FirstEntry |
|------|-------|------|------------|------------|
| 0 | Tracks | 1 | **1** | 0x00000010 |
| 1 | Genres | 3 | 0 | 0x1FFFFFF8 |
| 2 | Artists | 5 | 0 | 0x1FFFFFF8 |
| 3 | Albums | 7 | 0 | 0x1FFFFFF8 |
| 4 | Labels | 9 | 0 | 0x1FFFFFF8 |
| 5 | Keys | 11 | 0 | 0x1FFFFFF8 |
| 6 | Colors | 13 | 0 | 0x1FFFFFF8 |
| 7 | PlaylistTree | 15 | 0 | 0x1FFFFFF8 |
| 8 | PlaylistEntries | 17 | 0 | 0x1FFFFFF8 |
| 9-18 | (various) | - | 0 | 0x1FFFFFF8 |
| 19 | History | 39 | **1** | 0x00000140 |

### Verdict
**✅ CONFIRMED** - The fix in `page.rs` (lines 179-202) is correct. Only types 0 and 19 write index entries.

---

## TEST 2: Packed Row Count Format (bytes 0x18-0x1A)

### Hypothesis
Format: `packed = (num_row_offsets << 11) | num_rows`
- Lower 11 bits: num_rows
- Upper 13 bits: num_row_offsets  
- Expected ratio: 4:1 (offsets to rows)

### Golden File Results

| Table | 0x18 | 0x19 | 0x1A | rows | offsets | ratio |
|-------|------|------|------|------|---------|-------|
| Tracks | 0x07 | 0xA0 | 0x00 | 7 | 20 | **2.86** ⚠️ |
| Genres | 0x01 | 0x20 | 0x00 | 1 | 4 | 4.00 ✓ |
| Artists | 0x04 | 0x80 | 0x00 | 4 | 16 | 4.00 ✓ |
| Albums | 0x04 | 0x80 | 0x00 | 4 | 16 | 4.00 ✓ |
| Labels | 0x03 | 0x60 | 0x00 | 3 | 12 | 4.00 ✓ |
| Colors | 0x08 | 0x00 | 0x01 | 8 | 32 | 4.00 ✓ |
| PlaylistTree | 0x01 | 0x20 | 0x00 | 1 | 4 | 4.00 ✓ |
| PlaylistEntries | 0x05 | 0xA0 | 0x00 | 5 | 20 | 4.00 ✓ |
| Artwork | 0x02 | 0x40 | 0x00 | 2 | 8 | 4.00 ✓ |
| Columns | 0x1B | 0x60 | 0x03 | 27 | 108 | 4.00 ✓ |
| Unknown17 | 0x16 | 0xC0 | 0x02 | 22 | 88 | 4.00 ✓ |
| Unknown18 | 0x11 | 0x20 | 0x02 | 17 | 68 | 4.00 ✓ |
| History | 0x06 | 0x20 | 0x00 | 6 | 4 | **0.67** ⚠️ |

### Analysis of Anomalies

**Tracks (ratio 2.86):**
- 7 rows in packed count, but only 5 present (presence=0x005E)
- Row slots 0 and 5 exist but are "deleted" (presence bit = 0)
- `num_rows` appears to be "highest row index + 1", not count of present rows

**History (ratio 0.67):**
- 6 rows in packed count, only 1 present (presence=0x0020)
- Only row 5 is present, containing date "2026-01-18"

### Verdict
**⚠️ PARTIAL MATCH** - The 4:1 ratio holds for non-indexed tables. For Tracks and History, the semantics may differ. This may not be critical for compatibility.

---

## TEST 3: Page Flags Distribution (DATA Pages)

### Hypothesis
DATA page flags at offset 0x1B:
- 0x34 for Tracks and History (indexed tables)
- 0x24 for all other tables

### Golden File Results

| Table | Type | Flags | Expected | Match |
|-------|------|-------|----------|-------|
| Tracks | 0 | 0x34 | 0x34 | ✓ |
| Genres | 1 | 0x24 | 0x24 | ✓ |
| Artists | 2 | 0x24 | 0x24 | ✓ |
| Albums | 3 | 0x24 | 0x24 | ✓ |
| Labels | 4 | 0x24 | 0x24 | ✓ |
| Colors | 6 | 0x24 | 0x24 | ✓ |
| PlaylistTree | 7 | 0x24 | 0x24 | ✓ |
| PlaylistEntries | 8 | 0x24 | 0x24 | ✓ |
| Artwork | 13 | 0x24 | 0x24 | ✓ |
| Columns | 16 | 0x24 | 0x24 | ✓ |
| Unknown17 | 17 | 0x24 | 0x24 | ✓ |
| Unknown18 | 18 | 0x24 | 0x24 | ✓ |
| History | 19 | 0x34 | 0x34 | ✓ |

### Verdict
**✅ CONFIRMED** - Implementation should use 0x34 for types 0 and 19, 0x24 for others.

---

## TEST 4: Row Group Bytes 34-35

### Hypothesis
Bytes 34-35 of each row group duplicate the presence_flags (bytes 32-33).

### Golden File Results

| Table | Presence | Bytes 34-35 | Match |
|-------|----------|-------------|-------|
| Tracks | 0x005E | 0x0060 | **DIFFER** |
| Genres | 0x0001 | 0x0001 | MATCH |
| Artists | 0x000F | 0x0008 | **DIFFER** |
| Albums | 0x000F | 0x0008 | **DIFFER** |
| Labels | 0x0007 | 0x0004 | **DIFFER** |
| Colors | 0x00FF | 0x00FF | MATCH |
| PlaylistTree | 0x0001 | 0x0001 | MATCH |
| PlaylistEntries | 0x001F | 0x0010 | **DIFFER** |
| Artwork | 0x0003 | 0x0002 | **DIFFER** |
| Columns | 0xFFFF | 0xFFFF | MATCH |
| Unknown17 | 0xFFFF | 0xFFFF | MATCH |
| Unknown18 | 0xFFFF | 0xFFFF | MATCH |
| History | 0x0020 | 0x0030 | **DIFFER** |

### Pattern Analysis
- **MATCH**: When all slots in group are populated (static/reference tables)
- **DIFFER**: When table has been modified (user data tables)

The Kaitai spec says: "transaction flags - bitmask of rows touched by last transaction"

This suggests bytes 34-35 track which rows were modified in the last insert/update/delete operation, NOT a copy of presence flags.

### Verdict
**⚠️ NOT A SIMPLE COPY** - The implementation should probably just set bytes 34-35 to the same value as presence flags for new rows. rekordbox will update this during modifications. This is likely NOT a corruption cause.

---

## TEST 5: Unknown17 Row Size

### Conflict
- Kaitai Spec: 4 × u4 = 16 bytes per row
- REX Implementation: 4 × u16 = 8 bytes per row

### Golden File Results

```
Row sizes (from offset differences): [8, 8, 8, 8, 8, 8, 8, ...] 
Unique sizes: {8}
```

Sample rows interpreted as 4×u16:
```
Row 0: (1, 1, 355, 0)
Row 1: (5, 6, 261, 0)
Row 2: (6, 7, 355, 0)
...
```

### Verdict
**✅ CONFIRMED: 8-byte rows (4×u16)**

REX is correct. The Kaitai spec appears to have an error (u4 should be u16).

---

## TEST 6: History Table Row Structure

### Status
DeepSymmetry: "Not yet studied"

### Golden File Analysis

History DATA page (page 40):
- num_rows=6 (packed), but only 1 present (presence=0x0020 = bit 5 only)
- Present row at offset 0x00C8

Row hex dump:
```
80 02 A0 00 05 00 00 00 00 00 00 00 17 32 30 32
36 2D 30 31 2D 31 38 19 1E 0B 31 30 30 30 03 00
```

ASCII interpretation: `2026-01-18` (date) and `1000` (time 10:00?)

### Preliminary Structure
```
Offset  Size  Field
0x00    2     row_type (0x0280?)
0x02    2     unknown
0x04    4     track_id? (5)
0x08    4     unknown (0)
0x0C    1     date string length (0x17 = 23 - but date is only 10 chars?)
0x0D    10    date "2026-01-18"
...
```

### Verdict
**⚠️ NEEDS MORE STUDY** - Current hardcoded History row likely works but structure not fully understood.

---

## TEST 7: Empty Table Structure

### Golden File (Empty PDB) Analysis

Tables in an empty PDB (before any export):

**Single-page tables (INDEX only, no DATA):**
- Tracks, Genres, Artists, Albums, Labels, Keys
- PlaylistTree, PlaylistEntries
- Unknown9, Unknown10, HistoryPlaylists, HistoryEntries
- Artwork, Unknown14, Unknown15

**Pre-populated tables (exist in empty PDB):**
- Colors: 8 rows (standard color definitions)
- Columns: 27 rows (column definitions)  
- Unknown17: 22 rows
- Unknown18: 17 rows
- History: 1 row

### Verdict
**✅ CONFIRMED** - Empty tables don't need DATA pages. When first data is added, a DATA page is allocated and table pointers updated.

---

## Implementation Recommendations

### Immediate Actions (High Confidence)

1. **INDEX Page NumEntries** ✅ (Already fixed)
   - Only write index entries for types 0 and 19

2. **Page Flags**
   - Verify DATA page flag is 0x34 for Tracks/History, 0x24 for others

3. **Unknown17 Row Size**
   - Verify using 8-byte (4×u16) format, not 16-byte

### Lower Priority (May Not Affect Compatibility)

4. **Row Group Bytes 34-35**
   - Current approach (copy presence flags) is acceptable
   - rekordbox will update during modifications

5. **Packed Row Count**
   - Anomalies in Tracks/History may be due to deletion semantics
   - For fresh exports with no deletions, 4:1 ratio should work

---

## Files Analyzed

| File | Size | Pages | Description |
|------|------|-------|-------------|
| export_-_empty.pdb | 167,936 | 41 | Fresh USB initialization |
| export_-_filled.pdb | 167,936 | 41 | After 5-track export |

---

## Next Steps

1. Generate test PDB with current implementation
2. Compare byte-by-byte against golden file
3. Focus on critical structural elements:
   - File header (especially sequence counter)
   - Table pointers (especially empty_candidate values)
   - INDEX page headers and entries
   - DATA page headers
   - Row group structure

---

## Appendix A: INDEX Page 0x2C Field Analysis

The INDEX page has a special field at offset 0x2C that points to the associated DATA page.

### Pattern Observed

| Table | INDEX Page | 0x2C (Empty) | 0x2C (Filled) | Has DATA? |
|-------|------------|--------------|---------------|-----------|
| Tracks | 1 | 0x03FFFFFF | 0x00000002 | Yes |
| Genres | 3 | 0x03FFFFFF | 0x00000004 | Yes |
| Artists | 5 | 0x03FFFFFF | 0x00000006 | Yes |
| Keys | 11 | 0x03FFFFFF | 0x03FFFFFF | No |
| Colors | 13 | 0x0000000E | 0x0000000E | Yes (pre-populated) |
| History | 39 | 0x00000028 | 0x00000028 | Yes (pre-populated) |

### Rule
- When table has data: 0x2C → DATA page number
- When table is empty: 0x2C → 0x03FFFFFF (EMPTY_TABLE_MARKER)
- Pre-populated tables (Colors, Columns, Unknown17, Unknown18, History) always have data

### Implementation Status
**✅ Correctly implemented** in `page.rs` lines 170-172

---

## Appendix B: File Header Field 0x10 Analysis

The file header at offset 0x10 contains a value that changes from 1 (empty) to 5 (filled with 5 tracks).

### Hypothesis
This field may represent the number of tracks in the database.

### Evidence
- Empty PDB: 0x10 = 1
- Filled PDB with 5 tracks: 0x10 = 5

### Action Required
Need to verify if this field is critical for loading. If so, ensure it's set correctly.

---

## Appendix C: Detailed Row Offset Array Mapping

The row group structure at the end of DATA pages uses a reverse-indexed array.

### Structure (36 bytes per group)
```
Offset  Size  Description
0x00    32    Row offsets (16 × u16, stored in REVERSE order)
0x20    2     Presence flags (bitmask, bit N = row N present)
0x22    2     Transaction flags (varies)
```

### Offset Mapping
For a row at index `i` within the group (0-15):
- Array position = `15 - i`
- Offset bytes = `group_start + (15-i)*2` to `group_start + (15-i)*2 + 2`

Example from Tracks DATA page:
```
Presence = 0x005E = 0b1011110
Rows present: 1, 2, 3, 4, 6 (not 0, not 5)

slot[15] → row 0: offset = 0x0000 (not present)
slot[14] → row 1: offset = 0x0170 ✓
slot[13] → row 2: offset = 0x02E0 ✓
slot[12] → row 3: offset = 0x043C ✓
slot[11] → row 4: offset = 0x0598 ✓
slot[10] → row 5: offset = 0x06F8 (not present, but has offset - deleted row?)
slot[9]  → row 6: offset = 0x086C ✓
```

---

## Appendix D: Pre-populated Tables Content Comparison

These tables have identical content in both empty and filled PDB:

| Table | Rows | Purpose |
|-------|------|---------|
| Colors | 8 | Standard color palette |
| Columns | 27 | Column definitions for UI |
| Unknown17 | 22 | Unknown (possibly sort orders?) |
| Unknown18 | 17 | Unknown (possibly related to Unknown17) |
| History | 1 (empty) / 6 (filled) | Play history |

### Key Insight
The pre-populated tables likely come from a template and should be copied verbatim rather than generated.

