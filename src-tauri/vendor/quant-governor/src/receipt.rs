//! Exact fallback and degradation receipt types.

use serde::{Deserialize, Serialize};

/// Receipt for exact fallback decisions (compressed -> raw).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExactFallbackReceipt {
    /// Digest of the raw (uncompressed) representation
    pub raw_digest: Digest,

    /// Digest of the compressed representation
    pub compressed_digest: Digest,

    /// Whether compressed representation should be retained
    pub fallback_retention: bool,

    /// Reason for fallback
    pub fallback_reason: Option<String>,
}

impl ExactFallbackReceipt {
    /// Create a new exact fallback receipt.
    pub fn new(raw_digest: Digest, compressed_digest: Digest, fallback_retention: bool) -> Self {
        Self {
            raw_digest,
            compressed_digest,
            fallback_retention,
            fallback_reason: None,
        }
    }

    /// Create with a fallback reason.
    pub fn with_reason(
        raw_digest: Digest,
        compressed_digest: Digest,
        fallback_retention: bool,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            raw_digest,
            compressed_digest,
            fallback_retention,
            fallback_reason: Some(reason.into()),
        }
    }

    /// Returns bytes saved by using raw instead of compressed.
    pub fn bytes_saved(&self) -> Option<u64> {
        // This would be computed from size metadata in a full implementation
        None
    }
}

/// Digest types for content identification.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Digest {
    /// Algorithm used for digest
    pub algorithm: DigestAlgorithm,

    /// Hex-encoded digest value
    pub value: String,
}

/// Digest algorithm used for content identification.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DigestAlgorithm {
    /// SHA-256 digest
    Sha256,
    /// BLAKE3 digest
    Blake3,
}

impl Digest {
    /// Create a new SHA-256 digest.
    pub fn sha256(value: impl Into<String>) -> Self {
        Self {
            algorithm: DigestAlgorithm::Sha256,
            value: value.into(),
        }
    }

    /// Create a new BLAKE3 digest.
    pub fn blake3(value: impl Into<String>) -> Self {
        Self {
            algorithm: DigestAlgorithm::Blake3,
            value: value.into(),
        }
    }
}

impl std::fmt::Display for DigestAlgorithm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DigestAlgorithm::Sha256 => write!(f, "sha256"),
            DigestAlgorithm::Blake3 => write!(f, "blake3"),
        }
    }
}

impl std::fmt::Display for Digest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}", self.algorithm, self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn digest_creation() {
        let d = Digest::sha256("abc123");
        assert_eq!(d.algorithm, DigestAlgorithm::Sha256);
        assert_eq!(d.value, "abc123");
    }

    #[test]
    fn fallback_receipt_display() {
        let receipt =
            ExactFallbackReceipt::new(Digest::sha256("raw"), Digest::blake3("compressed"), true);
        assert!(receipt.fallback_retention);
    }
}
