//! Device Library Plus generation (rekordbox 6.6.5+)
//!
//! rekordbox PC reads a *device library* layer that is independent of
//! `export.pdb`. When this layer is missing or invalid, rekordbox reports
//! "Device library is corrupted" on import — even when `export.pdb` is
//! structurally perfect (CDJ hardware ignores this layer, which is why CDJs
//! accept the export while rekordbox PC rejects it).
//!
//! This module generates the two files that make up that layer:
//! - `PIONEER/rekordbox/exportLibrary.db` — a SQLCipher-encrypted SQLite DB
//! - `PIONEER/DeviceLibBackup/rbDevLibBaInfo_<masterDbId>.json`
//!
//! ## Encryption
//! The database uses SQLCipher 4 defaults (AES-256-CBC, PBKDF2-HMAC-SHA512,
//! 256000 iterations, HMAC-SHA512, 4096-byte pages) with a *universal static
//! passphrase* — the same key rekordbox uses for `master.db`. Applying
//! `PRAGMA key` alone reproduces the golden file's parameters; no custom
//! cipher pragmas are required.
//!
//! Schema and row values were reverse-engineered from a golden 1-track export
//! (rekordbox 6.8.4) after extracting the key via x64dbg (`sqlite3_key` hook).

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::{Error, Result};
use crate::track::TrackAnalysis;

/// Universal static SQLCipher passphrase for the device library.
///
/// Captured from rekordbox 6.8.4 via `sqlite3_key`. Shared with `master.db`;
/// not device-specific. If a future rekordbox version rotates this, it must be
/// re-captured with the same debugger hook.
const DEVICE_LIBRARY_KEY: &str = "r8gddnr4k847830ar6cqzbkk0el6qytmb3trbbx805jm74vez64i5o8fnrqryqls";

/// Device library schema version string written to `property.dbVersion`.
const DB_VERSION: &str = "10000";

// ---------------------------------------------------------------------------
// Static template tables (copied verbatim from the golden export)
//
// These define fixed browser/UI behaviour on the player and are identical
// across exports. They are NOT derived from user content.
// ---------------------------------------------------------------------------

/// (color_id, name)
const COLORS: &[(i64, &str)] = &[
    (1, "Pink"),
    (2, "Red"),
    (3, "Orange"),
    (4, "Yellow"),
    (5, "Green"),
    (6, "Aqua"),
    (7, "Blue"),
    (8, "Purple"),
];

/// (menuItem_id, kind, name)
///
/// Names are wrapped in the U+FFFA / U+FFFB interlinear-annotation sentinels,
/// exactly as rekordbox stores localizable menu labels.
const MENU_ITEMS: &[(i64, i64, &str)] = &[
    (1, 128, "\u{fffa}GENRE\u{fffb}"),
    (2, 129, "\u{fffa}ARTIST\u{fffb}"),
    (3, 130, "\u{fffa}ALBUM\u{fffb}"),
    (4, 131, "\u{fffa}TRACK\u{fffb}"),
    (5, 133, "\u{fffa}BPM\u{fffb}"),
    (6, 134, "\u{fffa}RATING\u{fffb}"),
    (7, 135, "\u{fffa}YEAR\u{fffb}"),
    (8, 136, "\u{fffa}REMIXER\u{fffb}"),
    (9, 137, "\u{fffa}LABEL\u{fffb}"),
    (10, 138, "\u{fffa}ORIGINAL ARTIST\u{fffb}"),
    (11, 139, "\u{fffa}KEY\u{fffb}"),
    (12, 141, "\u{fffa}CUE\u{fffb}"),
    (13, 142, "\u{fffa}COLOR\u{fffb}"),
    (14, 146, "\u{fffa}TIME\u{fffb}"),
    (15, 147, "\u{fffa}BITRATE\u{fffb}"),
    (16, 148, "\u{fffa}FILE NAME\u{fffb}"),
    (17, 132, "\u{fffa}PLAYLIST\u{fffb}"),
    (18, 152, "\u{fffa}HOT CUE BANK\u{fffb}"),
    (19, 149, "\u{fffa}HISTORY\u{fffb}"),
    (20, 145, "\u{fffa}SEARCH\u{fffb}"),
    (21, 150, "\u{fffa}COMMENTS\u{fffb}"),
    (22, 140, "\u{fffa}DATE ADDED\u{fffb}"),
    (23, 151, "\u{fffa}DJ PLAY COUNT\u{fffb}"),
    (24, 144, "\u{fffa}FOLDER\u{fffb}"),
    (25, 161, "\u{fffa}DEFAULT\u{fffb}"),
    (26, 162, "\u{fffa}ALPHABET\u{fffb}"),
    (27, 170, "\u{fffa}MATCHING\u{fffb}"),
];

