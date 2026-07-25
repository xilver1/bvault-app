# Project Status

This is the single source of truth for what `rekord-export` does today. It
replaces the earlier `IMPLEMENTATION_STATUS.md`, `PDB_STATUS_REPORT.md`, and
`PDB_ANALYSIS_REPORT.md`, which described superseded states and had drifted out
of sync with the code. The byte-level format findings from those reports live
on in `EMPIRICAL_FINDINGS.md`, `FORMAT_SPECIFICATION.md`, and
`KAITAI_COMPLIANCE.md`.

Roughly **70% of the target feature set is implemented.** The export loads and
plays in rekordbox PC for the golden test case; the remaining work is listed
under "Open items".

## What works

### `export.pdb` (DeviceSQL)
All 20 tables are generated. The **"device library corrupted"** class of PDB
failures was traced to a malformed page-chain graph: every data page's
`next_page` (`0x0C`) was patched to the single global `next_unused`, so multiple
tables shared one successor. The fix gives each data table its own distinct
phantom `empty_candidate` page and terminates that table's chain there
(`PdbBuilder::build`). Populated tables: Tracks, Genres, Artists, Albums,
Labels, Keys, Colors, PlaylistTree, PlaylistEntries, Columns, Unknown17,
Unknown18, History, Artwork. The remaining table types are emitted as the empty
(all-zero) pages rekordbox expects.

Multi-page tables are handled — the row builders chain overflow pages via
`PageBuilder::would_overflow`/`finalize`, so exports are not limited to a single
data page per table.

### Device Library Plus
`device_library.rs` generates `exportLibrary.db` — a SQLCipher-encrypted SQLite
database (22-table schema, SQLCipher 4 defaults, universal static key) — plus
`DeviceLibBackup/rbDevLibBaInfo_<masterDbId>.json`. rekordbox PC validates this
layer independently of `export.pdb` and rejects the import without it, even when
the PDB is byte-perfect; CDJ hardware ignores it. This is why exports could pass
on a CDJ yet fail in rekordbox PC.

### ANLZ files
`.DAT`, `.EXT`, and `.2EX` are generated (`anlz.rs`) with the tag sets and
ordering observed in golden files. A systematic header bug — `len_header` was
written as "length after the tag" rather than the full header size — has been
corrected across the generators, along with the constant "magic" fields several
tags require. `PQT2` (extended beat grid) is deliberately **not** emitted: its
header carries a per-track value that has not been derived, and a wrong `PQT2`
is worse than none.

### rekordbox XML
`xml.rs` emits a full `DJ_PLAYLISTS` document from the same `TrackAnalysis`
structs the binary writers consume. It serves two purposes: a "tier 1" export
path for users who own rekordbox PC (rekordbox rebuilds the PDB itself on
import, so it is always valid), and the input side of the golden-file harness.

### Audio analysis
`rekordbox-server` decodes with Symphonia and derives BPM (autocorrelation with
parabolic peak interpolation and octave folding into a DJ range), a
constant-tempo beat grid, and three waveforms (PWAV preview, PWV4 colour
preview, PWV5 colour detail) via FFT band separation. Metadata is read from
embedded tags, falling back to the filename when tags are absent. Results are
cached on disk, keyed by file hash **and** an analyzer-version counter so
algorithm changes invalidate stale entries.

### Tooling
- `validate.rs` — structural validation of a generated PDB.
- `diff.rs` + the `pdbdiff` binary — page-aware byte diff of two PDBs that
  labels each page by table type and annotates known header fields; the fastest
  way to bisect a corruption cause against a golden file.
- Auxiliary files: `DEVSETTING.DAT`, `djprofile.nxs`.
- Navidrome/Subsonic client for playlist mapping.

## Open items

Ranked roughly by impact:

1. **rekordbox hangs ("Not Responding")** while loading some tracks onto a
   deck — suspected malformed ANLZ length/parse issue. Needs a structural scan
   of a freshly rebuilt export, field-by-field against golden.
2. **`exportExt.pdb` is not generated.** The golden file is currently copied in
   by hand after each export; a generator is needed (9 tables, same
   phantom/`next_unused` scheme as `export.pdb`).
3. **Key detection is not implemented.** The analyzer always sets `key = None`,
   so the Keys table is populated only in principle; no musical key is written
   in practice.
4. **Artwork is not generated.** `content.image_id` is left NULL and
   `PIONEER/Artwork/` is not populated.
5. **`.EXT`/`.2EX` validation on hardware** is still pending for the newer
   waveform tags, even though the header fixes are in place.
6. **History table** uses a hardcoded 40-byte row that works but is not fully
   understood, and the indexed-table first-entry values for Tracks/History are
   file-specific constants rather than a derived formula (see the notes in
   `page.rs`).

## Validation status

| Target                 | Status                                             |
|------------------------|----------------------------------------------------|
| rekordbox PC (import)  | Loads and plays for the golden test case           |
| Device-library check   | Passes (no "corrupted" error)                      |
| CDJ-2000 / Nexus / 3000| Format targeted; per-track hang open (see item 1)  |

## Where things live

| Concern                     | File                                   |
|-----------------------------|----------------------------------------|
| PDB build orchestration     | `rekordbox-core/src/pdb.rs`            |
| Page/table/header structs   | `rekordbox-core/src/page.rs`          |
| DeviceSQL string encoding   | `rekordbox-core/src/string.rs`        |
| ANLZ generation             | `rekordbox-core/src/anlz.rs`          |
| Device Library Plus         | `rekordbox-core/src/device_library.rs`|
| rekordbox XML               | `rekordbox-core/src/xml.rs`           |
| PDB validation / diff       | `validate.rs`, `diff.rs`, `bin/pdbdiff.rs` |
| Audio analysis              | `rekordbox-server/src/analyzer.rs`, `waveform.rs` |
| Export orchestration        | `rekordbox-server/src/export.rs`      |

## Reference documents

- `EMPIRICAL_FINDINGS.md` — verified golden-PDB byte findings (page flags,
  packed row counts, row groups, index entries).
- `FORMAT_SPECIFICATION.md` — directory layout and format overview.
- `KAITAI_COMPLIANCE.md` — where the implementation matches or intentionally
  diverges from the Kaitai `rekordbox_pdb.ksy` spec.
- `binary_analysis_rekordbox.md` — external RE landscape and toolchain guide.
