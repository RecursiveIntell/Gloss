//! Canonical content digest computation.
//!
//! ## Digest law (from MASTER_SUPPORTING_DELTA §7)
//!
//! The digest algorithm for export/import idempotency is:
//! - Deterministic canonical serialization (JSON with sorted keys, no trailing whitespace)
//! - UTF-8 encoding
//! - BLAKE3 hash
//! - Hex-encoded output (64 chars)
//!
//! The digest domain (which fields are included) is defined per envelope type.
//! Bridge and importer must agree exactly on which fields are digested.
//!
//! ## Usage
//!
//! ```
//! use stack_ids::ContentDigest;
//!
//! let digest = ContentDigest::compute(b"hello world");
//! assert_eq!(digest.hex().len(), 64);
//! ```

use serde::{Deserialize, Serialize};

/// A BLAKE3 content digest for idempotent deduplication.
///
/// The inner value is a 64-character hex string representing the BLAKE3 hash.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentDigest(pub String);

impl ContentDigest {
    /// Compute a BLAKE3 digest from raw bytes.
    pub fn compute(data: &[u8]) -> Self {
        let hash = blake3::hash(data);
        Self(hash.to_hex().to_string())
    }

    /// Compute a BLAKE3 digest from a UTF-8 string.
    pub fn compute_str(data: &str) -> Self {
        Self::compute(data.as_bytes())
    }

    /// Compute a digest from a JSON-serializable value using canonical serialization.
    ///
    /// Canonical serialization means:
    /// - `serde_json::to_string()` (compact, no trailing whitespace)
    /// - The caller is responsible for ensuring field ordering is stable
    ///   (use `#[serde(sort_keys)]` or `BTreeMap` for maps).
    ///
    /// For structured data with guaranteed field order (structs with named fields),
    /// serde_json produces deterministic output by default.
    pub fn compute_json<T: Serialize>(value: &T) -> Result<Self, DigestError> {
        let canonical =
            serde_json::to_string(value).map_err(|e| DigestError::SerializationFailed {
                reason: e.to_string(),
            })?;
        Ok(Self::compute_str(&canonical))
    }

    /// Get the hex representation.
    pub fn hex(&self) -> &str {
        &self.0
    }

    /// Create from a pre-computed hex string.
    ///
    /// Validates that the string is exactly 64 hex characters.
    pub fn from_hex(hex: impl Into<String>) -> Result<Self, DigestError> {
        let hex = hex.into();
        if hex.len() != 64 {
            return Err(DigestError::InvalidDigest {
                reason: format!("expected 64 hex chars, got {}", hex.len()),
            });
        }
        if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(DigestError::InvalidDigest {
                reason: "digest must contain only hex characters".into(),
            });
        }
        Ok(Self(hex))
    }

    /// Create from a pre-computed hex string without validation.
    ///
    /// Use only when the digest is known to be valid (e.g. loaded from DB).
    pub fn from_hex_unchecked(hex: impl Into<String>) -> Self {
        Self(hex.into())
    }
}

impl std::fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Incremental digest builder for computing digests over multiple fields.
///
/// Use this when the digest domain spans multiple fields and you want to
/// hash them incrementally without allocating a single concatenated buffer.
pub struct DigestBuilder {
    hasher: blake3::Hasher,
}

impl DigestBuilder {
    /// Create a new builder.
    pub fn new() -> Self {
        Self {
            hasher: blake3::Hasher::new(),
        }
    }

    /// Feed raw bytes into the digest.
    pub fn update(&mut self, data: &[u8]) -> &mut Self {
        self.hasher.update(data);
        self
    }

    /// Feed a UTF-8 string into the digest.
    pub fn update_str(&mut self, data: &str) -> &mut Self {
        self.hasher.update(data.as_bytes());
        self
    }

    /// Feed a field separator. Use between fields to prevent ambiguity.
    pub fn separator(&mut self) -> &mut Self {
        self.hasher.update(b"\x00");
        self
    }

