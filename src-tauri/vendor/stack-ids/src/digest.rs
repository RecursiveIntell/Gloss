//! Shared domain-separated content digest computation.
//!
//! Canonical JSON delegates to boundary-compiler so the stack has one RFC
//! 8785 implementation and one JSON digest law.

use schemars::{
    r#gen::SchemaGenerator,
    schema::{
        InstanceType, Metadata, ObjectValidation, Schema, SchemaObject, StringValidation,
        SubschemaValidation,
    },
    JsonSchema, Map, Set,
};
use serde::{de, Deserialize, Deserializer, Serialize};

/// Domain separator for non-JSON raw byte digests.
pub const RAW_BYTES_DOMAIN: &str = "recursiveintell:raw-bytes:v1";

/// Identity context cryptographically bound into a content digest.
#[derive(
    Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
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
    fn raw_bytes() -> Self {
        Self {
            algorithm: boundary_compiler::DIGEST_ALGORITHM.to_owned(),
            canonicalization_profile: "raw-bytes".to_owned(),
            schema_id: "application/octet-stream".to_owned(),
            schema_version: "1".to_owned(),
            domain_separator: RAW_BYTES_DOMAIN.to_owned(),
        }
    }

    fn from_boundary(metadata: &boundary_compiler::digest::DigestMetadata) -> Self {
        Self {
            algorithm: metadata.algorithm.clone(),
            canonicalization_profile: metadata.canonicalization_profile.clone(),
            schema_id: metadata.schema_id.clone(),
            schema_version: metadata.schema_version.clone(),
            domain_separator: metadata.domain_separator.clone(),
        }
    }
}

/// A BLAKE3 content digest with explicit identity context.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub struct ContentDigest {
    hex: String,
    metadata: DigestMetadata,
}

impl<'de> Deserialize<'de> for ContentDigest {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ContentDigestWire {
            Structured {
                hex: String,
                metadata: DigestMetadata,
            },
            LegacyHex(String),
        }

        match ContentDigestWire::deserialize(deserializer)? {
            ContentDigestWire::Structured { hex, metadata } => {
                let hex = validate_hex(hex).map_err(de::Error::custom)?;
                Ok(Self { hex, metadata })
            }
            ContentDigestWire::LegacyHex(hex) => Self::from_hex(hex).map_err(de::Error::custom),
        }
    }
}

impl JsonSchema for ContentDigest {
    fn schema_name() -> String {
        "ContentDigest".to_owned()
    }

    fn json_schema(generator: &mut SchemaGenerator) -> Schema {
        let structured = SchemaObject {
            instance_type: Some(InstanceType::Object.into()),
            object: Some(Box::new(ObjectValidation {
                properties: Map::from([
                    ("hex".to_owned(), generator.subschema_for::<String>()),
                    (
                        "metadata".to_owned(),
                        generator.subschema_for::<DigestMetadata>(),
                    ),
                ]),
                required: Set::from(["hex".to_owned(), "metadata".to_owned()]),
                ..Default::default()
            })),
            ..Default::default()
        };
        let legacy = SchemaObject {
            instance_type: Some(InstanceType::String.into()),
            string: Some(Box::new(StringValidation {
                min_length: Some(64),
                max_length: Some(64),
                pattern: Some("^[0-9A-Fa-f]{64}$".to_owned()),
            })),
            ..Default::default()
        };

        Schema::Object(SchemaObject {
            metadata: Some(Box::new(Metadata {
                description: Some(
                    "A BLAKE3 content digest with explicit identity context.".to_owned(),
                ),
                ..Default::default()
            })),
            subschemas: Some(Box::new(SubschemaValidation {
                any_of: Some(vec![Schema::Object(structured), Schema::Object(legacy)]),
                ..Default::default()
            })),
            ..Default::default()
        })
    }
}

impl ContentDigest {
    /// Computes a domain-separated digest from raw bytes.
    pub fn compute(data: &[u8]) -> Self {
        let metadata = DigestMetadata::raw_bytes();
        Self {
            hex: hash_with_metadata(&metadata, data),
            metadata,
        }
    }

    /// Computes a domain-separated digest from a UTF-8 string.
    pub fn compute_str(data: &str) -> Self {
        Self::compute(data.as_bytes())
    }

