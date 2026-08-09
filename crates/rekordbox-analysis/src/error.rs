//! Analysis errors.
//!
//! Kept separate from `rekordbox_core::Error` on purpose: decode failures are a
//! concern of this crate and shouldn't leak Symphonia types into the format
//! library. A missing tempo or key is *not* an error — those degrade to defaults
//! inside the engine; only genuinely undecodable input fails here.

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    /// Symphonia could not probe, decode, or read a packet.
    #[error("decode: {0}")]
    Decode(String),

    /// The container has no default (primary) audio track to analyze.
    #[error("no default audio track in stream")]
    NoDefaultTrack,

    /// The codec reported no sample rate — we cannot build a time base.
    #[error("stream has unknown sample rate")]
    UnknownSampleRate,

    /// Hashing the input for its content identity failed.
    #[error("hash: {0}")]
    Hash(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<symphonia::core::errors::Error> for Error {
    fn from(e: symphonia::core::errors::Error) -> Self {
        Error::Decode(e.to_string())
    }
}