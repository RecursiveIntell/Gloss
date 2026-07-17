//! Error types for the compression runtime adapter.

use thiserror::Error;

/// Errors that can occur during compression operations.
#[derive(Debug, Error)]
pub enum CompressionError {
    #[error("codec `{0}` is not available in this build")]
    CodecNotAvailable(String),

    #[error("encode failed: {0}")]
    EncodeFailed(String),

    #[error("serialization failed: {0}")]
    SerializationFailed(String),

    #[error("quant-governor policy rejected compression: {0}")]
    PolicyRejected(String),
}

/// Errors that can occur during decompression operations.
#[derive(Debug, Error)]
pub enum DecompressError {
    #[error("codec `{0}` is not available in this build")]
    CodecNotAvailable(String),

    #[error("decode failed: {0}")]
    DecodeFailed(String),

    #[error("deserialization failed: {0}")]
    DeserializationFailed(String),

    #[error("compressed data is corrupted or truncated: expected {expected} bytes, got {actual}")]
    TruncatedData { expected: usize, actual: usize },

    #[error("exact fallback required but no fallback decoder provided")]
    NoFallbackProvided,
}
