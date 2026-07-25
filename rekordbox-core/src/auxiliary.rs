//! Auxiliary file generation for Pioneer USB exports
//!
//! This module generates the helper files required for complete CDJ compatibility:
//! - DEVSETTING.DAT: Device settings file
//! - djprofile.nxs: DJ profile information
//! - Artwork: Album art thumbnails and full images

/// rekordbox version string for DEVSETTING.DAT
const REKORDBOX_VERSION: &str = "6.8.4";

/// Generate DEVSETTING.DAT file contents
/// 
/// This 140-byte file contains device and application information.
/// Structure is little-endian.
pub fn generate_devsetting() -> Vec<u8> {
    let mut data = vec![0u8; 140];
    
    // 0x00-0x03: Size/Header value (0x60 = 96)
    data[0..4].copy_from_slice(&96u32.to_le_bytes());
    
    // 0x04-0x1F: Brand string "PIONEER DJ" (28 bytes, null-padded)
    let brand = b"PIONEER DJ";
    data[4..4 + brand.len()].copy_from_slice(brand);
    
    // 0x20-0x23: Padding (zeros) - already zero
    
    // 0x24-0x43: Application "rekordbox" (32 bytes, null-padded)
    let app = b"rekordbox";
    data[0x24..0x24 + app.len()].copy_from_slice(app);
    
    // 0x44-0x63: Version string (32 bytes, null-padded)
    let version = REKORDBOX_VERSION.as_bytes();
    data[0x44..0x44 + version.len()].copy_from_slice(version);
    
    // 0x64-0x67: Section marker (0x00000020)
    data[0x64..0x68].copy_from_slice(&0x20u32.to_le_bytes());
    
    // 0x68-0x6B: Magic value (0x12345678)
    data[0x68..0x6C].copy_from_slice(&0x12345678u32.to_le_bytes());
    
    // 0x6C-0x6F: Unknown value (0x00000001)
    data[0x6C..0x70].copy_from_slice(&1u32.to_le_bytes());
    
    // 0x70-0x7F: Settings flags (default: all enabled)
    // Bytes: 01 01 01 01 01 01 00 00 00 00 00 00 00 00 00 00
    data[0x70] = 0x01;
    data[0x71] = 0x01;
    data[0x72] = 0x01;
    data[0x73] = 0x01;
    data[0x74] = 0x01;
    data[0x75] = 0x01;
    // Rest are zeros
    
    // 0x80-0x87: More zeros
    
    // 0x88-0x8B: Tail value (observed: 0x0000D016 = 53270)
    // This might be a checksum or version indicator
    data[0x88..0x8C].copy_from_slice(&0xD016u32.to_le_bytes());
    
    data
}

/// Generate djprofile.nxs file contents
/// 
/// This 160-byte file contains the DJ profile name.
/// The name appears at offset 0x20.
pub fn generate_djprofile(profile_name: &str) -> Vec<u8> {
    let mut data = vec![0u8; 160];
    
    // 0x00-0x1F: Zero padding (32 bytes) - already zero
    
    // 0x20-0x3F: Profile name (32 bytes, null-terminated)
    let name_bytes = profile_name.as_bytes();
    let copy_len = name_bytes.len().min(31); // Leave room for null terminator
    data[0x20..0x20 + copy_len].copy_from_slice(&name_bytes[..copy_len]);
    
    // 0x40-0x9F: Zero padding (96 bytes) - already zero
    
    data
}

/// Artwork image sizes
pub const ARTWORK_THUMBNAIL_SIZE: u32 = 80;
pub const ARTWORK_FULL_SIZE: u32 = 240;

/// Generate artwork folder path for a given artwork ID
/// 
/// Artworks are organized in folders of ~100 items each.
/// Folder naming: 5-digit zero-padded (00001, 00002, ...)
pub fn artwork_folder_path(artwork_id: u32) -> String {
    let folder_num = (artwork_id / 100) + 1;
    format!("PIONEER/Artwork/{:05}", folder_num)
}

/// Generate artwork filename for thumbnail
pub fn artwork_thumbnail_name(artwork_id: u32) -> String {
    format!("a{}.jpg", artwork_id)
}

/// Generate artwork filename for full-size image
pub fn artwork_full_name(artwork_id: u32) -> String {
    format!("a{}_m.jpg", artwork_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_devsetting_generation() {
        let data = generate_devsetting();
        
        assert_eq!(data.len(), 140);
        
        // Check header
        assert_eq!(u32::from_le_bytes([data[0], data[1], data[2], data[3]]), 96);
        
        // Check brand
        assert_eq!(&data[4..14], b"PIONEER DJ");
        
        // Check app
        assert_eq!(&data[0x24..0x2D], b"rekordbox");
        
        // Check version
        assert_eq!(&data[0x44..0x49], b"6.8.4");
        
        // Check magic
        assert_eq!(u32::from_le_bytes([data[0x68], data[0x69], data[0x6A], data[0x6B]]), 0x12345678);
    }
    
    #[test]
    fn test_djprofile_generation() {
        let data = generate_djprofile("Test DJ");
        
        assert_eq!(data.len(), 160);
        
        // Check name at offset 0x20
        assert_eq!(&data[0x20..0x27], b"Test DJ");
        
        // Check null termination
        assert_eq!(data[0x27], 0);
    }
    
    #[test]
    fn test_artwork_paths() {
        assert_eq!(artwork_folder_path(1), "PIONEER/Artwork/00001");
        assert_eq!(artwork_folder_path(99), "PIONEER/Artwork/00001");
        assert_eq!(artwork_folder_path(100), "PIONEER/Artwork/00002");
        assert_eq!(artwork_folder_path(250), "PIONEER/Artwork/00003");
        
        assert_eq!(artwork_thumbnail_name(42), "a42.jpg");
        assert_eq!(artwork_full_name(42), "a42_m.jpg");
    }
}
