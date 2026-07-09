//! Analysis cache using filesystem storage
//!
//! Stores analysis results on disk keyed by file hash.
//! This is critical for memory-constrained environments.

use std::fs::{self, File};
use std::io::{BufReader, BufWriter, Read};
use std::path::{Path, PathBuf};

use xxhash_rust::xxh3::xxh3_64;

use crate::error::{Error, Result};
use crate::track::TrackAnalysis;

/// File-based cache for track analysis results
pub struct AnalysisCache {
    cache_dir: PathBuf,
}

impl AnalysisCache {
    /// Create a new cache at the given directory
    pub fn new<P: AsRef<Path>>(cache_dir: P) -> Result<Self> {
        let cache_dir = cache_dir.as_ref().to_path_buf();
        fs::create_dir_all(&cache_dir)?;
        Ok(Self { cache_dir })
    }
    
    /// Analyzer output version. BUMP THIS whenever BPM, beat-grid, or waveform
    /// generation changes, so stale cached analyses are not silently reused.
    /// (Cache was keyed on file hash ALONE - every algorithm fix was invisible
    /// until the cache dir was manually cleared.)
    const ANALYZER_VERSION: u32 = 2;

    /// Generate a cache key from file hash + analyzer version
    fn cache_key(file_hash: u64) -> String {
        format!("{:016x}.v{}.json", file_hash, Self::ANALYZER_VERSION)
    }
    
    /// Get cached analysis if it exists and is valid
    pub fn get(&self, file_hash: u64) -> Option<TrackAnalysis> {
        let key = Self::cache_key(file_hash);
        let path = self.cache_dir.join(&key);
        
        if !path.exists() {
            return None;
        }
        
        let file = File::open(&path).ok()?;
        let reader = BufReader::new(file);
        serde_json::from_reader(reader).ok()
    }
    
    /// Store analysis result in cache
    pub fn put(&self, analysis: &TrackAnalysis) -> Result<()> {
        let key = Self::cache_key(analysis.file_hash);
        let path = self.cache_dir.join(&key);
        
        let file = File::create(&path)?;
        let writer = BufWriter::new(file);
        serde_json::to_writer(writer, analysis)?;
        
        Ok(())
    }
    
    /// Remove cached analysis
    pub fn invalidate(&self, file_hash: u64) -> Result<()> {
        let key = Self::cache_key(file_hash);
        let path = self.cache_dir.join(&key);
        
        if path.exists() {
            fs::remove_file(&path)?;
        }
        Ok(())
    }
    
    /// Clear entire cache
    pub fn clear(&self) -> Result<()> {
        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }
    
    /// Get cache statistics
    pub fn stats(&self) -> Result<CacheStats> {
        let mut count = 0;
        let mut total_size = 0;
        
        for entry in fs::read_dir(&self.cache_dir)? {
            let entry = entry?;
            if entry.path().extension().map(|e| e == "json").unwrap_or(false) {
                count += 1;
                total_size += entry.metadata()?.len();
            }
        }
        
        Ok(CacheStats {
            entry_count: count,
            total_size_bytes: total_size,
        })
    }
}

/// Cache statistics
#[derive(Debug, Clone)]
pub struct CacheStats {
    pub entry_count: usize,
    pub total_size_bytes: u64,
}

/// Compute file hash for cache invalidation
/// Uses XXH3 on a sample of the file (first 1MB + file size) for speed
pub fn compute_file_hash<P: AsRef<Path>>(path: P) -> Result<u64> {
    let metadata = fs::metadata(&path)?;
    let file_size = metadata.len();
    
    // Read first 1MB (or entire file if smaller)
    let sample_size = std::cmp::min(file_size as usize, 1024 * 1024);
    let mut sample = vec![0u8; sample_size + 8];
    
    let mut file = File::open(&path)?;
    file.read_exact(&mut sample[..sample_size])?;
    
    // Append file size to sample for uniqueness
    sample[sample_size..].copy_from_slice(&file_size.to_le_bytes());
    
    Ok(xxh3_64(&sample))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::*;
    use tempfile::TempDir;
    
    fn make_test_analysis() -> TrackAnalysis {
        TrackAnalysis {
            id: 1,
            file_path: "test.mp3".into(),
            title: "Test Track".into(),
            artist: "Test Artist".into(),
            album: None,
            genre: None,
            duration_secs: 180.0,
            sample_rate: 44100,
            bit_depth: 16,
            bitrate: 320,
            bpm: 128.0,
            key: None,
            beat_grid: BeatGrid::default(),
            waveform: Waveform::default(),
            cue_points: Vec::new(),
            file_size: 5_000_000,
            file_hash: 0x12345678ABCDEF00,
            year: None,
            comment: None,
            track_number: None,
            file_type: FileType::Mp3,
        }
    }
    
    #[test]
    fn test_cache_roundtrip() {
        let tmp = TempDir::new().unwrap();
        let cache = AnalysisCache::new(tmp.path()).unwrap();
        
        let analysis = make_test_analysis();
        
        // Store and retrieve
        cache.put(&analysis).unwrap();
        let retrieved = cache.get(analysis.file_hash).unwrap();
        
        assert_eq!(retrieved.id, analysis.id);
        assert_eq!(retrieved.title, analysis.title);
        assert_eq!(retrieved.file_hash, analysis.file_hash);
    }
    
    #[test]
    fn test_cache_miss() {
        let tmp = TempDir::new().unwrap();
        let cache = AnalysisCache::new(tmp.path()).unwrap();
        
        let result = cache.get(0xDEADBEEF);
        assert!(result.is_none());
    }
    
    #[test]
    fn test_cache_stats() {
        let tmp = TempDir::new().unwrap();
        let cache = AnalysisCache::new(tmp.path()).unwrap();
        
        let mut analysis = make_test_analysis();
        cache.put(&analysis).unwrap();
        
        analysis.file_hash = 0x9999;
        cache.put(&analysis).unwrap();
        
        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 2);
        assert!(stats.total_size_bytes > 0);
    }
    
    #[test]
    fn test_cache_clear() {
        let tmp = TempDir::new().unwrap();
        let cache = AnalysisCache::new(tmp.path()).unwrap();
        
        let analysis = make_test_analysis();
        cache.put(&analysis).unwrap();
        
        cache.clear().unwrap();
        
        let stats = cache.stats().unwrap();
        assert_eq!(stats.entry_count, 0);
    }
}