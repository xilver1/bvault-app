# Kaitai Spec Compliance

> **Reference document.** Records where the PDB implementation matches or
> intentionally diverges from the Kaitai spec. For current project status see
> `STATUS.md`.

This document verifies that the PDB implementation matches the Kaitai struct specification (`rekordbox_pdb.ksy`) from Deep-Symmetry/crate-digger.

## Page Types (CRITICAL FIX)

The page type enum was **INCORRECT** and has been fixed:

| Type | Kaitai Spec Name    | Old Value | Corrected Value |
|------|---------------------|-----------|-----------------|
| 0    | tracks              | 0 ✓       | 0               |
| 1    | genres              | 1 ✓       | 1               |
| 2    | artists             | 2 ✓       | 2               |
| 3    | albums              | 3 ✓       | 3               |
| 4    | labels              | 4 ✓       | 4               |
| 5    | keys                | 5 ✓       | 5               |
| 6    | colors              | 6 ✓       | 6               |
| 7    | playlist_tree       | 7 ✓       | 7               |
| 8    | playlist_entries    | 8 ✓       | 8               |
| 9    | unknown_9           | 9 ✓       | 9               |
| 10   | unknown_10          | 10 ✓      | 10              |
| 11   | **history_playlists** | ~~13~~ | **11**          |
| 12   | **history_entries**   | ~~14~~ | **12**          |
| 13   | **artwork**           | ~~15~~ | **13**          |
| 14   | unknown_14          | ~~11~~    | **14**          |
| 15   | unknown_15          | ~~12~~    | **15**          |
| 16   | **columns**           | ~~17~~ | **16**          |
| 17   | **uk17**              | N/A    | **17**          |
| 18   | unknown_18          | ~~SortOrders~~ | **18**     |
| 19   | **history**           | ~~Unknown19~~ | **19**    |

## File Header (Page 0)

| Offset | Size | Field           | Implementation | Status |
|--------|------|-----------------|----------------|--------|
| 0x00   | 4    | zero            | zeros          | ✓      |
| 0x04   | 4    | len_page        | 4096           | ✓      |
| 0x08   | 4    | num_tables      | 20             | ✓      |
| 0x0C   | 4    | next_unused_page| dynamic        | ✓      |
| 0x10   | 4    | unknown         | 5              | ✓      |
| 0x14   | 4    | sequence        | 1              | ✓      |
| 0x18   | 4    | gap             | zeros          | ✓      |
| 0x1C   | 16×N | tables          | TablePointer[] | ✓      |

## Table Pointer (16 bytes)

| Offset | Size | Field           | Implementation | Status |
|--------|------|-----------------|----------------|--------|
| 0x00   | 4    | type            | PageType enum  | ✓      |
| 0x04   | 4    | empty_candidate | next available | ✓      |
| 0x08   | 4    | first_page      | index page     | ✓      |
| 0x0C   | 4    | last_page       | last data page | ✓      |

## Page Header (40 bytes = 0x28)

| Offset | Size | Field           | Implementation | Status |
|--------|------|-----------------|----------------|--------|
| 0x00   | 4    | gap             | zeros          | ✓      |
| 0x04   | 4    | page_index      | page number    | ✓      |
| 0x08   | 4    | type            | PageType       | ✓      |
| 0x0C   | 4    | next_page       | next index     | ✓      |
| 0x10   | 4    | sequence        | 0              | ✓      |
| 0x14   | 4    | unknown         | zeros          | ✓      |
| 0x18   | 1    | num_rows_small  | row count ≤255 | ✓      |
| 0x19   | 1    | bitmask         | 0              | ✓      |
| 0x1A   | 1    | unknown         | 0              | ✓      |
| 0x1B   | 1    | page_flags      | 0x24/0x34/0x64 | ✓      |
| 0x1C   | 2    | free_size       | calculated     | ✓      |
| 0x1E   | 2    | used_size       | calculated     | ✓      |
| 0x20   | 2    | unknown         | 0              | ✓      |
| 0x22   | 2    | num_rows_large  | row count >255 | ✓      |
| 0x24   | 2    | unknown         | 0              | ✓      |
| 0x26   | 2    | unknown         | 0              | ✓      |

