# rekord-export

Rust tooling that generates Pioneer rekordbox-compatible USB exports directly
from a music library — no PC running rekordbox required. The goal is exports
that both **rekordbox PC** (the strictest validator) and **CDJ-2000-and-newer**
hardware accept.

Pioneer's export formats are undocumented and proprietary; this project builds
them from the byte-level reverse engineering by
[Deep Symmetry](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/)
plus a golden-file harness (see `docs/`).

> **Status:** ~70% of the target feature set is implemented. `export.pdb`, the
> Device Library Plus layer, ANLZ (`.DAT`/`.EXT`/`.2EX`), and the rekordbox XML
> path all generate; several items remain open. See
> [`docs/STATUS.md`](docs/STATUS.md) for the current picture.

## Architecture

```
┌─────────────────┐        TCP / JSON        ┌─────────────────┐
│  rekordbox-cli  │ ◄──────────────────────► │ rekordbox-server│
│   (Termux)      │      (line protocol)     │   (NAS / x86)   │
└─────────────────┘                          └────────┬────────┘
                                                      │
                                             ┌────────▼────────┐
                                             │  rekordbox-core │
                                             │ (PDB/ANLZ/XML/  │
                                             │  device library)│
                                             └─────────────────┘
```

- **rekordbox-core** — pure-Rust library that generates every Pioneer artifact:
  `export.pdb` (DeviceSQL), ANLZ files, the encrypted device library, the
  rekordbox XML, and the auxiliary device files. Also provides a validator and a
  page-aware golden-file diff (`pdbdiff` binary).
- **rekordbox-server** — audio analysis (BPM, beat grid, waveforms via
  Symphonia + FFT) and export orchestration; runs as a daemon on the NAS or does
  a one-shot direct export. Optional Navidrome/Subsonic playlist integration.
- **rekordbox-cli** — lightweight client (built for Termux on Android) that
  drives the server over TCP.

## USB structure generated

```
USB_ROOT/
├── PIONEER/
│   ├── rekordbox/
│   │   ├── export.pdb          # DeviceSQL track database
│   │   ├── exportLibrary.db    # Device Library Plus (SQLCipher-encrypted)
│   │   └── rekord-export.xml   # rekordbox XML (import + golden-file harness)
│   ├── USBANLZ/Pxxx/xxxxxxxx/
│   │   ├── ANLZ0000.DAT        # beat grid, preview waveforms (all CDJs)
│   │   ├── ANLZ0000.EXT        # detail/colour waveforms, cues (Nexus+)
│   │   └── ANLZ0000.2EX        # 3-band analysis (CDJ-3000)
│   ├── DeviceLibBackup/
│   │   └── rbDevLibBaInfo_<id>.json
│   ├── Artwork/                # (reserved; artwork generation is not wired up)
│   ├── DEVSETTING.DAT
│   └── djprofile.nxs
└── Contents/
    └── *.mp3, *.flac, ...      # audio files (flat + Artist/Album/ hierarchy)
```

## Building

```bash
cargo build --release                 # all crates
cargo build --release -p rekordbox-cli # CLI only (for Termux)
cargo build --release -p rekordbox-cli --target aarch64-linux-android

cargo test --workspace                # run the test suite
```

Building `rekordbox-core` compiles a vendored SQLCipher (for the device
library), which requires a C toolchain plus **Perl** and **NASM** on the host.

## Usage

### Direct export (no server)

```bash
rekordbox-server --music-dir /path/to/music --export /media/usb
```

### Server mode

On the NAS:

```bash
rekordbox-server --music-dir /mnt/ssd/pre-export --bind 0.0.0.0:6969
```

From Termux (or any client):

```bash
rekordbox status        # server health
rekordbox analyze       # analyze the library
rekordbox export /storage/usb
rekordbox list          # list analyzed tracks
rekordbox cache-stats
rekordbox cache-clear
```

Navidrome playlists can drive the export by passing `--navidrome-url`,
`--navidrome-user`, and `--navidrome-pass` (or the matching `NAVIDROME_*`
environment variables); otherwise playlists are inferred from folder structure.

## Format notes

**`export.pdb`** — DeviceSQL: 4096-byte pages, little-endian, all 20 tables
present. The row heap grows forward from `0x28`; the row-offset index grows
backward from the page end. Strings use the DeviceSQL short-ASCII / long-ASCII /
UTF-16LE encodings.

**ANLZ (`.DAT`/`.EXT`/`.2EX`)** — big-endian tagged sections: `PPTH` (path),
`PQTZ` (beat grid), `PWAV`/`PWV2` (preview waveforms), `PWV3`–`PWV7` (colour and
3-band waveforms), `PCOB`/`PCO2` (cues), `PVBR` (VBR seek index).

**Device Library Plus** — `exportLibrary.db` is a SQLCipher-encrypted SQLite DB
(22 tables) that rekordbox PC validates independently of `export.pdb`; without
it rekordbox reports "device library corrupted" even for a byte-perfect PDB.
CDJs ignore this layer.

## Testing without CDJ hardware

- **rekordbox PC** — the strictest validator; the primary compatibility target.
- **rekordcrate** — `rekordcrate dump-pdb export.pdb` to inspect a generated PDB.
- **Kaitai Web IDE** — visual binary inspection at `ide.kaitai.io`.
- **`pdbdiff`** — `cargo run -p rekordbox-core --bin pdbdiff -- golden.pdb mine.pdb`
  localises every differing byte to a page and annotates known header fields.

## References

- [Deep Symmetry — rekordbox export analysis](https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/exports.html)
- [crate-digger](https://github.com/Deep-Symmetry/crate-digger) — Kaitai specs + docs
- [rekordcrate](https://github.com/Holzhaus/rekordcrate) — Rust PDB/ANLZ library
- [REX](https://github.com/kimtore/rex) — Go PDB generation reference
- [pyrekordbox](https://github.com/dylanljones/pyrekordbox) — Python library

## License

MIT
