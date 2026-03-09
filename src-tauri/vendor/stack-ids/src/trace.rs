//! Trace context primitives for cross-crate and cross-boundary correlation.
//!
//! ## Trace law (from MASTER_SUPPORTING_DELTA §4.3)
//!
//! - `TraceCtx` is canonical in-process.
//! - Across queue/network boundaries, preserve W3C trace context plus bounded baggage only.
//! - No large opaque blobs. No sensitive payloads in baggage.
//!
//! ## W3C Trace Context
//!
//! The `traceparent` header format is:
//! ```text
//! {version}-{trace-id}-{parent-id}-{trace-flags}
//! 00-{32 hex chars}-{16 hex chars}-{2 hex chars}
//! ```

use serde::{Deserialize, Serialize};

/// In-process trace context for cross-crate correlation.
///
/// Wraps a W3C-compatible trace ID and optional parent span ID.
/// Additional bounded baggage can be attached for cross-boundary metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceCtx {
    /// The trace ID (W3C: 32 hex chars, or any opaque string for legacy compat).
    pub trace_id: String,
    /// Optional parent span ID (W3C: 16 hex chars).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    /// Bounded baggage for cross-boundary metadata.
    /// Keys and values must be short ASCII strings. No sensitive data.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub baggage: Vec<BaggageEntry>,
}

/// Maximum number of baggage entries allowed.
pub const MAX_BAGGAGE_ENTRIES: usize = 16;

/// Maximum byte length for a single baggage key or value.
pub const MAX_BAGGAGE_ITEM_BYTES: usize = 256;

/// A single baggage entry (key-value pair).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct BaggageEntry {
    pub key: String,
    pub value: String,
}

impl TraceCtx {
    /// Create a new trace context with a generated trace ID (UUID v4 hex).
    pub fn generate() -> Self {
        let trace_id = uuid::Uuid::new_v4().as_simple().to_string();
        Self {
            trace_id,
            parent_id: None,
            baggage: Vec::new(),
        }
    }

    /// Create from a raw trace ID string.
    pub fn from_trace_id(trace_id: impl Into<String>) -> Self {
        Self {
            trace_id: trace_id.into(),
            parent_id: None,
            baggage: Vec::new(),
        }
    }

    /// Create from a legacy `TraceId(String)` value for migration compatibility.
    ///
    /// Phase status: compatibility / migration-only
    pub fn from_legacy_trace_id(legacy_id: impl Into<String>) -> Self {
        Self::from_trace_id(legacy_id)
    }

    /// Extract the trace ID as a string for legacy interop.
    ///
    /// Phase status: compatibility / migration-only
    pub fn to_legacy_trace_id(&self) -> &str {
        &self.trace_id
    }

    /// Set the parent span ID.
    pub fn with_parent(mut self, parent_id: impl Into<String>) -> Self {
        self.parent_id = Some(parent_id.into());
        self
    }

    /// Create a child context: same trace ID, new parent.
    pub fn child(&self, span_id: impl Into<String>) -> Self {
        Self {
            trace_id: self.trace_id.clone(),
            parent_id: Some(span_id.into()),
            baggage: self.baggage.clone(),
        }
    }

    /// Add a baggage entry. Returns Err if limits are exceeded.
    pub fn add_baggage(
        &mut self,
        key: impl Into<String>,
        value: impl Into<String>,
    ) -> Result<(), TraceError> {
        let key = key.into();
        let value = value.into();

        if self.baggage.len() >= MAX_BAGGAGE_ENTRIES {
            return Err(TraceError::BaggageLimitExceeded {
                max: MAX_BAGGAGE_ENTRIES,
            });
        }
        if key.len() > MAX_BAGGAGE_ITEM_BYTES {
            return Err(TraceError::BaggageItemTooLarge {
                field: "key".into(),
                len: key.len(),
                max: MAX_BAGGAGE_ITEM_BYTES,
            });
        }
        if value.len() > MAX_BAGGAGE_ITEM_BYTES {
            return Err(TraceError::BaggageItemTooLarge {
                field: "value".into(),
                len: value.len(),
                max: MAX_BAGGAGE_ITEM_BYTES,
            });
        }

        self.baggage.push(BaggageEntry { key, value });
        Ok(())
    }

    /// Get a baggage value by key.
    pub fn baggage_value(&self, key: &str) -> Option<&str> {
        self.baggage
            .iter()
            .find(|e| e.key == key)
            .map(|e| e.value.as_str())
    }

