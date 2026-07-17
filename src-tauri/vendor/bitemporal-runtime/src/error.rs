//! Bitemporal error types.

use thiserror::Error;

/// Errors produced by bitemporal-runtime operations.
#[derive(Debug, Error)]
pub enum BitemporalError {
    /// No record found for the given ID.
    #[error("record not found: {0}")]
    RecordNotFound(String),

    /// Duplicate record ID in append operation.
    #[error("duplicate record ID: {0}")]
    DuplicateRecordId(String),

    /// Invalid time range (valid_time > recorded_time or similar).
    #[error("invalid time range: {0}")]
    InvalidTimeRange(String),

    /// No supersession possible — no prior record to supersede.
    #[error("no prior record to supersede for ID: {0}")]
    NoPriorRecord(String),

    /// Database error from underlying storage.
    #[error("database error: {0}")]
    DatabaseError(String),

    /// Serialization error.
    #[error("serialization error: {0}")]
    SerializationError(String),
}