/// (category_id, menuItem_id, sequenceNo, isVisible)
const CATEGORIES: &[(i64, i64, i64, i64)] = &[
    (1, 1, 0, 0),
    (2, 2, 1, 1),
    (3, 3, 2, 1),
    (4, 4, 3, 1),
    (5, 17, 5, 1),
    (6, 5, 0, 0),
    (7, 6, 0, 0),
    (8, 7, 0, 0),
    (9, 8, 0, 0),
    (10, 9, 0, 0),
    (11, 10, 0, 0),
    (12, 11, 4, 1),
    (15, 13, 0, 0),
    (17, 24, 9, 1),
    (18, 20, 7, 1),
    (19, 14, 0, 0),
    (20, 15, 0, 0),
    (21, 16, 0, 0),
    (22, 19, 6, 1),
    (23, 18, 0, 0),
    (26, 27, 8, 1),
    (27, 22, 10, 1),
];

/// (sort_id, menuItem_id, sequenceNo, isVisible, isSelectedAsSubColumn)
const SORTS: &[(i64, i64, i64, i64, i64)] = &[
    (0, 25, 1, 1, 0),
    (1, 26, 2, 1, 0),
    (2, 2, 3, 1, 0),
    (3, 3, 4, 1, 0),
    (4, 5, 5, 1, 0),
    (5, 6, 6, 1, 0),
    (6, 1, 0, 0, 0),
    (7, 21, 0, 0, 0),
    (8, 14, 0, 0, 0),
    (9, 8, 0, 0, 0),
    (10, 9, 0, 0, 0),
    (11, 10, 0, 0, 0),
    (12, 11, 7, 1, 0),
    (13, 15, 0, 0, 0),
    (15, 13, 0, 0, 0),
    (16, 23, 0, 0, 0),
    (17, 22, 0, 0, 0),
];

/// (myTag_id, sequenceNo, name, attribute, myTag_id_parent)
///
/// Only the four built-in root categories are seeded. The golden file also
/// contained ~24 *user-created* child tags specific to the source library;
/// those are intentionally omitted since they are not structural.
const MYTAG_ROOTS: &[(i64, i64, &str, i64, i64)] = &[
    (1, 0, "Genre", 1, 0),
    (2, 1, "Components", 1, 0),
    (3, 2, "Situation", 1, 0),
    (4, 3, "Untitled Column", 1, 0),
];

