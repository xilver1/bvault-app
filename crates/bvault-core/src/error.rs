//! Error types for bvault-core

use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    
    #[error("Binary format error: {0}")]
    BinRw(String),
    
    #[error("Audio decoding error: {0}")]
    AudioDecode(String),
    
    #[error("Analysis error: {0}")]
    Analysis(String),
    
    #[error("Invalid track: {0}")]
    InvalidTrack(String),
    
    #[error("Cache error: {0}")]
    Cache(String),
    
    #[error("Path error: {0}")]
    Path(String),
    
    #[error("String encoding error: {0}")]
    StringEncoding(String),
    
    #[error("Page overflow: {0}")]
    PageOverflow(String),
    
    #[error("Database error: {0}")]
    Database(String),

    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Device library error: {0}")]
    DeviceLibrary(String),
}

pub type Result<T> = std::result::Result<T, Error>;

impl From<binrw::Error> for Error {
    fn from(e: binrw::Error) -> Self {
        Error::BinRw(e.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Cache(e.to_string())
    }
}