//! rekordbox-core: Pioneer DJ format structures with write support
//!
//! This crate provides binary serialization for:
//! - export.pdb (DeviceSQL database) - little-endian
//! - ANLZ files (.DAT, .EXT) - big-endian
//!
//! Based on Deep Symmetry's reverse engineering documentation:
//! https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/

pub mod error;
pub mod string;
pub mod page;
pub mod pdb;
pub mod anlz;
pub mod track;
pub mod cache;
pub mod validate;
pub mod auxiliary;
pub mod xml;
pub mod diff;

#[cfg(feature = "device-library")]
pub mod device_library;

// Re-exports for convenience
pub use error::{Error, Result};
pub use track::{TrackAnalysis, BeatGrid, Beat, Waveform, WaveformPreview, WaveformDetail,
                WaveformColumn, WaveformColorEntry, WaveformColorPreview, WaveformColorPreviewColumn,
                Key, FileType, CuePoint, CueType, HotCueColor};
pub use pdb::PdbBuilder;
pub use anlz::{generate_dat_file, generate_ext_file, generate_2ex_file, generate_anlz_path};
pub use cache::{AnalysisCache, CacheStats, compute_file_hash};
pub use validate::{validate_pdb, validate_and_print, ValidationResult, PdbStats};
pub use auxiliary::{generate_devsetting, generate_djprofile, artwork_folder_path,
                    artwork_thumbnail_name, artwork_full_name, ARTWORK_THUMBNAIL_SIZE,
                    ARTWORK_FULL_SIZE};
pub use xml::{generate_xml, XmlExportOptions};
pub use diff::{diff_pdb, PdbDiff, PageDiff, ByteRange};

#[cfg(feature = "device-library")]
pub use device_library::{build_export_library, build_devlib_backup_json,
                         devlib_backup_filename, DeviceLibraryOptions, PlaylistSpec};