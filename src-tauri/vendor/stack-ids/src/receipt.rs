//! Receipt module for stack-ids.
//!
//! Receipts are emitted at the crate's canonical output boundary for auditability.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Audit receipt for stack-ids operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CrateReceiptV1 {
    pub crate_name: &'static str,
    pub operation: String,
    pub timestamp: DateTime<Utc>,
    pub digest: String,
    pub outcome: Outcome,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Outcome {
    Success,
    Failure { reason: String },
}

impl CrateReceiptV1 {
    /// Emit a success receipt.
    pub fn success(operation: impl Into<String>, digest: impl Into<String>) -> Self {
        Self {
            crate_name: env!("CARGO_PKG_NAME"),
            operation: operation.into(),
            timestamp: Utc::now(),
            digest: digest.into(),
            outcome: Outcome::Success,
        }
    }

    /// Emit a failure receipt.
    pub fn failure(operation: impl Into<String>, digest: impl Into<String>, reason: impl Into<String>) -> Self {
        Self {
            crate_name: env!("CARGO_PKG_NAME"),
            operation: operation.into(),
            timestamp: Utc::now(),
            digest: digest.into(),
            outcome: Outcome::Failure { reason: reason.into() },
        }
    }
}
