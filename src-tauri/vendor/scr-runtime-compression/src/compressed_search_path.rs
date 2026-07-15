//! `CompressedSearchPath` — a search path wrapper that carries compression metadata.
//!
//! Wraps a "normal" search path (e.g. a list of namespaces or collection identifiers)
//! and annotates it with codec identity and provenance so that the semantic-memory
//! runtime can make governance decisions about whether to use compressed lookup,
//! exact fallback, or a hybrid path.

use serde::{Deserialize, Serialize};

use super::CodecId;

/// A search path annotated with compression context.
///
/// When a query enters the semantic-memory runtime, it may carry compressed
/// index artifacts (e.g. turbo-quant polar codes or fib-quant radial sketches).
/// `CompressedSearchPath` wraps the raw search path + the codec metadata needed
/// to route the query correctly.
///
/// ## Type parameters
/// - `P` — the underlying search path type (e.g. `Vec<NamespaceId>`, `ScopeFilter`, etc.)
///   must be `Send + Sync` so the wrapper can be used across async boundaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(serialize = "P: Serialize", deserialize = "P: Deserialize<'de>"))]
pub struct CompressedSearchPath<P> {
    /// The raw search path being executed.
    path: P,

    /// Which codec compressed the index artifacts associated with this path.
    codec_id: CodecId,

    /// When this compressed path was produced (for audit and staleness checks).
    #[serde(skip_serializing_if = "Option::is_none")]
    produced_at: Option<chrono::DateTime<chrono::Utc>>,

    /// Human-readable provenance label, e.g. "turbo-quant:polar:v2" or "fib-quant:radial:1".
    /// Used in logs and receipts.
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance_label: Option<String>,

    /// Whether this path is allowed to use lossy approximate decode.
    /// `false` means every decode must produce an exact (fallback) result.
    approximate_allowed: bool,
}

impl<P> CompressedSearchPath<P>
where
    P: Send + Sync,
{
    /// Wrap an existing search path with compression context.
    pub fn new(path: P, codec_id: CodecId) -> Self {
        Self {
            path,
            codec_id,
            produced_at: None,
            provenance_label: None,
            approximate_allowed: true,
        }
    }

    /// Set the `produced_at` timestamp.
    pub fn produced_at(mut self, ts: chrono::DateTime<chrono::Utc>) -> Self {
        self.produced_at = Some(ts);
        self
    }

    /// Set the provenance label.
    pub fn provenance_label(mut self, label: impl Into<String>) -> Self {
        self.provenance_label = Some(label.into());
        self
    }

    /// Disallow approximate decode — require exact fallback on every decode.
    pub fn require_exact(mut self) -> Self {
        self.approximate_allowed = false;
        self
    }

    /// Returns the codec identity.
    pub fn codec_id(&self) -> CodecId {
        self.codec_id
    }

    /// Returns the wrapped search path by reference.
    pub fn path(&self) -> &P {
        &self.path
    }

    /// Consumes the wrapper and returns the inner search path.
    pub fn into_path(self) -> P {
        self.path
    }

    /// Returns `true` if approximate decode is allowed on this path.
    pub fn approximate_allowed(&self) -> bool {
        self.approximate_allowed
    }

    /// Returns `true` if this path requires an exact fallback decode.
    pub fn requires_exact_fallback(&self) -> bool {
        self.codec_id.requires_exact_fallback() || !self.approximate_allowed
    }

    /// Map the inner path type using `f`.
    ///
    /// This allows transforming the wrapped path type without touching the
    /// compression metadata.
    pub fn map_path<Q, F>(self, f: F) -> CompressedSearchPath<Q>
    where
        F: FnOnce(P) -> Q,
    {
        CompressedSearchPath {
            path: f(self.path),
            codec_id: self.codec_id,
            produced_at: self.produced_at,
            provenance_label: self.provenance_label,
            approximate_allowed: self.approximate_allowed,
        }
    }
}

impl<P> std::fmt::Display for CompressedSearchPath<P>
where
    P: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "CompressedSearchPath(codec={}, approx={}, path={:?})",
            self.codec_id, self.approximate_allowed, self.path
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compressed_search_path_build() {
        let path = vec!["ns1", "ns2"];
        let csp = CompressedSearchPath::new(path, CodecId::TurboQuant)
            .require_exact()
            .provenance_label("test:v1");

        assert_eq!(csp.codec_id(), CodecId::TurboQuant);
        assert!(!csp.approximate_allowed());
        assert!(csp.requires_exact_fallback());
    }

    #[test]
    fn codec_id_requires_exact_fallback() {
        assert!(CodecId::TurboQuant.requires_exact_fallback());
        assert!(CodecId::FibQuant.requires_exact_fallback());
        assert!(!CodecId::Uncompressed.requires_exact_fallback());
    }

    #[test]
    fn map_path_transforms_inner() {
        let csp: CompressedSearchPath<Vec<&str>> =
            CompressedSearchPath::new(vec!["a", "b"], CodecId::FibQuant);
        let mapped: CompressedSearchPath<Vec<String>> =
            csp.map_path(|v| v.into_iter().map(String::from).collect());
        assert_eq!(mapped.into_path(), vec!["a", "b"]);
    }
}