/// The 22 `CREATE TABLE` / `CREATE INDEX` statements, verbatim from the golden
/// schema (`sqlite_master.sql`).
const SCHEMA_DDL: &[&str] = &[
    "CREATE TABLE album(album_id integer primary key, name varchar, artist_id integer, image_id integer, isComplation integer, nameForSearch varchar)",
    "CREATE TABLE artist(artist_id integer primary key, name varchar, nameForSearch varchar)",
    "CREATE TABLE category(category_id integer primary key, menuItem_id integer, sequenceNo integer, isVisible integer)",
    "CREATE TABLE color(color_id integer primary key, name varchar)",
    "CREATE TABLE content(content_id integer primary key, title varchar, titleForSearch varchar, subtitle varchar, bpmx100 integer, length integer, trackNo integer, discNo integer, artist_id_artist integer, artist_id_remixer integer, artist_id_originalArtist integer, artist_id_composer integer, artist_id_lyricist integer, album_id integer, genre_id integer, label_id integer, key_id integer, color_id integer, image_id integer, djComment varchar, rating integer, releaseYear integer, releaseDate varchar, dateCreated varchar, dateAdded varchar, path varchar, fileName varchar, fileSize integer, fileType integer, bitrate integer, bitDepth integer, samplingRate integer, isrc varchar, djPlayCount integer, isHotCueAutoLoadOn integer, isKuvoDeliverStatusOn integer, kuvoDeliveryComment varchar, masterDbId integer, masterContentId integer, analysisDataFilePath varchar, analysedBits integer, contentLink integer, hasModified integer, cueUpdateCount integer, analysisDataUpdateCount integer, informationUpdateCount integer)",
    "CREATE TABLE cue(cue_id integer primary key, content_id integer, kind integer, colorTableIndex integer, cueComment varchar, isActiveLoop integer, beatLoopNumerator integer, beatLoopDenominator integer, inUsec integer, outUsec integer, in150FramePerSec integer, out150FramePerSec integer, inMpegFrameNumber integer, outMpegFrameNumber integer, inMpegAbs integer, outMpegAbs integer, inDecodingStartFramePosition integer, outDecodingStartFramePosition integer, inFileOffsetInBlock integer, OutFileOffsetInBlock integer, inNumberOfSampleInBlock integer, outNumberOfSampleInBlock integer)",
    "CREATE TABLE genre(genre_id integer primary key, name varchar)",
    "CREATE TABLE history(history_id integer primary key, sequenceNo integer, name varchar, attribute integer, history_id_parent integer)",
    "CREATE TABLE history_content(history_id integer, content_id integer, sequenceNo integer)",
    "CREATE TABLE hotCueBankList(hotCueBankList_id integer primary key, sequenceNo integer, name varchar, image_id integer, attribute integer, hotCueBankList_id_parent integer)",
    "CREATE TABLE hotCueBankList_cue(hotCueBankList_id integer, cue_id integer, sequenceNo integer)",
    "CREATE TABLE image(image_id integer primary key, path varchar)",
    "CREATE TABLE key(key_id integer primary key, name varchar)",
    "CREATE TABLE label(label_id integer primary key, name varchar)",
    "CREATE TABLE menuItem(menuItem_id integer primary key, kind integer, name varchar)",
    "CREATE TABLE myTag(myTag_id integer primary key, sequenceNo integer, name varchar, attribute integer, myTag_id_parent integer)",
    "CREATE TABLE myTag_content(myTag_id integer, content_id integer)",
    "CREATE TABLE playlist(playlist_id integer primary key, sequenceNo integer, name varchar, image_id integer, attribute integer, playlist_id_parent integer)",
    "CREATE TABLE playlist_content(playlist_id integer, content_id integer, sequenceNo integer)",
    "CREATE TABLE property(deviceName varchar, dbVersion varchar, numberOfContents integer, createdDate varchar, backGroundColorType integer, myTagMasterDBID integer)",
    "CREATE TABLE recommendedLike(content_id_1 integer, content_id_2 integer, rating integer, createdDate integer)",
    "CREATE TABLE sort(sort_id integer primary key, menuItem_id integer, sequenceNo integer, isVisible integer, isSelectedAsSubColumn integer)",
    "CREATE INDEX index_hotCueBankList_cue_hotCueBankList_id on hotCueBankList_cue(hotCueBankList_id)",
    "CREATE INDEX index_myTag_content_content_id on myTag_content(content_id)",
    "CREATE INDEX index_myTag_content_myTag_id on myTag_content(myTag_id)",
    "CREATE INDEX index_playlist_content_playlist_id on playlist_content(playlist_id)",
];

/// Inputs needed to build the device library, beyond the tracks themselves.
pub struct DeviceLibraryOptions {
    /// A stable per-device identifier. Written to `property`-linked ids and
    /// used to name the `rbDevLibBaInfo_<masterDbId>.json` file. Any value is
    /// accepted by rekordbox; deterministic-per-USB is fine.
    pub master_db_id: i64,
    /// Device UUID (32 lowercase hex chars, no dashes) for the backup JSON.
    pub uuid: String,
    /// Creation date, `YYYY-MM-DD`.
    pub created_date: String,
    /// Optional device name (golden used empty string).
    pub device_name: String,
}

impl Default for DeviceLibraryOptions {
    fn default() -> Self {
        Self {
            master_db_id: 0,
            uuid: "00000000000000000000000000000000".to_string(),
            created_date: "2025-01-01".to_string(),
            device_name: String::new(),
        }
    }
}

