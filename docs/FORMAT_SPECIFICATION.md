# Pioneer rekordbox USB Export Format Specification

> **Reference document** describing the on-disk formats. For which parts are
> implemented and what is still open, see `STATUS.md`.

Based on reverse engineering of actual rekordbox 6.8.4 USB exports and Deep Symmetry documentation.

## Directory Structure

```
USB_ROOT/
├── PIONEER/
│   ├── rekordbox/
│   │   ├── export.pdb        # Main DeviceSQL database (REQUIRED)
│   │   ├── exportExt.pdb     # Extended database (OPTIONAL)
│   │   └── exportLibrary.db  # Encrypted SQLCipher database
│   ├── USBANLZ/
│   │   ├── P000/
│   │   │   └── 00000001/
│   │   │       ├── ANLZ0000.DAT   # Basic analysis
│   │   │       ├── ANLZ0000.EXT   # Extended analysis (NXS/NXS2)
│   │   │       └── ANLZ0000.2EX   # Second extended (CDJ-3000)
│   │   ├── P001/
│   │   └── P0xx/...
│   ├── Artwork/
│   │   ├── 00001/
│   │   │   ├── a1.jpg         # 80x80 thumbnail
│   │   │   └── a1_m.jpg       # 240x240 full
│   │   └── ...
│   ├── DEVSETTING.DAT         # Device settings (140 bytes)
│   └── djprofile.nxs          # DJ profile name (160 bytes)
└── Contents/
    ├── [audio files]          # Flat at root level
    ├── [Artist]/
    │   └── [Album]/
    │       └── [audio files]  # Hierarchical by metadata
    └── ...
```

## export.pdb File Header (Page 0)

The PDB file header is a 4096-byte page with the following structure:

| Offset | Size | Description |
|--------|------|-------------|
| 0x00 | 4 | Zero padding |
| 0x04 | 4 | Page size (always 4096 = 0x1000) |
| 0x08 | 4 | Number of tables (20) |
| 0x0C | 4 | Next unused page index |
| 0x10 | 320 | Table pointers (20 × 16 bytes) |
| 0x150+ | 3760 | Zero padding |

### Table Pointer Format (16 bytes each)

Each table pointer describes one of the 20 table types:

| Offset | Size | Description |
|--------|------|-------------|
| 0x00 | 4 | First page index |
| 0x04 | 4 | Empty candidate (next available page) |
| 0x08 | 4 | Last page index |
| 0x0C | 4 | Table type (0-19) |

**IMPORTANT**: The table_type is in the LAST position, not the first!

Tables are stored in order by type (0, 1, 2, ..., 19).

## DEVSETTING.DAT Format (140 bytes) ✅

| Offset | Size | Description |
|--------|------|-------------|
| 0x00 | 4 | Size/Header (0x60 = 96) |
| 0x04 | 28 | Brand "PIONEER DJ" |
| 0x24 | 32 | Application "rekordbox" |
| 0x44 | 32 | Version "6.8.4" |
| 0x64 | 4 | Section marker (0x00000020) |
| 0x68 | 4 | Magic (0x12345678) |
| 0x6C | 4 | Unknown (0x00000001) |
| 0x70 | 16 | Settings flags |
| 0x80 | 12 | Tail data |

## djprofile.nxs Format (160 bytes) ✅

| Offset | Size | Description |
|--------|------|-------------|
| 0x00 | 32 | Zero padding |
| 0x20 | 32 | DJ Profile name |
| 0x40 | 96 | Zero padding |

## export.pdb Tables

| Type | Name | Status |
|------|------|--------|
| 0 | Tracks | ✅ |
| 1 | Genres | ✅ |
| 2 | Artists | ✅ |
| 3 | Albums | ✅ |
| 4 | Labels | ✅ |
| 5 | Keys | ✅ |
| 6 | Colors | ✅ |
| 7 | PlaylistTree | ✅ |
| 8 | PlaylistEntries | ✅ |
| 13 | Artwork | ✅ |

## Data Page Structure

Each data page (4096 bytes) has:

| Offset | Size | Description |
|--------|------|-------------|
| 0x00 | 4 | Zero padding |
| 0x04 | 4 | Page index |
| 0x08 | 4 | Page type (0-19) |
| 0x0C | 4 | Next page index |
| 0x10 | 4 | Unknown1 |
| 0x14 | 4 | Unknown2 (transaction counter?) |
| 0x18 | 3 | Packed row counts |
| 0x1B | 1 | Page flags (0x24=data, 0x34=track data, 0x64=index) |
| 0x1C | 2 | Free size |
| 0x1E | 2 | Used size |
| 0x20 | 8+ | Index-specific or data-specific header |
| 0x28+ | ? | Row data (heap, grows forward) |
| End-36 | 36 | Row group (offsets + presence flags) |

## ANLZ File Tags

### DAT File ✅
- **PMAI**: File header
- **PPTH**: File path (UTF-16BE)
- **PQTZ**: Beat grid
- **PWAV**: Preview waveform (400 bytes)
- **PWV5**: Detail color waveform (2 bytes/entry)

### EXT File ✅
All DAT sections plus:
- **PWV3**: 3-band waveform (1 byte/entry)
- **PWV4**: Color preview (1200×6 bytes)
- **PCOB**: Basic cue points
- **PCO2**: Extended cues with colors

### 2EX File ✅
Same as EXT for CDJ-3000+

## PWV5 Color Detail (2 bytes, big-endian)

| Bits | Field |
|------|-------|
| 15-13 | Red (0-7) |
| 12-10 | Green (0-7) |
| 9-7 | Blue (0-7) |
| 6-2 | Height (0-31) |

## PCO2 Hot Cue Colors (63-color palette)

| Index | Color | RGB |
|-------|-------|-----|
| 0x00 | Green | #28E214 |
| 0x09 | Cyan | #00E0FF |
| 0x22 | Orange | #FFA000 |
| 0x2A | Red | #E62828 |
| 0x32 | Yellow | #FFFF00 |
| 0x3E | Purple | #6473FF |

## Notes

- PDB files: **little-endian**
- ANLZ files: **big-endian**
- All essential features implemented for CDJ-2000NXS2, CDJ-3000, XDJ-RX3