    /// Computes an RFC 8785 digest using the default JSON schema identity.
    pub fn compute_json<T: Serialize + ?Sized>(value: &T) -> Result<Self, DigestError> {
        Self::compute_json_with_schema(
            value,
            boundary_compiler::DEFAULT_SCHEMA_ID,
            boundary_compiler::DEFAULT_SCHEMA_VERSION,
        )
    }

    /// Computes an RFC 8785 digest bound to a schema ID and version.
    pub fn compute_json_with_schema<T: Serialize + ?Sized>(
        value: &T,
        schema_id: impl Into<String>,
        schema_version: impl Into<String>,
    ) -> Result<Self, DigestError> {
        let value = serde_json::to_value(value).map_err(serialization_failed)?;
        let digest = boundary_compiler::ContentDigest::compute_with_schema(
            &value,
            schema_id,
            schema_version,
        )
        .map_err(|error| DigestError::SerializationFailed {
            reason: error.to_string(),
        })?;
        Ok(Self {
            hex: digest.hex(),
            metadata: DigestMetadata::from_boundary(digest.metadata()),
        })
    }

    /// Returns the hex representation.
    pub fn hex(&self) -> &str {
        &self.hex
    }

    /// Returns the raw 32-byte digest.
    pub fn as_bytes(&self) -> [u8; 32] {
        let mut bytes = [0_u8; 32];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.hex[index * 2..index * 2 + 2], 16)
                .expect("ContentDigest maintains validated hexadecimal");
        }
        bytes
    }

    /// Returns the identity context bound into the digest.
    pub fn metadata(&self) -> &DigestMetadata {
        &self.metadata
    }

    /// Creates a raw-byte digest wrapper from validated hex.
    pub fn from_hex(hex: impl Into<String>) -> Result<Self, DigestError> {
        let hex = validate_hex(hex.into())?;
        Ok(Self {
            hex,
            metadata: DigestMetadata::raw_bytes(),
        })
    }

    /// Creates a raw-byte digest wrapper without validating its hex.
    pub fn from_hex_unchecked(hex: impl Into<String>) -> Self {
        Self {
            hex: hex.into(),
            metadata: DigestMetadata::raw_bytes(),
        }
    }
}

impl std::fmt::Display for ContentDigest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.hex)
    }
}

/// Incremental raw-byte digest builder.
pub struct DigestBuilder {
    hasher: blake3::Hasher,
    metadata: DigestMetadata,
}

impl DigestBuilder {
    /// Creates a domain-separated raw-byte digest builder.
    pub fn new() -> Self {
        let metadata = DigestMetadata::raw_bytes();
        let mut hasher = blake3::Hasher::new();
        seed_metadata(&mut hasher, &metadata);
        Self { hasher, metadata }
    }

    /// Feeds raw bytes into the digest.
    pub fn update(&mut self, data: &[u8]) -> &mut Self {
        self.hasher.update(data);
        self
    }

    /// Feeds a UTF-8 string into the digest.
    pub fn update_str(&mut self, data: &str) -> &mut Self {
        self.update(data.as_bytes())
    }

    /// Feeds a field separator.
    pub fn separator(&mut self) -> &mut Self {
        self.update(b"\0")
    }

    /// Feeds RFC 8785 bytes for a serializable JSON value.
    pub fn update_json<T: Serialize + ?Sized>(
        &mut self,
        value: &T,
    ) -> Result<&mut Self, DigestError> {
        let value = serde_json::to_value(value).map_err(serialization_failed)?;
        let bytes = boundary_compiler::Canonicalizer::new()
            .canonicalize_bytes(&value)
            .map_err(|error| DigestError::SerializationFailed {
                reason: error.to_string(),
            })?;
        self.update(&bytes);
        Ok(self)
    }

    /// Finalizes the digest.
    pub fn finalize(self) -> ContentDigest {
        ContentDigest {
            hex: self.hasher.finalize().to_hex().to_string(),
            metadata: self.metadata,
        }
    }
}

impl Default for DigestBuilder {
    fn default() -> Self {
        Self::new()
    }
}