    /// Format as a W3C traceparent header.
    ///
    /// Format: `00-{trace_id}-{parent_id}-01`
    ///
    /// If the trace ID is not exactly 32 hex chars (e.g., a legacy opaque string),
    /// it is converted via deterministic BLAKE3 hash truncation using
    /// [`hash_to_w3c_trace_id`]. Padding and truncation are forbidden.
    ///
    /// If no parent ID exists, a zero span ID is used.
    /// Non-compliant parent IDs (not 16 hex chars) are rejected.
    pub fn to_traceparent(&self) -> Result<String, TraceError> {
        let trace_id = if is_w3c_trace_id(&self.trace_id) {
            self.trace_id.clone()
        } else {
            hash_to_w3c_trace_id(&self.trace_id)
        };
        let parent_id = match &self.parent_id {
            Some(p) if is_w3c_span_id(p) => p.clone(),
            Some(p) => {
                return Err(TraceError::InvalidTraceparent {
                    reason: format!(
                        "parent_id must be 16 hex chars, got {} chars: '{}'",
                        p.len(),
                        p
                    ),
                });
            }
            None => "0000000000000000".to_string(),
        };
        Ok(format!("00-{trace_id}-{parent_id}-01"))
    }

    /// Parse from a W3C traceparent header.
    ///
    /// Format: `{version}-{trace_id}-{parent_id}-{flags}`
    pub fn from_traceparent(header: &str) -> Result<Self, TraceError> {
        let parts: Vec<&str> = header.split('-').collect();
        if parts.len() != 4 {
            return Err(TraceError::InvalidTraceparent {
                reason: format!("expected 4 dash-separated parts, got {}", parts.len()),
            });
        }

        let version = parts[0];
        if version != "00" {
            return Err(TraceError::InvalidTraceparent {
                reason: format!("unsupported version: {version}"),
            });
        }

        let trace_id = parts[1].to_string();
        if trace_id.len() != 32 || !trace_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(TraceError::InvalidTraceparent {
                reason: "trace-id must be 32 hex characters".into(),
            });
        }

        let parent_id = parts[2].to_string();
        if parent_id.len() != 16 || !parent_id.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(TraceError::InvalidTraceparent {
                reason: "parent-id must be 16 hex characters".into(),
            });
        }

        let parent = if parent_id == "0000000000000000" {
            None
        } else {
            Some(parent_id)
        };

        Ok(Self {
            trace_id,
            parent_id: parent,
            baggage: Vec::new(),
        })
    }
}

/// Errors related to trace context operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TraceError {
    /// Too many baggage entries.
    BaggageLimitExceeded { max: usize },
    /// A single baggage key or value exceeds the size limit.
    BaggageItemTooLarge {
        field: String,
        len: usize,
        max: usize,
    },
    /// Invalid traceparent header format.
    InvalidTraceparent { reason: String },
}

impl std::fmt::Display for TraceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BaggageLimitExceeded { max } => {
                write!(f, "baggage limit exceeded (max {max} entries)")
            }
            Self::BaggageItemTooLarge { field, len, max } => {
                write!(f, "baggage {field} too large ({len} bytes, max {max})")
            }
            Self::InvalidTraceparent { reason } => {
                write!(f, "invalid traceparent: {reason}")
            }
        }
    }
}

impl std::error::Error for TraceError {}

