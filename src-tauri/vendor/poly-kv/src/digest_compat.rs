//! Digest type abstraction that compiles with or without `stack-ids`.
//!
//! When `typed-ids` is active, `Digest` is `stack_ids::ContentDigest`.
//! When inactive, `Digest` is a local newtype wrapping `String`.
//!
//! Both paths expose the same method signatures so callers don't need
//! cfg-gates at every call site.

#[cfg(feature = "typed-ids")]
pub use stack_ids::ContentDigest as Digest;

#[cfg(feature = "typed-ids")]
impl From<stack_ids::DigestError> for crate::error::PolyKvError {
    fn from(e: stack_ids::DigestError) -> Self {
        crate::error::PolyKvError::Internal(format!("digest error: {:?}", e))
    }
}

/// Unified wrapper around `ContentDigest::compute_json` that returns
/// `Result<Digest, PolyKvError>` regardless of feature configuration.
/// Call this instead of `Digest::compute_json` directly to avoid
/// `DigestError` → `PolyKvError` conversion issues.
#[cfg(feature = "typed-ids")]
pub fn compute_json<T: serde::Serialize>(value: &T) -> crate::error::Result<Digest> {
    Digest::compute_json(value).map_err(crate::error::PolyKvError::from)
}

/// Unified wrapper around `ContentDigest::compute` (raw bytes).
#[cfg(feature = "typed-ids")]
pub fn compute(data: &[u8]) -> Digest {
    Digest::compute(data)
}

/// Unified wrapper around `ContentDigest::compute_str`.
#[cfg(feature = "typed-ids")]
pub fn compute_str(data: &str) -> Digest {
    Digest::compute_str(data)
}

// ── Fallback (no typed-ids) ────────────────────────────────────────

#[cfg(not(feature = "typed-ids"))]
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct Digest(pub String);

#[cfg(not(feature = "typed-ids"))]
impl Digest {
    pub fn compute(data: &[u8]) -> Self {
        Self(blake3::hash(data).to_hex().to_string())
    }

    pub fn compute_str(data: &str) -> Self {
        Self::compute(data.as_bytes())
    }

    pub fn compute_json<T: serde::Serialize>(value: &T) -> crate::error::Result<Self> {
        internal_compute_json(value)
    }

    pub fn hex(&self) -> &str {
        &self.0
    }

    pub fn from_hex_unchecked(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
}

/// Unified wrapper (same signature as the typed-ids path).
#[cfg(not(feature = "typed-ids"))]
pub fn compute_json<T: serde::Serialize>(value: &T) -> crate::error::Result<Digest> {
    internal_compute_json(value)
}

#[cfg(not(feature = "typed-ids"))]
pub fn compute(data: &[u8]) -> Digest {
    Digest::compute(data)
}

#[cfg(not(feature = "typed-ids"))]
pub fn compute_str(data: &str) -> Digest {
    Digest::compute_str(data)
}

#[cfg(not(feature = "typed-ids"))]
fn internal_compute_json<T: serde::Serialize>(value: &T) -> crate::error::Result<Digest> {
    let json = serde_json::to_string(value)
        .map_err(|e| crate::error::PolyKvError::Internal(format!("digest json: {}", e)))?;
    // Canonicalize JSON key ordering for determinism
    let val: serde_json::Value = serde_json::from_str(&json)
        .map_err(|e| crate::error::PolyKvError::Internal(format!("digest parse: {}", e)))?;
    let canonical = canonicalize_json_value(val);
    let canonical_str = serde_json::to_string(&canonical)
        .map_err(|e| crate::error::PolyKvError::Internal(format!("digest canonicalize: {}", e)))?;
    Ok(Digest(
        blake3::hash(canonical_str.as_bytes()).to_hex().to_string(),
    ))
}

#[cfg(not(feature = "typed-ids"))]
fn canonicalize_json_value(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(map) => {
            let mut entries: Vec<(String, serde_json::Value)> = map
                .into_iter()
                .map(|(k, v)| (k, canonicalize_json_value(v)))
                .collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut ordered = serde_json::Map::new();
            for (k, v) in entries {
                ordered.insert(k, v);
            }
            serde_json::Value::Object(ordered)
        }
        serde_json::Value::Array(items) => {
            serde_json::Value::Array(items.into_iter().map(canonicalize_json_value).collect())
        }
        other => other,
    }
}
