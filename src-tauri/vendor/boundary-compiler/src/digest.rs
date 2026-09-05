//! Domain-separated BLAKE3 digests over RFC 8785 bytes.

use crate::{Canonicalizer, JcsError};
use blake3::Hash;
use serde::{Deserialize, Serialize};

/// Default domain separator for canonical JSON content identity.
pub const CANONICAL_JSON_DOMAIN: &str = "recursiveintell:canonical-json:v1";
/// Digest algorithm identifier.
pub const DIGEST_ALGORITHM: &str = "blake3-256";
/// Canonicalization profile identifier.
pub const CANONICALIZATION_PROFILE: &str = "rfc8785";
/// Schema identifier used when no application schema is supplied.
pub const DEFAULT_SCHEMA_ID: &str = "application/json";
/// Version of the default JSON schema identity.
pub const DEFAULT_SCHEMA_VERSION: &str = "rfc8785-ijson";

/// Identity context cryptographically bound into a content digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DigestMetadata {
    /// Hash algorithm.
    pub algorithm: String,
    /// Canonicalization algorithm/profile.
    pub canonicalization_profile: String,
    /// Schema identifier for the hashed value.
    pub schema_id: String,
    /// Schema version for the hashed value.
    pub schema_version: String,
    /// Domain separator preventing cross-protocol reuse.
    pub domain_separator: String,
}

impl DigestMetadata {
    /// Creates RFC 8785 JSON digest metadata for an application schema.
    pub fn canonical_json(schema_id: impl Into<String>, schema_version: impl Into<String>) -> Self {
        Self {
            algorithm: DIGEST_ALGORITHM.to_owned(),
            canonicalization_profile: CANONICALIZATION_PROFILE.to_owned(),
            schema_id: schema_id.into(),
            schema_version: schema_version.into(),
            domain_separator: CANONICAL_JSON_DOMAIN.to_owned(),
        }
    }
}

impl Default for DigestMetadata {
    fn default() -> Self {
        Self::canonical_json(DEFAULT_SCHEMA_ID, DEFAULT_SCHEMA_VERSION)
    }
}

/// A domain-separated BLAKE3 digest of RFC 8785 canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContentDigest {
    hash: Hash,
    metadata: DigestMetadata,
}

impl ContentDigest {
    /// Computes a digest with the default JSON schema identity.
    pub fn compute(value: &serde_json::Value) -> Result<Self, JcsError> {
        Self::compute_with_schema(value, DEFAULT_SCHEMA_ID, DEFAULT_SCHEMA_VERSION)
    }

    /// Computes a digest bound to an explicit schema ID and version.
    pub fn compute_with_schema(
        value: &serde_json::Value,
        schema_id: impl Into<String>,
        schema_version: impl Into<String>,
    ) -> Result<Self, JcsError> {
        let canonical_bytes = Canonicalizer::new().canonicalize_bytes(value)?;
        let metadata = DigestMetadata::canonical_json(schema_id, schema_version);
        let hash = hash_with_metadata(&metadata, &canonical_bytes);
        Ok(Self { hash, metadata })
    }

    /// Returns the hex-encoded digest.
    pub fn hex(&self) -> String {
        self.hash.to_hex().to_string()
    }

    /// Returns the raw digest bytes.
    pub fn as_bytes(&self) -> [u8; 32] {
        *self.hash.as_bytes()
    }

    /// Returns the identity context bound into the digest.
    pub fn metadata(&self) -> &DigestMetadata {
        &self.metadata
    }
}

fn hash_with_metadata(metadata: &DigestMetadata, bytes: &[u8]) -> Hash {
    let mut hasher = blake3::Hasher::new();
    for field in [
        metadata.algorithm.as_bytes(),
        metadata.canonicalization_profile.as_bytes(),
        metadata.schema_id.as_bytes(),
        metadata.schema_version.as_bytes(),
        metadata.domain_separator.as_bytes(),
        bytes,
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
    hasher.finalize()
}

impl std::fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hex())
    }
}

impl std::fmt::LowerHex for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.hex())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn digest_binds_value_and_schema_identity() {
        let value = json!({"b": 1, "a": 2});
        let first = ContentDigest::compute_with_schema(&value, "claim", "1").unwrap();
        let same = ContentDigest::compute_with_schema(&value, "claim", "1").unwrap();
        let other_value =
            ContentDigest::compute_with_schema(&json!({"a": 3}), "claim", "1").unwrap();
        let other_schema = ContentDigest::compute_with_schema(&value, "claim", "2").unwrap();
        assert_eq!(first, same);
        assert_ne!(first, other_value);
        assert_ne!(first, other_schema);
        assert_eq!(first.metadata().algorithm, DIGEST_ALGORITHM);
        assert_eq!(
            first.metadata().canonicalization_profile,
            CANONICALIZATION_PROFILE
        );
        assert_eq!(first.metadata().domain_separator, CANONICAL_JSON_DOMAIN);
    }
}
