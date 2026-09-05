//! Error types for boundary-compiler.

use thiserror::Error;

/// Errors that can occur during JCS canonicalization or boundary profile processing.
#[derive(Debug, Error)]
pub enum JcsError {
    /// Duplicate object key encountered (violates RFC 8785 §2.7).
    #[error("duplicate object key: {key:?}")]
    DuplicateKey { key: String },

    /// Invalid JSON structure during canonicalization.
    #[error("invalid JSON: {reason}")]
    InvalidJson { reason: String },

    /// JSON lexer/syntax error.
    #[error("JSON parse error: {0}")]
    ParseError(#[from] serde_json::Error),

    /// Schema validation failed.
    #[error("schema validation failed: {0}")]
    SchemaValidation(String),

    /// JSON Schema initialization error.
    #[error("JSON Schema error: {0}")]
    SchemaError(String),

    /// ContentDigest computation error.
    #[error("digest error: {0}")]
    DigestError(String),

    /// Invalid profile configuration.
    #[error("invalid profile: {reason}")]
    InvalidProfile { reason: String },

    /// Resource ceiling exceeded.
    #[error("resource ceiling exceeded: {resource} ({used} / {limit})")]
    ResourceCeilingExceeded {
        resource: String,
        used: usize,
        limit: usize,
    },
}
