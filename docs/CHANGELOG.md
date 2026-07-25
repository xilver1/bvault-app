# Changelog

The `vX.Y` labels below are internal development milestones, not published
releases — the crate version is `0.1.0`. See `STATUS.md` for the current state.

## Device Library Plus, ANLZ, and page-chain fixes (current)

The milestones that moved the project from "CDJ-only, rejected by rekordbox PC"
to "loads and plays in rekordbox PC":

- **`export.pdb` page-chain fix.** Root cause of the "device library corrupted"
  class of PDB failures: every data page's `next_page` (`0x0C`) was patched to
  one global `next_unused`, making the page-chain graph malformed. Each data
  table now terminates its chain at its own distinct phantom `empty_candidate`
  (`pdb.rs`).
- **Device Library Plus.** New `device_library.rs` generates the
  SQLCipher-encrypted `exportLibrary.db` (22-table schema) and its backup JSON.
  rekordbox PC validates this layer independently of `export.pdb`; without it
  the import is rejected even for a byte-perfect PDB.
- **ANLZ `len_header` fix.** Corrected a systematic bug where `len_header` was
  written as "length after the tag" instead of the full header size, plus the
  constant magic fields several tags require, across `.DAT`/`.EXT`/`.2EX`.
- **rekordbox XML export** as a tier-1 path and golden-file harness input.

## Codebase cleanup

- Removed dead/superseded code (a duplicate device-backup-JSON implementation,
  unused builder helpers, an orphaned struct, redundant page/string helpers),
  unused imports and variables, and a `[profile.release]` block that cargo
  silently ignored in a non-root crate.
- Repaired the test suite, which no longer compiled after struct fields were
  added (missing `label`/`color` in fixtures); it now builds and passes.
- Extracted a shared `band_energy` helper in `waveform.rs`, verified to produce
  byte-identical waveform output.

## v2.7.2 - Track Bitmask Fix

### Bug Fixes

**1. Color Row Structure (pdb.rs)**
- **Issue**: Byte 4 was `0x00` instead of the color ID
- **Fix**: Byte 4 now equals byte 5 (both contain the color ID)

**2. uk17 Row Size (pdb.rs)**
- **Issue**: Rows were 16 bytes (4 × u32)
- **Fix**: Rows are now 8 bytes (4 × u16)

**3. Row Group Padding (page.rs)**  
- **Issue**: Bytes 34-35 were left as zeros
- **Fix**: Bytes 34-35 now duplicate presence_flags

**4. num_row_offsets Calculation (page.rs)**
- **Issue**: Was set to actual row count
- **Fix**: Must be `num_rows × 4` (rekordbox requirement)

**5. Row 4-Byte Alignment (page.rs)**
- **Issue**: Rows were not aligned, causing offset mismatches
- **Fix**: Each row is now padded to 4-byte boundary

**6. Track Bitmask (pdb.rs) - NEW**
- **Issue**: bitmask was 0x00000000
- **Fix**: bitmask is now 0x000C0700 (standard rekordbox 6.x value)
- **Impact**: This field controls string field presence validation

## v2.7.3 (Fix 7) - Index Page Active Table Fields

**Bug**: Rekordbox 6+ validates index page fields for "active" tables (Tracks, History)
and rejects files where these fields are not properly set.

**Golden Reference Analysis:**
- Tracks index (page 1): version=0x12, 0x24=0x000103EC, 0x38=0x1FFF0001, 0x3C=num_row_offsets
- Other tables: version=0x01, 0x24=0x000003EC, 0x38=0x1FFF0000, 0x3C=0x1FFFFFF8

**Root Cause**: Our IndexPageBuilder used static values (version=1, 0x24=0x03EC, etc.)
for all tables, but Tracks and History require:
- version = 0x12 (18) instead of 1
- 0x26-0x27 = 1 (active flag)
- 0x38 low word = 1 (num_entries)
- 0x3C = first index entry (num_row_offsets from data page)

**Fix Applied (page.rs, pdb.rs):**
```rust
// In IndexPageBuilder::finalize():
let is_active_table = matches!(self.page_type, PageType::Tracks | PageType::History);
let version = if is_active_table && has_data { 0x12u32 } else { 1u32 };
let active_flag = if is_active_table && has_data { 1u16 } else { 0u16 };
let num_entries = if is_active_table && has_data { 1u16 } else { 0u16 };

// 0x3C gets num_row_offsets instead of fill pattern for active tables
```

**Files Modified:**
- `rekordbox-core/src/page.rs`: IndexPageBuilder::finalize() updated with active table logic
- `rekordbox-core/src/pdb.rs`: build_table() extracts num_row_offsets and passes to index builder

**Verification:**
- Tracks index page key fields now match golden reference exactly
- version, field_24, field_38, field_3c all correct for active tables
