use std::io;

/// Errors of the media engine.
#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error(transparent)]
    Io(#[from] io::Error),
    /// the bytes are not an image this build can read
    #[error("Input file contains unsupported image format")]
    Unsupported,
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("encode failed: {0}")]
    Encode(String),
    #[error("image too large: {0} pixels")]
    TooLarge(u64),
    /// a feature (heif, svg) is not compiled in
    #[error("{0} support is not built in")]
    NotBuiltIn(&'static str),
    #[error("{0}")]
    Ffmpeg(String),
    #[error("{0} timed out")]
    Timeout(String),
    #[error("Canceled")]
    Canceled,
}

pub type Result<T, E = MediaError> = std::result::Result<T, E>;