    /// Feed a JSON-serializable value using canonical serialization.
    pub fn update_json<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<&mut Self, DigestError> {
        let canonical =
            serde_json::to_string(value).map_err(|e| DigestError::SerializationFailed {
                reason: e.to_string(),
            })?;
        self.hasher.update(canonical.as_bytes());
        Ok(self)
    }

    /// Finalize and return the digest.
    pub fn finalize(self) -> ContentDigest {
        let hash = self.hasher.finalize();
        ContentDigest(hash.to_hex().to_string())
    }
}

impl Default for DigestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Errors from digest operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DigestError {
    /// Serialization to canonical JSON failed.
    SerializationFailed { reason: String },
    /// The provided digest string is invalid.
    InvalidDigest { reason: String },
}

impl std::fmt::Display for DigestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SerializationFailed { reason } => {
                write!(f, "digest serialization failed: {reason}")
            }
            Self::InvalidDigest { reason } => {
                write!(f, "invalid digest: {reason}")
            }
        }
    }
}

impl std::error::Error for DigestError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[test]
    fn compute_and_verify_length() {
        let digest = ContentDigest::compute(b"hello world");
        assert_eq!(digest.hex().len(), 64);
        assert!(digest.hex().chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn deterministic_same_input() {
        let a = ContentDigest::compute(b"test data");
        let b = ContentDigest::compute(b"test data");
        assert_eq!(a, b);
    }

    #[test]
    fn different_input_different_digest() {
        let a = ContentDigest::compute(b"input A");
        let b = ContentDigest::compute(b"input B");
        assert_ne!(a, b);
    }

    #[test]
    fn compute_json_deterministic() {
        let mut map = BTreeMap::new();
        map.insert("b", "two");
        map.insert("a", "one");
        let d1 = ContentDigest::compute_json(&map).unwrap();

        let mut map2 = BTreeMap::new();
        map2.insert("a", "one");
        map2.insert("b", "two");
        let d2 = ContentDigest::compute_json(&map2).unwrap();

        // BTreeMap ensures sorted keys → same digest regardless of insertion order
        assert_eq!(d1, d2);
    }

    #[test]
    fn from_hex_valid() {
        let digest = ContentDigest::compute(b"test");
        let restored = ContentDigest::from_hex(digest.hex()).unwrap();
        assert_eq!(restored, digest);
    }

    #[test]
    fn from_hex_wrong_length() {
        let err = ContentDigest::from_hex("abc").unwrap_err();
        assert!(matches!(err, DigestError::InvalidDigest { .. }));
    }

    #[test]
    fn from_hex_non_hex_chars() {
        let err = ContentDigest::from_hex("g".repeat(64)).unwrap_err();
        assert!(matches!(err, DigestError::InvalidDigest { .. }));
    }

    #[test]
    fn builder_deterministic() {
        let d1 = {
            let mut b = DigestBuilder::new();
            b.update_str("field1").separator().update_str("field2");
            b.finalize()
        };
        let d2 = {
            let mut b = DigestBuilder::new();
            b.update_str("field1").separator().update_str("field2");
            b.finalize()
        };
        assert_eq!(d1, d2);
    }

    #[test]
    fn builder_separator_prevents_collision() {
        // "ab" + "c" should differ from "a" + "bc"
        let d1 = {
            let mut b = DigestBuilder::new();
            b.update_str("ab").separator().update_str("c");
            b.finalize()
        };
        let d2 = {
            let mut b = DigestBuilder::new();
            b.update_str("a").separator().update_str("bc");
            b.finalize()
        };
        assert_ne!(d1, d2);
    }

    #[test]
    fn serde_roundtrip() {
        let digest = ContentDigest::compute(b"test");
        let json = serde_json::to_string(&digest).unwrap();
        let back: ContentDigest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, digest);
    }
}