/// Check if a string is a valid W3C trace ID (exactly 32 hex chars).
fn is_w3c_trace_id(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Check if a string is a valid W3C span ID (exactly 16 hex chars).
fn is_w3c_span_id(id: &str) -> bool {
    id.len() == 16 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// Convert a non-W3C trace ID to a W3C-compliant 32-hex-char wire ID
/// via deterministic BLAKE3 hash truncation.
///
/// The same input always produces the same output.
/// The original legacy identifier should be preserved in baggage
/// under the key `legacy_trace_id` by the caller if round-trip is needed.
///
/// Padding and truncation are forbidden. This is the only canonical
/// conversion path for non-W3C trace IDs.
pub fn hash_to_w3c_trace_id(legacy_id: &str) -> String {
    let hash = blake3::hash(legacy_id.as_bytes());
    let bytes = hash.as_bytes();
    // Take first 16 bytes (128 bits) = 32 hex chars, matching W3C trace-id length.
    bytes[..16].iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_ctx_generate() {
        let ctx = TraceCtx::generate();
        assert_eq!(ctx.trace_id.len(), 32);
        assert!(ctx.parent_id.is_none());
        assert!(ctx.baggage.is_empty());
    }

    #[test]
    fn trace_ctx_child() {
        let parent = TraceCtx::generate();
        let child = parent.child("abcdef0123456789");
        assert_eq!(child.trace_id, parent.trace_id);
        assert_eq!(child.parent_id.as_deref(), Some("abcdef0123456789"));
    }

    #[test]
    fn traceparent_roundtrip() {
        let ctx = TraceCtx::from_trace_id("0af7651916cd43dd8448eb211c80319c")
            .with_parent("b7ad6b7169203331");
        let header = ctx.to_traceparent().unwrap();
        assert_eq!(
            header,
            "00-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01"
        );

        let parsed = TraceCtx::from_traceparent(&header).unwrap();
        assert_eq!(parsed.trace_id, "0af7651916cd43dd8448eb211c80319c");
        assert_eq!(parsed.parent_id.as_deref(), Some("b7ad6b7169203331"));
    }

    #[test]
    fn traceparent_no_parent() {
        let ctx = TraceCtx::from_trace_id("0af7651916cd43dd8448eb211c80319c");
        let header = ctx.to_traceparent().unwrap();
        assert!(header.contains("0000000000000000"));

        let parsed = TraceCtx::from_traceparent(&header).unwrap();
        assert!(parsed.parent_id.is_none());
    }

    #[test]
    fn traceparent_legacy_trace_id_uses_hash() {
        let ctx = TraceCtx::from_trace_id("old-trace-abc");
        let header = ctx.to_traceparent().unwrap();
        // The legacy ID is hashed, not padded/truncated.
        let parts: Vec<&str> = header.split('-').collect();
        assert_eq!(parts[0], "00");
        assert_eq!(parts[1].len(), 32);
        assert!(parts[1].chars().all(|c| c.is_ascii_hexdigit()));
        // Deterministic: same input always produces same output.
        let header2 = ctx.to_traceparent().unwrap();
        assert_eq!(header, header2);
    }

    #[test]
    fn traceparent_rejects_non_w3c_parent_id() {
        let ctx = TraceCtx::from_trace_id("0af7651916cd43dd8448eb211c80319c")
            .with_parent("not-hex-parent");
        let err = ctx.to_traceparent().unwrap_err();
        assert!(matches!(err, TraceError::InvalidTraceparent { .. }));
    }

    #[test]
    fn hash_to_w3c_trace_id_is_deterministic() {
        let id1 = super::hash_to_w3c_trace_id("legacy-id-123");
        let id2 = super::hash_to_w3c_trace_id("legacy-id-123");
        assert_eq!(id1, id2);
        assert_eq!(id1.len(), 32);
        assert!(id1.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn hash_to_w3c_trace_id_different_inputs_differ() {
        let id1 = super::hash_to_w3c_trace_id("legacy-id-123");
        let id2 = super::hash_to_w3c_trace_id("legacy-id-456");
        assert_ne!(id1, id2);
    }

    #[test]
    fn traceparent_invalid_format() {
        let err = TraceCtx::from_traceparent("bad").unwrap_err();
        assert!(matches!(err, TraceError::InvalidTraceparent { .. }));
    }

    #[test]
    fn traceparent_unsupported_version() {
        let err =
            TraceCtx::from_traceparent("01-0af7651916cd43dd8448eb211c80319c-b7ad6b7169203331-01")
                .unwrap_err();
        assert!(matches!(err, TraceError::InvalidTraceparent { .. }));
    }

    #[test]
    fn baggage_add_and_get() {
        let mut ctx = TraceCtx::generate();
        ctx.add_baggage("env", "prod").unwrap();
        assert_eq!(ctx.baggage_value("env"), Some("prod"));
        assert_eq!(ctx.baggage_value("missing"), None);
    }

    #[test]
    fn baggage_limit_enforced() {
        let mut ctx = TraceCtx::generate();
        for i in 0..MAX_BAGGAGE_ENTRIES {
            ctx.add_baggage(format!("k{i}"), "v").unwrap();
        }
        let err = ctx.add_baggage("overflow", "v").unwrap_err();
        assert!(matches!(err, TraceError::BaggageLimitExceeded { .. }));
    }

    #[test]
    fn baggage_size_limit_enforced() {
        let mut ctx = TraceCtx::generate();
        let big_key = "x".repeat(MAX_BAGGAGE_ITEM_BYTES + 1);
        let err = ctx.add_baggage(big_key, "v").unwrap_err();
        assert!(matches!(err, TraceError::BaggageItemTooLarge { .. }));
    }

    #[test]
    fn legacy_trace_id_compat() {
        let ctx = TraceCtx::from_legacy_trace_id("old-trace-abc");
        assert_eq!(ctx.to_legacy_trace_id(), "old-trace-abc");
    }

    #[test]
    fn trace_ctx_serde_roundtrip() {
        let mut ctx = TraceCtx::generate().with_parent("abcdef0123456789");
        ctx.add_baggage("env", "test").unwrap();
        let json = serde_json::to_string(&ctx).unwrap();
        let back: TraceCtx = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ctx);
    }
}
