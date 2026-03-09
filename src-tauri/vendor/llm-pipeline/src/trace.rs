//! Cross-crate trace identifier for correlating operations.
//!
//! ## Phase status: compatibility / migration-only
//!
//! [`TraceId`] is a crate-local trace identifier retained for backward
//! compatibility. The canonical replacement is `stack_ids::TraceCtx`, which
//! adds parent span tracking, bounded baggage, and W3C traceparent support.
//!
//! **This type will be removed in v1.0.** Migrate to `stack_ids::TraceCtx` now.
//!
//! **Removal condition**: removed when all callers migrate to `TraceCtx`.
//!
//! Use [`TraceId::to_trace_ctx()`] and [`TraceId::from_trace_ctx()`] to
//! convert between the legacy and canonical types during migration.

use serde::{Deserialize, Serialize};

/// A unique identifier that correlates all artifacts from a single operation.
///
/// ## Phase status: compatibility / migration-only
///
/// This is a crate-local trace identifier. The canonical replacement is
/// `stack_ids::TraceCtx`. Use [`to_trace_ctx()`](Self::to_trace_ctx) and
/// [`from_trace_ctx()`](Self::from_trace_ctx) for interop.
///
/// **This type will be removed in v1.0.** Migrate to `stack_ids::TraceCtx` now.
///
/// **Removal condition**: removed when all callers migrate to `TraceCtx`.
///
/// # Example
///
/// ```
/// #[allow(deprecated)]
/// use llm_pipeline::TraceId;
///
/// #[allow(deprecated)]
/// let id = TraceId::new();
/// println!("trace: {}", id);
///
/// #[allow(deprecated)]
/// let id2 = TraceId::from_string("custom-trace-123");
/// assert_eq!(id2.as_str(), "custom-trace-123");
/// ```
#[deprecated(since = "0.6.0", note = "Use stack_ids::TraceCtx instead. This type will be removed in v1.0. Retained for backward compatibility only.")]
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceId(pub String);

impl TraceId {
    /// Generate a new random trace ID (UUID v4).
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Create a trace ID from an existing string.
    pub fn from_string(s: &str) -> Self {
        Self(s.to_owned())
    }

    /// Return the inner string slice.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Convert to canonical `stack_ids::TraceCtx`.
    ///
    /// Phase status: compatibility / migration-only
    pub fn to_trace_ctx(&self) -> stack_ids::TraceCtx {
        stack_ids::TraceCtx::from_legacy_trace_id(&self.0)
    }

    /// Create from canonical `stack_ids::TraceCtx`.
    ///
    /// Phase status: compatibility / migration-only
    pub fn from_trace_ctx(ctx: &stack_ids::TraceCtx) -> Self {
        Self(ctx.to_legacy_trace_id().to_string())
    }
}

impl Default for TraceId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for TraceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for TraceId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for TraceId {
    fn from(s: &str) -> Self {
        Self(s.to_owned())
    }
}

#[allow(deprecated)]
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_trace_id_new_is_unique() {
        let id1 = TraceId::new();
        let id2 = TraceId::new();
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_trace_id_from_string() {
        let id = TraceId::from_string("abc-123");
        assert_eq!(id.as_str(), "abc-123");
        assert_eq!(id.to_string(), "abc-123");
    }

    #[test]
    fn test_trace_id_display() {
        let id = TraceId::from_string("test-id");
        assert_eq!(format!("{}", id), "test-id");
    }

    #[test]
    fn test_trace_id_default() {
        let id = TraceId::default();
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_trace_id_serde_roundtrip() {
        let id = TraceId::from_string("serde-test");
        let json = serde_json::to_string(&id).unwrap();
        let back: TraceId = serde_json::from_str(&json).unwrap();
        assert_eq!(id, back);
    }

    #[test]
    fn test_trace_id_from_conversions() {
        let from_str: TraceId = "hello".into();
        assert_eq!(from_str.as_str(), "hello");

        let from_string: TraceId = String::from("world").into();
        assert_eq!(from_string.as_str(), "world");
    }
}
