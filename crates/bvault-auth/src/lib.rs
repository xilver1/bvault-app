//! bvault-auth: the security primitives behind self-hosted accounts — password
//! hashing and opaque session tokens. Deliberately pure: no database, no HTTP.
//! It knows how to turn a password into a verifiable hash and how to mint and
//! fingerprint session tokens; *where* users and sessions live is bvault-meta's
//! job, and the axum extractor that ties a request to a user lands with the
//! services that need it.
//!
//! Two hashing choices, on purpose:
//! - Passwords are low-entropy and attacker-guessable → **Argon2id**
//!   (memory-hard, deliberately slow) with a per-password random salt.
//! - Session tokens are 256-bit CSPRNG output — not guessable — so at rest they
//!   get a plain **SHA-256** fingerprint. Fingerprinting means a read-only leak
//!   of the sessions table yields no usable tokens, while staying fast: there's
//!   no attacker dictionary to slow down on an already-uniform 256-bit secret.
//!   The raw token is shown to the client once and never stored.

use aes_gcm::{
    aead::{Aead, AeadCore, KeyInit},
    Aes256Gcm, Key, Nonce,
};
use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use rand_core::{OsRng, RngCore};
use sha2::{Digest, Sha256};

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("password hashing: {0}")]
    Hash(String),
    #[error("encryption error: {0}")]
    Encryption(String),
    #[error("decryption error: {0}")]
    Decryption(String),
}
pub type Result<T> = std::result::Result<T, Error>;

/// Hash a password with Argon2id (crate defaults: OWASP-aligned memory/time
/// cost) and a fresh random salt. Returns the PHC string to store verbatim — it
/// embeds algorithm, parameters, salt and digest, so verification needs nothing
/// else alongside it.
pub fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Hash(e.to_string()))
}

/// Verify a password against a stored PHC hash. Returns `false` for both a wrong
/// password and a malformed stored hash — callers get a single boolean and
/// can't build an oracle from *why* it failed. The digest comparison is
/// constant-time inside Argon2.
pub fn verify_password(password: &str, phc: &str) -> bool {
    PasswordHash::new(phc)
        .map(|parsed| {
            Argon2::default()
                .verify_password(password.as_bytes(), &parsed)
                .is_ok()
        })
        .unwrap_or(false)
}

/// Mint a new opaque session token: 256 bits from the OS CSPRNG, hex-encoded to
/// 64 chars. This raw value is the bearer token handed to the client **once**;
/// only its [`hash_token`] fingerprint is persisted.
pub fn generate_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    to_hex(&bytes)
}

/// Fingerprint a session token for storage and lookup: SHA-256, hex-encoded.
/// The sessions table stores and is queried by this value, so it never holds a
/// token that could be replayed if the table leaked.
pub fn hash_token(token: &str) -> String {
    to_hex(&Sha256::digest(token.as_bytes()))
}

fn to_hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0x0f) as u32, 16).unwrap());
    }
    s
}

/// Encrypt a cookie string (or any plaintext) using AES-256-GCM.
/// The 96-bit nonce is generated randomly and prepended to the ciphertext.
pub fn encrypt_cookie(plaintext: &str, key: &[u8; 32]) -> Result<Vec<u8>> {
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng); // 96-bits; unique per message
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|e| Error::Encryption(e.to_string()))?;

    // Prepend nonce to ciphertext
    let mut payload = nonce.to_vec();
    payload.extend(ciphertext);
    Ok(payload)
}

/// Decrypt a cookie payload (nonce + ciphertext) using AES-256-GCM.
pub fn decrypt_cookie(payload: &[u8], key: &[u8; 32]) -> Result<String> {
    if payload.len() < 12 {
        return Err(Error::Decryption("payload too short".into()));
    }
    let (nonce_bytes, ciphertext) = payload.split_at(12);
    let cipher = Aes256Gcm::new(Key::<Aes256Gcm>::from_slice(key));
    let nonce = Nonce::from_slice(nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|e| Error::Decryption(e.to_string()))?;

    String::from_utf8(plaintext).map_err(|_| Error::Decryption("invalid utf8".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn password_roundtrip() {
        let hash = hash_password("correct horse battery staple").unwrap();
        assert!(verify_password("correct horse battery staple", &hash));
        assert!(!verify_password("Correct Horse Battery Staple", &hash));
        assert!(!verify_password("", &hash));
    }

    #[test]
    fn hash_is_argon2id_and_salted() {
        let h1 = hash_password("same").unwrap();
        let h2 = hash_password("same").unwrap();
        assert!(
            h1.starts_with("$argon2id$"),
            "expected argon2id PHC, got {h1}"
        );
        assert_ne!(h1, h2, "random salt should make each hash unique");
    }

    #[test]
    fn malformed_hash_verifies_false_not_panic() {
        assert!(!verify_password("whatever", "not-a-phc-string"));
        assert!(!verify_password("whatever", ""));
    }

    #[test]
    fn tokens_are_unique_256_bit_hex() {
        let a = generate_token();
        let b = generate_token();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "two tokens must differ");
    }

    #[test]
    fn token_fingerprint_is_stable_sha256_hex() {
        let t = "abc";
        assert_eq!(hash_token(t), hash_token(t), "deterministic");
        assert_eq!(hash_token(t).len(), 64);
        assert_ne!(hash_token("abc"), hash_token("abd"));
        // SHA-256("abc") = ba7816bf8f01cfea..., sanity-checking the encoding.
        assert!(hash_token("abc").starts_with("ba7816bf8f01cfea"));
    }
}
