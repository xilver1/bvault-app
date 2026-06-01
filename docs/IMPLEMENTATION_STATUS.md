# rekord-export Implementation Status

**Date:** January 18, 2026  
**Version:** 2.9.1 (Track Row Critical Fixes)

---

## Executive Summary

Binary analysis of the generated PDB revealed **two critical bugs** in track row generation that were causing "Device library corrupted" errors:

1. **index_shift field was 0 instead of 0x20** - Caused string parsing failures
2. **4 extra bytes in track row** - Shifted all field offsets after 0x14

---

## Applied Fixes (v2.9.1)

### CRITICAL: Track Row index_shift Field
**Issue:** index_shift at offset 0x02 was `0x0000` (1-byte string offsets)  
**Fix:** Changed to `0x0020` (32) to indicate 2-byte string offsets  
**Impact:** Without this, rekordbox couldn't parse any track strings  
**Location:** `pdb.rs` line 938

### CRITICAL: Track Row Extra Bytes Removed
**Issue:** Code had extra u3 (2 bytes) and u4 (2 bytes) fields at 0x18-0x1B  
**Fix:** Removed these fields, fixed all subsequent field offsets  
**Impact:** All fields after offset 0x14 were shifted by 4 bytes, causing complete corruption  
**Location:** `pdb.rs` lines 955-970

### Field Offset Corrections
Old (wrong) → New (correct):
- artwork_id: 0x1C → 0x18
- key_id: 0x20 → 0x1C  
- orig_artist_id: 0x24 → 0x20
- label_id: 0x28 → 0x24
- remixer_id: 0x2C → 0x28
- bitrate: 0x30 → 0x2C
- track_number: 0x34 → 0x30
- tempo: 0x38 → 0x34
- genre_id: 0x3C → 0x38
- album_id: 0x40 → 0x3C
- artist_id: 0x44 → 0x40
- id: 0x48 → 0x44
- String offsets: 0x5E → 0x5A

### FIXED_SIZE Constant
**Issue:** Was 0x5E (94 bytes)  
**Fix:** Changed to 0x5A (90 bytes)  
**Impact:** String offset calculations were wrong

---

## Verified Correct (No Changes Needed)

### Page Flags (0x1B) ✅
- 0x64 for INDEX pages
- 0x34 for Tracks/History DATA pages  
- 0x24 for other DATA pages

### Unknown17 Row Size ✅
- 8 bytes per row (4×u16), NOT 16 bytes
- Kaitai spec is WRONG (says 4×u4)

### Unknown18 Row Size ✅
- 8 bytes per row (4×u16)

### Row Group Bytes 34-35 ✅
- Current approach (copy presence flags) is acceptable
- Golden shows these may differ for modified tables

### INDEX Page 0x2C Field ✅
- Points to DATA page when table has data
- 0x03FFFFFF when empty

---

## Known Ambiguities & Knowledge Gaps

### 1. DATA Page 0x20-0x23 Fields ⚠️
**Status:** Partially understood  
**Golden:** Tracks DATA shows 0x20=2, 0x22=5 (not matching row_count)  
**Current:** Sets 0x20=row_count, 0x22=0  
**Impact:** Unknown - may need investigation if corruption persists

### 2. INDEX First Entry Value (0x3C) ⚠️
**Status:** Unclear semantics  
**Golden:** Tracks INDEX has 0x00000010 (16)  
**Current:** Writes num_row_offsets (20 for 5 tracks)  
**Impact:** Likely low - may be ignored by rekordbox

### 3. Packed Row Count Anomalies ⚠️
**Status:** Observed but unexplained  
**Golden:** Tracks and History don't follow 4:1 ratio  
**Hypothesis:** Related to deletion semantics (num_rows = highest index + 1)  
**Impact:** Likely low for fresh exports with no deletions

### 4. History Table Row Structure ⚠️
**Status:** Partially documented  
**DeepSymmetry:** "Not yet studied"  
**Current:** Using hardcoded 40-byte structure from observed export  
**Impact:** Works but structure not fully understood

### 5. Pre-populated Table Content ⚠️
**Status:** Static data copied from rekordbox  
**Tables:** Colors, Columns, Unknown17, Unknown18  
**Issue:** Data extracted from rekordbox 6.8 - may differ across versions  
**Impact:** Should work, but untested across versions

---

## Not Yet Implemented

### Multi-Page Tables
**Status:** Single DATA page per table  
**Needed When:** >~50 tracks (page overflow)  
**Complexity:** Medium - need proper page chaining

### Dynamic Key Table
**Status:** Key table is always empty  
**Issue:** Key values not being looked up/populated  
**Impact:** Key metadata won't display on CDJs

### Artwork Linking
**Status:** Artwork paths stored but files not copied  
**Needed:** Copy artwork files to PIONEER/Artwork/

### ANLZ File Generation
**Status:** Separate module exists but not integrated  
**Files:** .DAT, .EXT, .2EX  
**Needed For:** Waveforms, beat grids, hot cues on CDJs

---

## Testing Checklist

### Minimum Viable Test
- [ ] Generate PDB with 5 tracks
- [ ] Load in rekordbox 6.x PC software
- [ ] Verify no "Device library corrupted" error
- [ ] Verify tracks appear in track list

### Functionality Test
- [ ] Artist/Album/Genre metadata displays
- [ ] Playlist structure loads
- [ ] Track playback works on CDJ

### Compatibility Test
- [ ] rekordbox 6.x PC software ✓/✗
- [ ] CDJ-2000NXS2 ✓/✗
- [ ] CDJ-3000 ✓/✗
- [ ] XDJ-XZ ✓/✗

---

## File Format Quick Reference

### PDB File Structure
```
Page 0: File Header
  0x04: len_page (4096)
  0x08: num_tables (20)
  0x0C: next_unused_page
  0x10: track_count ← EMPIRICALLY VERIFIED
  0x14: sequence
  0x1C+: table_pointers[20]

Each Table:
  INDEX Page (flags 0x64):
    0x26: NextOffset (1 if has indexed data, else 0)
    0x2C: DATA page or 0x03FFFFFF
    0x38: NumEntries (only Tracks/History have >0)
    0x3C+: Index entries or 0x1FFFFFF8 fill
  
  DATA Page (flags 0x24/0x34):
    0x18-0x1A: packed = (num_offsets << 11) | num_rows
    0x1B: flags (0x34 for Tracks/History, 0x24 others)
    0x28+: heap data (rows)
    End-36n: row groups (36 bytes each)
```

### Row Group Structure (36 bytes)
```
Bytes 0-31:  row_offsets[16] (u16, REVERSE order)
Bytes 32-33: presence_flags (bitmask)
Bytes 34-35: transaction_flags (copy presence for new data)
```

---

## Next Steps (Priority Order)

1. **Test with rekordbox PC** - Verify corruption fix works
2. **Debug if still failing** - Compare generated vs golden byte-by-byte
3. **Implement Key table** - Enable key metadata display
4. **Add multi-page support** - Handle larger track counts
5. **Integrate ANLZ generation** - Enable waveforms on CDJs

---

## Resources

- **DeepSymmetry Docs:** https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/
- **Kaitai Spec:** https://github.com/Deep-Symmetry/crate-digger/tree/main/src/main/kaitai
- **REX (Go):** Reference implementation for PDB generation
- **rekordcrate (Rust):** binrw-based parsing (read support)
