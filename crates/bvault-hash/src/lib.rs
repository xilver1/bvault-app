//! bvault-hash: the one definition of content identity.
//!
//! A track's identity is `xxh3_64` over a fixed sample of its bytes — the first
//! [`SAMPLE_BYTES`] of content followed by the total length as a little-endian
//! `u64` — rendered as 16 lowercase hex chars ([`hash_hex`]). This is
//! deliberately the *only* place the formula lives, so the value ingestion
//! mints, the value the queue dedups on, and the value the analysis records can
//! never drift apart.
//!
//! It reproduces exactly what `bvault_core::cache::compute_file_hash` computes
//! from a path — verified by an equality test that mirrors that function
//! byte-for-byte — but is reachable without pulling core (and thus OpenSSL) into
//! the ingest path, and adds a streaming form the path-based one lacks.
//!
//! Two entry points produce the identical value:
//! - [`hash_bytes`] for content already in memory.
//! - [`ContentHasher`], an [`std::io::Write`] sink for streaming ingestion — tee
//!   a download or upload through it while the bytes land on disk, then
//!   [`ContentHasher::finalize`]. You never have to buffer a whole file to hash
//!   it, and you learn the size and the hash in the same single pass.

use std::io::{self, Write};

use xxhash_rust::xxh3::xxh3_64;

/// Bytes of content sampled from the head of the file, before the length suffix.
/// Matches core's 1 MiB window; changing it would change every existing hash.
pub const SAMPLE_BYTES: usize = 1024 * 1024;

/// Content identity for an in-memory buffer. `data` is the *whole* file's bytes.
pub fn hash_bytes(data: &[u8]) -> u64 {
    let sample_len = data.len().min(SAMPLE_BYTES);
    let mut buf = Vec::with_capacity(sample_len + 8);
    buf.extend_from_slice(&data[..sample_len]);
    buf.extend_from_slice(&(data.len() as u64).to_le_bytes());
    xxh3_64(&buf)
}

/// Render a content hash as the canonical 16-char lowercase hex string — the
/// form stored in the DB, used as the artifact-store key, and (with a user
/// prefix) embedded in job dedup keys.
pub fn hash_hex(hash: u64) -> String {
    format!("{hash:016x}")
}

/// Streaming content hasher. Implements [`Write`], so a reader can be copied
/// through it (e.g. tee'd alongside a file write) to hash and store in one pass.
/// Produces the same value as [`hash_bytes`] over the full stream, regardless of
/// how the bytes are chunked.
pub struct ContentHasher {
    /// The first `SAMPLE_BYTES` of content, captured as it streams past.
    sample: Vec<u8>,
    /// Total bytes seen — becomes the file size in the length suffix.
    total: u64,
}

impl Default for ContentHasher {
    fn default() -> Self {
        Self::new()
    }
}

impl ContentHasher {
    pub fn new() -> Self {
        // Grow into the sample rather than allocating a megabyte up front: most
        // tracks are multi-MB, but the worker is RAM-limited and small files are
        // common in tests.
        Self {
            sample: Vec::with_capacity(64 * 1024),
            total: 0,
        }
    }

    /// Feed bytes into the hasher (also available via the [`Write`] impl).
    pub fn update(&mut self, bytes: &[u8]) {
        self.total += bytes.len() as u64;
        if self.sample.len() < SAMPLE_BYTES {
            let take = (SAMPLE_BYTES - self.sample.len()).min(bytes.len());
            self.sample.extend_from_slice(&bytes[..take]);
        }
    }

    /// Total bytes fed so far — the file size, once the stream is fully drained.
    pub fn len(&self) -> u64 {
        self.total
    }

    /// Whether nothing has been fed yet.
    pub fn is_empty(&self) -> bool {
        self.total == 0
    }

    /// Consume the hasher and produce the content hash.
    pub fn finalize(mut self) -> u64 {
        self.sample.extend_from_slice(&self.total.to_le_bytes());
        xxh3_64(&self.sample)
    }
}

impl Write for ContentHasher {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.update(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Byte-for-byte mirror of `bvault_core::cache::compute_file_hash`, but over
    /// an in-memory buffer instead of a path. This is the contract bvault-hash
    /// must honour: mint the identical identity core does, without depending on
    /// core. If core's formula ever changes, this test (and the hash universe)
    /// changes with it — deliberately.
    fn reference_like_core(data: &[u8]) -> u64 {
        let file_size = data.len() as u64;
        let sample_size = (file_size as usize).min(1024 * 1024);
        let mut sample = vec![0u8; sample_size + 8];
        sample[..sample_size].copy_from_slice(&data[..sample_size]);
        sample[sample_size..].copy_from_slice(&file_size.to_le_bytes());
        xxh3_64(&sample)
    }

    /// Deterministic pseudo-random bytes (LCG) so the head and the tail of a
    /// >1 MiB buffer differ — exercising the sampling boundary rather than
    /// hashing a uniform run where truncation wouldn't matter.
    fn pseudo_random(len: usize) -> Vec<u8> {
        let mut state: u64 = 0x9e37_79b9_7f4a_7c15;
        (0..len)
            .map(|_| {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 56) as u8
            })
            .collect()
    }

    // A spread of sizes straddling the 1 MiB sample boundary.
    const SIZES: &[usize] = &[
        0,
        1,
        1023,
        SAMPLE_BYTES - 1,
        SAMPLE_BYTES,
        SAMPLE_BYTES + 1,
        3 * SAMPLE_BYTES + 7,
    ];

    #[test]
    fn matches_core_formula() {
        for &n in SIZES {
            let data = pseudo_random(n);
            assert_eq!(
                hash_bytes(&data),
                reference_like_core(&data),
                "hash_bytes diverged from core's formula at len {n}"
            );
        }
    }

    #[test]
    fn streaming_equals_one_shot_across_chunkings() {
        for &n in SIZES {
            let data = pseudo_random(n);
            let want = hash_bytes(&data);
            for &chunk in &[1usize, 7, 4096, 65536, SAMPLE_BYTES, usize::MAX] {
                let mut h = ContentHasher::new();
                for c in data.chunks(chunk.min(data.len().max(1))) {
                    h.update(c);
                }
                assert_eq!(h.len(), n as u64, "len wrong at size {n}, chunk {chunk}");
                assert_eq!(
                    h.finalize(),
                    want,
                    "streaming hash diverged at size {n}, chunk {chunk}"
                );
            }
        }
    }

    #[test]
    fn write_impl_matches_update() {
        let data = pseudo_random(3 * SAMPLE_BYTES + 7);
        let want = hash_bytes(&data);

        // Drive it purely through io::copy, as ingestion will.
        let mut h = ContentHasher::new();
        let mut src = &data[..];
        io::copy(&mut src, &mut h).unwrap();
        assert_eq!(h.finalize(), want);
    }

    #[test]
    fn hex_is_16_chars_zero_padded_lowercase() {
        assert_eq!(hash_hex(0x1234_5678), "0000000012345678");
        assert_eq!(hash_hex(0), "0000000000000000");
        assert_eq!(hash_hex(u64::MAX), "ffffffffffffffff");
        // Round-trips back to the u64 the worker parses with from_str_radix.
        let h = hash_bytes(&pseudo_random(4096));
        assert_eq!(u64::from_str_radix(&hash_hex(h), 16).unwrap(), h);
    }
}
