//! bvault-core: Pioneer DJ format structures with write support
//!
//! This crate provides binary serialization for:
//! - export.pdb (DeviceSQL database) - little-endian
//! - ANLZ files (.DAT, .EXT) - big-endian
//!
//! Based on Deep Symmetry's reverse engineering documentation:
//! https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/

pub mod anlz;
pub mod auxiliary;
pub mod error;
pub mod page;
pub mod pdb;
pub mod string;
pub mod track;

#[cfg(feature = "debug-tools")]
pub mod diff;
#[cfg(feature = "debug-tools")]
pub mod validate;
#[cfg(feature = "debug-tools")]
pub mod xml;

#[cfg(feature = "device-library")]
pub mod device_library;

// Re-exports for convenience
pub use anlz::{generate_2ex_file, generate_anlz_path, generate_dat_file, generate_ext_file};
pub use auxiliary::{
    artwork_folder_path, artwork_full_name, artwork_thumbnail_name, generate_devsetting,
    generate_djprofile, ARTWORK_FULL_SIZE, ARTWORK_THUMBNAIL_SIZE,
};
pub use error::{Error, Result};
pub use pdb::PdbBuilder;
pub use track::{
    Beat, BeatGrid, CuePoint, CueType, FileType, HotCueColor, Key, TrackAnalysis, Waveform,
    WaveformColorEntry, WaveformColorPreview, WaveformColorPreviewColumn, WaveformColumn,
    WaveformDetail, WaveformPreview,
};

#[cfg(feature = "debug-tools")]
pub use diff::{diff_pdb, ByteRange, PageDiff, PdbDiff};
#[cfg(feature = "debug-tools")]
pub use validate::{validate_and_print, validate_pdb, PdbStats, ValidationResult};
#[cfg(feature = "debug-tools")]
pub use xml::{generate_xml, XmlExportOptions};

#[cfg(feature = "device-library")]
pub use device_library::{
    build_devlib_backup_json, build_export_library, devlib_backup_filename, DeviceLibraryOptions,
    PlaylistSpec,
};