fn seed_metadata(hasher: &mut blake3::Hasher, metadata: &DigestMetadata) {
    for field in [
        metadata.algorithm.as_bytes(),
        metadata.canonicalization_profile.as_bytes(),
        metadata.schema_id.as_bytes(),
        metadata.schema_version.as_bytes(),
        metadata.domain_separator.as_bytes(),
    ] {
        hasher.update(&(field.len() as u64).to_be_bytes());
        hasher.update(field);
    }
}

fn hash_with_metadata(metadata: &DigestMetadata, data: &[u8]) -> String {
    let mut hasher = blake3::Hasher::new();
    seed_metadata(&mut hasher, metadata);
    hasher.update(&(data.len() as u64).to_be_bytes());
    hasher.update(data);
    hasher.finalize().to_hex().to_string()
}

fn validate_hex(hex: String) -> Result<String, DigestError> {
    if hex.len() != 64 {
        return Err(DigestError::InvalidDigest {
            reason: format!("expected 64 hex chars, got {}", hex.len()),
        });
    }
    if !hex.chars().all(|character| character.is_ascii_hexdigit()) {
        return Err(DigestError::InvalidDigest {
            reason: "digest must contain only hex characters".to_owned(),
        });
    }
    Ok(hex)
}

fn serialization_failed(error: serde_json::Error) -> DigestError {
    DigestError::SerializationFailed {
        reason: error.to_string(),
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

impl DigestError {
    /// Stable error kind.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::SerializationFailed { .. } => "serialization_failed",
            Self::InvalidDigest { .. } => "invalid_digest",
        }
    }
}

impl std::fmt::Display for DigestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SerializationFailed { reason } => {
                write!(f, "digest serialization failed: {reason}")
            }
            Self::InvalidDigest { reason } => write!(f, "invalid digest: {reason}"),
        }
    }
}

impl std::error::Error for DigestError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn raw_digest_is_deterministic_and_domain_bound() {
        let first = ContentDigest::compute(b"test");
        assert_eq!(first, ContentDigest::compute(b"test"));
        assert_ne!(first, ContentDigest::compute(b"other"));
        assert_eq!(first.hex().len(), 64);
        assert_eq!(first.metadata().domain_separator, RAW_BYTES_DOMAIN);
    }

    #[test]
    fn json_digest_normalizes_maps() {
        let left = HashMap::from([("b", 2), ("a", 1)]);
        let right = HashMap::from([("a", 1), ("b", 2)]);
        assert_eq!(
            ContentDigest::compute_json(&left).unwrap(),
            ContentDigest::compute_json(&right).unwrap()
        );
    }

    #[test]
    fn schema_identity_changes_json_digest() {
        let value = serde_json::json!({"a": 1});
        let first = ContentDigest::compute_json_with_schema(&value, "claim", "1").unwrap();
        let second = ContentDigest::compute_json_with_schema(&value, "claim", "2").unwrap();
        assert_ne!(first, second);
    }

    #[test]
    fn from_hex_validates_and_serde_roundtrips_metadata() {
        assert!(ContentDigest::from_hex("abc").is_err());
        assert!(ContentDigest::from_hex("g".repeat(64)).is_err());
        let digest = ContentDigest::compute(b"test");
        let encoded = serde_json::to_string(&digest).unwrap();
        let decoded: ContentDigest = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, digest);
    }

    #[test]
    fn deserializes_legacy_hex_string_as_raw_bytes() {
        let hex = "a".repeat(64);
        let encoded = serde_json::to_string(&hex).unwrap();

        let digest: ContentDigest = serde_json::from_str(&encoded).unwrap();

        assert_eq!(digest.hex(), hex);
        assert_eq!(digest.metadata(), &DigestMetadata::raw_bytes());
    }

    #[test]
    fn rejects_invalid_legacy_hex_strings() {
        for hex in ["a".repeat(63), "g".repeat(64)] {
            let encoded = serde_json::to_string(&hex).unwrap();
            assert!(serde_json::from_str::<ContentDigest>(&encoded).is_err());
        }
    }

    #[test]
    fn builder_separator_prevents_collision() {
        let first = {
            let mut builder = DigestBuilder::new();
            builder.update_str("ab").separator().update_str("c");
            builder.finalize()
        };
        let second = {
            let mut builder = DigestBuilder::new();
            builder.update_str("a").separator().update_str("bc");
            builder.finalize()
        };
        assert_ne!(first, second);
    }
}