/// One resolved playlist: name plus the ordered `content_id`s it contains.
///
/// `content_id` equals `TrackAnalysis::id` (both 1-based), matching how the
/// PDB and XML writers reference tracks.
pub struct PlaylistSpec<'a> {
    pub id: i64,
    pub name: &'a str,
    /// Ordered content ids (1-based, matching track ids).
    pub content_ids: &'a [u32],
}

/// Contents of `rbDevLibBaInfo_<masterDbId>.json`.
#[derive(Serialize)]
struct DevLibBackupInfo<'a> {
    uuid: &'a str,
    /// Always empty for a fresh export.
    info: [(); 0],
}

/// Build the encrypted `exportLibrary.db` in memory and return its bytes.
///
/// The database is created on a temp path (SQLCipher cannot operate purely
/// in-memory through an encrypted file), then read back as raw bytes so the
/// caller controls where it is written. `db_path` should be a writable scratch
/// path (it is created and left populated; caller may delete it).
pub fn build_export_library(
    db_path: &std::path::Path,
    tracks: &[TrackAnalysis],
    playlists: &[PlaylistSpec<'_>],
    opts: &DeviceLibraryOptions,
) -> Result<()> {
    // Start clean — rusqlite opens/creates the file.
    if db_path.exists() {
        std::fs::remove_file(db_path)
            .map_err(|e| Error::DeviceLibrary(format!("removing stale exportLibrary.db: {e}")))?;
    }

    let conn = Connection::open(db_path).map_err(|e| Error::DeviceLibrary(format!("open: {e}")))?;

    // Key the database. Must be the very first statement issued. SQLCipher 4
    // defaults (AES-256-CBC / PBKDF2-HMAC-SHA512 256000 / HMAC-SHA512 / 4096)
    // match the golden file, so no further cipher pragmas are needed.
    conn.pragma_update(None, "key", DEVICE_LIBRARY_KEY)
        .map_err(|e| Error::DeviceLibrary(format!("PRAGMA key: {e}")))?;

    conn.execute_batch("BEGIN;")
        .map_err(|e| Error::DeviceLibrary(format!("begin: {e}")))?;

    // --- schema --------------------------------------------------------
    for ddl in SCHEMA_DDL {
        conn.execute(ddl, [])
            .map_err(|e| Error::DeviceLibrary(format!("ddl [{ddl}]: {e}")))?;
    }

    // --- static template tables ---------------------------------------
    for (id, name) in COLORS {
        conn.execute(
            "INSERT INTO color(color_id,name) VALUES(?1,?2)",
            params![id, name],
        )
        .map_err(|e| Error::DeviceLibrary(format!("color: {e}")))?;
    }
    for (id, kind, name) in MENU_ITEMS {
        conn.execute(
            "INSERT INTO menuItem(menuItem_id,kind,name) VALUES(?1,?2,?3)",
            params![id, kind, name],
        )
        .map_err(|e| Error::DeviceLibrary(format!("menuItem: {e}")))?;
    }
    for (id, mi, seq, vis) in CATEGORIES {
        conn.execute(
            "INSERT INTO category(category_id,menuItem_id,sequenceNo,isVisible) VALUES(?1,?2,?3,?4)",
            params![id, mi, seq, vis],
        )
        .map_err(|e| Error::DeviceLibrary(format!("category: {e}")))?;
    }
    for (id, mi, seq, vis, sub) in SORTS {
        conn.execute(
            "INSERT INTO sort(sort_id,menuItem_id,sequenceNo,isVisible,isSelectedAsSubColumn) VALUES(?1,?2,?3,?4,?5)",
            params![id, mi, seq, vis, sub],
        )
        .map_err(|e| Error::DeviceLibrary(format!("sort: {e}")))?;
    }
    for (id, seq, name, attr, parent) in MYTAG_ROOTS {
        conn.execute(
            "INSERT INTO myTag(myTag_id,sequenceNo,name,attribute,myTag_id_parent) VALUES(?1,?2,?3,?4,?5)",
            params![id, seq, name, attr, parent],
        )
        .map_err(|e| Error::DeviceLibrary(format!("myTag: {e}")))?;
    }

    // --- artists (deduplicated by name) --------------------------------
    // Assign artist ids in first-seen order; map track -> artist id.
    let mut artist_ids: Vec<(String, i64)> = Vec::new();
    let mut next_artist_id: i64 = 1;
    let artist_id_for = |name: &str, list: &mut Vec<(String, i64)>, next: &mut i64| -> i64 {
        if let Some((_, id)) = list.iter().find(|(n, _)| n == name) {
            *id
        } else {
            let id = *next;
            *next += 1;
            list.push((name.to_string(), id));
            id
        }
    };
    // Resolve each track's artist id up front.
    let mut track_artist_id: Vec<i64> = Vec::with_capacity(tracks.len());
    for t in tracks {
        let aid = artist_id_for(&t.artist, &mut artist_ids, &mut next_artist_id);
        track_artist_id.push(aid);
    }
    for (name, id) in &artist_ids {
        conn.execute(
            "INSERT INTO artist(artist_id,name,nameForSearch) VALUES(?1,?2,NULL)",
            params![id, name],
        )
        .map_err(|e| Error::DeviceLibrary(format!("artist: {e}")))?;
    }

    // --- images (one per track that has artwork) -----------------------
    // Image id == track id for simplicity; only inserted when an artwork path
    // is known. Golden used `/PIONEER/Artwork/00001/b1.jpg`.
    for t in tracks {
        if let Some(path) = artwork_image_path(t.id) {
            conn.execute(
                "INSERT INTO image(image_id,path) VALUES(?1,?2)",
                params![t.id as i64, path],
            )
            .map_err(|e| Error::DeviceLibrary(format!("image: {e}")))?;
        }
    }

    // --- content (one row per track) -----------------------------------
    for (i, t) in tracks.iter().enumerate() {
        let bpmx100 = (t.bpm * 100.0).round() as i64;
        let length = t.duration_secs.round() as i64;
        let file_type = t.file_type as i64;
        let anlz_dat = crate::anlz::generate_anlz_path(t.id);
        let anlz_dat = format!("/{}", anlz_dat.trim_start_matches('/'));
        let content_id = t.id as i64;
        let artist_id = track_artist_id[i];
        let image_id = if artwork_image_path(t.id).is_some() {
            Some(t.id as i64)
        } else {
            None
        };
        // `path` is the USB-relative audio path (leading slash), matching the
        // golden's `/Contents/Artist/Album/file.mp3`.
        let usb_path = normalize_usb_path(&t.file_path);
        let file_name = std::path::Path::new(&t.file_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();

        conn.execute(
            "INSERT INTO content(\
                content_id,title,titleForSearch,subtitle,bpmx100,length,trackNo,discNo,\
                artist_id_artist,artist_id_remixer,artist_id_originalArtist,artist_id_composer,artist_id_lyricist,\
                album_id,genre_id,label_id,key_id,color_id,image_id,djComment,rating,releaseYear,releaseDate,\
                dateCreated,dateAdded,path,fileName,fileSize,fileType,bitrate,bitDepth,samplingRate,isrc,\
                djPlayCount,isHotCueAutoLoadOn,isKuvoDeliverStatusOn,kuvoDeliveryComment,masterDbId,masterContentId,\
                analysisDataFilePath,analysedBits,contentLink,hasModified,cueUpdateCount,analysisDataUpdateCount,informationUpdateCount\
             ) VALUES(\
                ?1,?2,NULL,'',?3,?4,?5,?6,\
                ?7,NULL,NULL,NULL,0,\
                NULL,NULL,NULL,0,0,?8,'',?9,?10,'',\
                ?11,?12,?13,?14,?15,?16,?17,?18,?19,'',\
                0,1,1,'',?20,?21,\
                ?22,?23,0,0,NULL,NULL,NULL\
             )",
            params![
                content_id,
                t.title,
                bpmx100,
                length,
                t.track_number.unwrap_or(0) as i64,
                0i64, // discNo
                artist_id,
                image_id,
                0i64, // rating — not tracked; default 0
                t.year.unwrap_or(0) as i64,
                opts.created_date, // dateCreated
                opts.created_date, // dateAdded
                usb_path,
                file_name,
                t.file_size as i64,
                file_type,
                t.bitrate as i64,
                t.bit_depth as i64,
                t.sample_rate as i64,
                opts.master_db_id,
                content_id, // masterContentId — arbitrary; reuse content id
                anlz_dat,
                41i64, // analysedBits — golden value; observed constant
            ],
        )
        .map_err(|e| Error::DeviceLibrary(format!("content: {e}")))?;
    }

    // --- playlists -----------------------------------------------------
    for pl in playlists {
        conn.execute(
            "INSERT INTO playlist(playlist_id,sequenceNo,name,image_id,attribute,playlist_id_parent) VALUES(?1,?2,?3,NULL,0,0)",
            params![pl.id, pl.id - 1, pl.name],
        )
        .map_err(|e| Error::DeviceLibrary(format!("playlist: {e}")))?;

        for (seq, cid) in pl.content_ids.iter().enumerate() {
            conn.execute(
                "INSERT INTO playlist_content(playlist_id,content_id,sequenceNo) VALUES(?1,?2,?3)",
                params![pl.id, *cid as i64, (seq as i64) + 1],
            )
            .map_err(|e| Error::DeviceLibrary(format!("playlist_content: {e}")))?;
        }
    }

    // --- property (single row) -----------------------------------------
    conn.execute(
        "INSERT INTO property(deviceName,dbVersion,numberOfContents,createdDate,backGroundColorType,myTagMasterDBID) VALUES(?1,?2,?3,?4,0,?5)",
        params![
            opts.device_name,
            DB_VERSION,
            tracks.len() as i64,
            opts.created_date,
            opts.master_db_id,
        ],
    )
    .map_err(|e| Error::DeviceLibrary(format!("property: {e}")))?;

    conn.execute_batch("COMMIT;")
        .map_err(|e| Error::DeviceLibrary(format!("commit: {e}")))?;

    conn.close()
        .map_err(|(_, e)| Error::DeviceLibrary(format!("close: {e}")))?;

    Ok(())
}

/// Serialize the `rbDevLibBaInfo_<masterDbId>.json` contents.
pub fn build_devlib_backup_json(opts: &DeviceLibraryOptions) -> Result<String> {
    let info = DevLibBackupInfo {
        uuid: &opts.uuid,
        info: [],
    };
    serde_json::to_string_pretty(&info)
        .map_err(|e| Error::DeviceLibrary(format!("backup json: {e}")))
}

/// Filename for the device-library backup JSON.
pub fn devlib_backup_filename(opts: &DeviceLibraryOptions) -> String {
    format!("rbDevLibBaInfo_{}.json", opts.master_db_id)
}

/// USB-relative artwork path for a track id, or `None` if no artwork exists.
///
/// Mirrors the PDB/artwork layout (`/PIONEER/Artwork/NNNNN/b1.jpg`). Returns
/// `None` for now since the exporter does not yet emit artwork; wire this to
/// real artwork generation when that lands.
fn artwork_image_path(_track_id: u32) -> Option<String> {
    None
}

/// Ensure a USB-relative path has a single leading slash and forward slashes.
fn normalize_usb_path(p: &str) -> String {
    let p = p.replace('\\', "/");
    if p.starts_with('/') {
        p
    } else {
        format!("/{p}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backup_json_shape() {
        let opts = DeviceLibraryOptions {
            master_db_id: 572520782,
            uuid: "7e3edbae287b47ae9518e96877691de9".into(),
            ..Default::default()
        };
        let json = build_devlib_backup_json(&opts).unwrap();
        assert!(json.contains("7e3edbae287b47ae9518e96877691de9"));
        assert!(json.contains("\"info\": []"));
        assert_eq!(
            devlib_backup_filename(&opts),
            "rbDevLibBaInfo_572520782.json"
        );
    }

    #[test]
    fn builds_encrypted_db_that_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("exportLibrary.db");
        let tracks: Vec<TrackAnalysis> = vec![];
        let opts = DeviceLibraryOptions::default();
        build_export_library(&db, &tracks, &[], &opts).unwrap();

        // File must NOT be plaintext SQLite (i.e. it is encrypted).
        let head = std::fs::read(&db).unwrap();
        assert!(
            !head.starts_with(b"SQLite format 3"),
            "db must be encrypted"
        );

        // Reopening with the key must succeed and expose the schema.
        let conn = Connection::open(&db).unwrap();
        conn.pragma_update(None, "key", DEVICE_LIBRARY_KEY).unwrap();
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM color", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 8);
        let prop: i64 = conn
            .query_row("SELECT numberOfContents FROM property", [], |r| r.get(0))
            .unwrap();
        assert_eq!(prop, 0);
    }
}
