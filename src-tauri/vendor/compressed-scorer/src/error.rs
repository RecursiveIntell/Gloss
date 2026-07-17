//! Error types for compressed scoring

use core::fmt;

#[cfg(feature = "no_std")]
use alloc::string::String;

#[derive(Debug)]
pub enum ScorerError {
    /// Dimension mismatch between query and stored vectors
    DimensionMismatch { expected: usize, got: usize },
    /// Invalid compressed data (corrupt, wrong codec, etc.)
    CorruptPayload(String),
    /// Codec not available (feature flag disabled)
    CodecUnavailable(&'static str),
    /// Scoring failed (non-finite values, numerical issues)
    ScoringFailed(String),
    /// Empty query or empty candidate set
    Empty,
}

impl fmt::Display for ScorerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DimensionMismatch { expected, got } => {
                write!(f, "dimension mismatch: expected {expected}, got {got}")
            }
            Self::CorruptPayload(msg) => write!(f, "corrupt payload: {msg}"),
            Self::CodecUnavailable(name) => {
                write!(f, "codec '{name}' not available (feature flag disabled?)")
            }
            Self::ScoringFailed(msg) => write!(f, "scoring failed: {msg}"),
            Self::Empty => write!(f, "empty query or candidate set"),
        }
    }
}

#[cfg(not(feature = "no_std"))]
impl std::error::Error for ScorerError {}

pub type ScorerResult<T> = core::result::Result<T, ScorerError>;