Heap starts at 0x28.

## Row Group Structure (36 bytes = 0x24)

Built backwards from end of page:

| Offset in Group | Size | Field             | Implementation | Status |
|-----------------|------|-------------------|----------------|--------|
| 0x00-0x1F       | 32   | row_offsets[16]   | reversed order | ✓      |
| 0x20-0x21       | 2    | row_present_flags | bitmask        | ✓      |
| 0x22-0x23       | 2    | padding           | zeros          | ✓      |

## Row Structures

### track_row
| Offset | Size | Field             | Status |
|--------|------|-------------------|--------|
| 0x00   | 2    | magic (0x0024)    | ✓      |
| 0x02   | 2    | index_shift       | ✓      |
| 0x04   | 4    | bitmask           | ✓      |
| 0x08   | 4    | sample_rate       | ✓      |
| 0x0C   | 4    | composer_id       | ✓      |
| 0x10   | 4    | file_size         | ✓      |
| 0x14   | 4    | unknown           | ✓      |
| 0x18   | 2    | unknown           | ✓      |
| 0x1A   | 2    | unknown           | ✓      |
| 0x1C   | 4    | artwork_id        | ✓      |
| 0x20   | 4    | key_id            | ✓      |
| 0x24   | 4    | original_artist_id| ✓      |
| 0x28   | 4    | label_id          | ✓      |
| 0x2C   | 4    | remixer_id        | ✓      |
| 0x30   | 4    | bitrate           | ✓      |
| 0x34   | 4    | track_number      | ✓      |
| 0x38   | 4    | tempo (BPM×100)   | ✓      |
| 0x3C   | 4    | genre_id          | ✓      |
| 0x40   | 4    | album_id          | ✓      |
| 0x44   | 4    | artist_id         | ✓      |
| 0x48   | 4    | id                | ✓      |
| 0x4C   | 2    | disc_number       | ✓      |
| 0x4E   | 2    | play_count        | ✓      |
| 0x50   | 2    | year              | ✓      |
| 0x52   | 2    | sample_depth      | ✓      |
| 0x54   | 2    | duration          | ✓      |
| 0x56   | 2    | unknown (41)      | ✓ FIXED |
| 0x58   | 1    | color_id          | ✓      |
| 0x59   | 1    | rating            | ✓      |
| 0x5A   | 2    | unknown (1)       | ✓ FIXED |
| 0x5C   | 2    | unknown (2/3)     | ✓      |
| 0x5E   | 42   | ofs_strings[21]   | ✓      |

### Other Row Structures
- **artist_row**: subtype + index_shift + id + marker + ofs_name ✓
- **album_row**: magic + index_shift + unknown + artist_id + id + unknown + marker + ofs_name ✓
- **genre_row**: id + name_string ✓
- **key_row**: id + id2 + name_string ✓
- **label_row**: id + name_string ✓
- **color_row**: 5 unknown + id(u16) + 1 unknown + name_string ✓
- **playlist_tree_row**: parent_id + unknown + sort_order + id + is_folder + name ✓
- **playlist_entry_row**: entry_index + track_id + playlist_id ✓
- **artwork_row**: id + path_string ✓
- **uk17_row**: Using REX format (4×u16=8 bytes) - note: Kaitai says 4×u4=16 bytes

## DeviceSQL String Encoding

| Type | Flag | Format | Status |
|------|------|--------|--------|
| Short ASCII | odd | header = ((len+1)<<1)\|1, then chars | ✓ |
| Long ASCII | 0x40 | 0x40 + len(u16) + 0x00 + chars | ✓ |
| UTF-16LE | 0x90 | 0x90 + len(u16) + 0x00 + utf16 | ✓ |

## Changes Made

1. **PageType enum values corrected** (was completely wrong for types 11-19)
2. **Page header restructured** to match exact byte offsets from Kaitai
3. **Track row fields fixed** at offsets 0x56 and 0x5A
4. **Unknown17/Unknown18 tables added** with REX-compatible data
5. **History table (type 19) added** as empty
