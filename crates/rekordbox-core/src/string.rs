//! DeviceSQL string encoding for Pioneer databases
//!
//! DeviceSQL strings use three encoding formats:
//! - Short ASCII (flag LSB=1): Length encoded in header byte, max 126 chars
//! - Long ASCII (0x40): 4-byte header + ASCII data
//! - UTF-16LE (0x90): 4-byte header + UTF-16LE encoded data
//!
//! Reference: https://djl-analysis.deepsymmetry.org/rekordbox-export-analysis/exports.html

/// Maximum length for short ASCII strings
const MAX_SHORT_ASCII_LEN: usize = 126;

/// Flag byte values
const FLAG_LONG_ASCII: u8 = 0x40;
const FLAG_UTF16LE: u8 = 0x90;

/// Encode a string in DeviceSQL format
/// 
/// Automatically selects the appropriate encoding:
/// - Short ASCII for ASCII strings ≤126 chars
/// - Long ASCII for longer ASCII strings
/// - UTF-16LE for strings containing non-ASCII characters
pub fn encode_string(s: &str) -> Vec<u8> {
    if s.is_empty() {
        // Empty string: just the flag byte indicating length 1 (includes the flag itself)
        return vec![0x03]; // (1 << 1) | 1 = 3
    }
    
    let is_ascii = s.bytes().all(|b| b < 128);
    
    if is_ascii && s.len() <= MAX_SHORT_ASCII_LEN {
        encode_short_ascii(s)
    } else if is_ascii {
        encode_long_ascii(s)
    } else {
        encode_utf16le(s)
    }
}

/// Encode as short ASCII string
/// Header byte: ((length + 1) << 1) | 1
fn encode_short_ascii(s: &str) -> Vec<u8> {
    let total_len = s.len() + 1; // +1 for header byte
    let header = ((total_len as u8) << 1) | 1;
    
    let mut result = Vec::with_capacity(total_len);
    result.push(header);
    result.extend_from_slice(s.as_bytes());
    result
}

/// Encode as long ASCII string
/// Format: [0x40, len_lo, len_hi, 0x00, ...ascii_data...]
fn encode_long_ascii(s: &str) -> Vec<u8> {
    let total_len = 4 + s.len(); // 4-byte header + data
    
    let mut result = Vec::with_capacity(total_len);
    result.push(FLAG_LONG_ASCII);
    result.push((total_len & 0xFF) as u8);
    result.push(((total_len >> 8) & 0xFF) as u8);
    result.push(0x00); // padding
    result.extend_from_slice(s.as_bytes());
    result
}

/// Encode as UTF-16LE string
/// Format: [0x90, len_lo, len_hi, 0x00, ...utf16_data...]
fn encode_utf16le(s: &str) -> Vec<u8> {
    let utf16_chars: Vec<u16> = s.encode_utf16().collect();
    let utf16_bytes_len = utf16_chars.len() * 2;
    let total_len = 4 + utf16_bytes_len; // 4-byte header + data
    
    let mut result = Vec::with_capacity(total_len);
    result.push(FLAG_UTF16LE);
    result.push((total_len & 0xFF) as u8);
    result.push(((total_len >> 8) & 0xFF) as u8);
    result.push(0x00); // padding
    
    // Write UTF-16LE bytes
    for ch in utf16_chars {
        result.push((ch & 0xFF) as u8);
        result.push(((ch >> 8) & 0xFF) as u8);
    }
    
    result
}

/// Encode an ISRC (International Standard Recording Code)
/// ISRCs use a special format: [0x90, len_lo, len_hi, 0x00, 0x03, ...ascii..., 0x00]
pub fn encode_isrc(isrc: &str) -> Vec<u8> {
    if isrc.is_empty() {
        return encode_string("");
    }
    
    // ISRC format: flag + length + padding + 0x03 marker + ASCII + null terminator
    let data_len = 1 + isrc.len() + 1; // 0x03 + data + null
    let total_len = 4 + data_len;
    
    let mut result = Vec::with_capacity(total_len);
    result.push(FLAG_UTF16LE); // Uses 0x90 flag despite being ASCII
    result.push((total_len & 0xFF) as u8);
    result.push(((total_len >> 8) & 0xFF) as u8);
    result.push(0x00);
    result.push(0x03); // ISRC marker
    result.extend_from_slice(isrc.as_bytes());
    result.push(0x00); // Null terminator
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_empty_string() {
        let encoded = encode_string("");
        assert_eq!(encoded, vec![0x03]);
    }
    
    #[test]
    fn test_short_ascii() {
        let encoded = encode_string("foo");
        // Length = 4 (3 chars + 1 header), header = (4 << 1) | 1 = 9
        assert_eq!(encoded[0], 0x09);
        assert_eq!(&encoded[1..], b"foo");
    }
    
    #[test]
    fn test_short_ascii_single_char() {
        let encoded = encode_string("A");
        // Length = 2 (1 char + 1 header), header = (2 << 1) | 1 = 5
        assert_eq!(encoded[0], 0x05);
        assert_eq!(encoded[1], b'A');
    }
    
    #[test]
    fn test_long_ascii() {
        let long_str = "a".repeat(200);
        let encoded = encode_long_ascii(&long_str);
        
        assert_eq!(encoded[0], FLAG_LONG_ASCII);
        let len = encoded[1] as u16 | ((encoded[2] as u16) << 8);
        assert_eq!(len as usize, 4 + 200);
        assert_eq!(&encoded[4..], long_str.as_bytes());
    }
    
    #[test]
    fn test_utf16le() {
        let encoded = encode_utf16le("日本語");
        
        assert_eq!(encoded[0], FLAG_UTF16LE);
        // 3 characters * 2 bytes + 4 byte header = 10
        let len = encoded[1] as u16 | ((encoded[2] as u16) << 8);
        assert_eq!(len, 10);
    }
}
