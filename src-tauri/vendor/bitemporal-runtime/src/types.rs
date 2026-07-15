//! Core bitemporal types.

use chrono::{DateTime, Utc};
#[cfg(feature = "schema")]
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A globally unique record identifier.
pub type RecordId = String;
/// Canonical identity of one immutable record version/event.
pub type EventId = String;

/// A bitemporal record.
///
/// Type parameter `T` is the domain value being recorded.
/// The record carries two orthogonal timelines:
/// - `valid_time`: when the value is true in the domain (business time)
/// - `recorded_time`: when the system captured the value (system time)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
#[cfg_attr(
    feature = "schema",
    schemars(bound = "T: ::schemars::JsonSchema + Default")
)]
pub struct BitemporalRecord<T = ()> {
    /// Unique identifier for this record (stable across versions).
    /// Used to link superseding records.
    pub id: RecordId,

    /// Valid time — when this record's value is true in the domain.
    /// Backward-bounded (valid_time is set to the moment the fact became true).
    pub valid_time: DateTime<Utc>,

    /// Recorded time — when this record was inserted into the system.
    /// Forward-bounded (recorded_time is the moment of system insertion).
    pub recorded_time: DateTime<Utc>,

    /// The domain value this record captures. Defaults to `T::default()`
    /// when missing from the wire format — both with and without the
    /// `schema` feature. The schema feature adds `T: Default` to the
    /// schemars bound, which is also the bound `serde(default)` requires.
    #[serde(default)]
    pub value: T,
}

impl<T> BitemporalRecord<T> {
    /// Map the value of this record through a function, preserving temporal fields.
    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> BitemporalRecord<U> {
        BitemporalRecord {
            id: self.id,
            valid_time: self.valid_time,
            recorded_time: self.recorded_time,
            value: f(self.value),
        }
    }

    /// Returns the record's ID.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the valid time (when the value is true in the domain).
    pub fn valid_time(&self) -> DateTime<Utc> {
        self.valid_time
    }

    /// Returns the recorded time (when the system captured this).
    pub fn recorded_time(&self) -> DateTime<Utc> {
        self.recorded_time
    }
}

impl<T: Serialize> BitemporalRecord<T> {
    /// Derive the canonical, content-bound identity for this immutable version.
    pub fn try_event_id(&self) -> Result<EventId, crate::BitemporalError> {
        let encoded = Self::canonical_bytes(self)?;
        let mut digest = Sha256::new();
        digest.update(b"recursiveintell:bitemporal-event:v3\0");
        digest.update((encoded.len() as u64).to_be_bytes());
        digest.update(encoded);
        Ok(format!("bte_{:x}", digest.finalize()))
    }

    fn canonical_bytes(record: &Self) -> Result<Vec<u8>, crate::BitemporalError> {
        let value = serde_json::to_value(record)
            .map_err(|error| crate::BitemporalError::SerializationError(error.to_string()))?;
        boundary_compiler::Canonicalizer::new()
            .canonicalize_bytes(&value)
            .map_err(|error| crate::BitemporalError::SerializationError(error.to_string()))
    }
}

pub(crate) type CanonicalRecordOrderKey = (DateTime<Utc>, DateTime<Utc>, EventId);

pub(crate) fn canonical_record_order_key<T: Serialize>(
    record: &BitemporalRecord<T>,
) -> Result<CanonicalRecordOrderKey, crate::BitemporalError> {
    Ok((
        record.recorded_time,
        record.valid_time,
        record.try_event_id()?,
    ))
}

/// Reference to the record that was superseded by a supersession event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct SupersessionTarget {
    /// Exact immutable event/version identity that was superseded.
    /// Absent only when decoding the legacy V1 wire shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseded_event_id: Option<EventId>,
    /// ID of the record that was superseded.
    pub superseded_id: RecordId,
    /// Recorded time of the superseded record.
    pub superseded_recorded_time: DateTime<Utc>,
}

/// Receipt for a supersession event.
///
/// Cryptographically identifies both the superseded record and the superseding record,
/// providing an auditable chain of custody.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schema", derive(JsonSchema))]
pub struct SupersessionReceipt {
    /// Versioned wire contract for additive event identity fields.
    #[serde(default = "legacy_supersession_receipt_schema")]
    pub schema_version: String,
    /// Exact immutable event/version identity that superseded the old event.
    /// Absent only when decoding the legacy V1 wire shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub superseding_event_id: Option<EventId>,
    /// ID of the record that superseded another.
    pub superseding_id: RecordId,
    /// Recorded time of the superseding record.
    pub superseding_recorded_time: DateTime<Utc>,
    /// The target that was superseded.
    pub superseded: SupersessionTarget,
    /// SHA-256 digest of the superseding record content (for integrity verification).
    pub superseding_digest: String,
    /// SHA-256 digest of the superseded record content (for integrity verification).
    pub superseded_digest: String,
    /// SHA-256 digest of the receipt itself (self-checksum).
    pub receipt_digest: String,
}

impl SupersessionReceipt {
    /// Create a new supersession receipt from superseded and superseding records.
    ///
    /// The receipt is the cryptographic audit handle for a supersession
    /// event. The digests bind **all** content of both records (id,
    /// temporal fields, AND the JSON-serialized value) so that two
    /// records differing only in their value produce different digests.
    /// The receipt_digest additionally binds the two record digests so
    /// the supersession relationship is itself tamper-evident.
    pub fn new<T>(
        superseded: BitemporalRecord<T>,
        superseding: BitemporalRecord<T>,
    ) -> Result<Self, crate::BitemporalError>
    where
        T: Serialize,
    {
        let superseded_digest = Self::digest_record(&superseded)?;
        let superseding_digest = Self::digest_record(&superseding)?;
        let superseded_event_id = superseded.try_event_id()?;
        let superseding_event_id = superseding.try_event_id()?;

        let receipt_content = format!(
            "supersession:v2:{}:{}:{}:{}:{}:{}",
            superseding_event_id,
            superseding
                .recorded_time
                .timestamp_nanos_opt()
                .ok_or_else(|| crate::BitemporalError::SerializationError(
                    "superseding timestamp outside nanosecond range".into()
                ))?,
            superseded_event_id,
            superseded
                .recorded_time
                .timestamp_nanos_opt()
                .ok_or_else(|| crate::BitemporalError::SerializationError(
                    "superseded timestamp outside nanosecond range".into()
                ))?,
            superseding_digest,
            superseded_digest
        );
        let receipt_digest = format!("{:x}", Sha256::digest(receipt_content.as_bytes()));

        Ok(Self {
            schema_version: "supersession_receipt_v2".into(),
            superseding_event_id: Some(superseding_event_id),
            superseding_id: superseding.id,
            superseding_recorded_time: superseding.recorded_time,
            superseded: SupersessionTarget {
                superseded_event_id: Some(superseded_event_id),
                superseded_id: superseded.id,
                superseded_recorded_time: superseded.recorded_time,
            },
            superseding_digest,
            superseded_digest,
            receipt_digest,
        })
    }

    /// Compatibility alias for the fallible constructor.
    pub fn try_new<T>(
        superseded: BitemporalRecord<T>,
        superseding: BitemporalRecord<T>,
    ) -> Result<Self, crate::BitemporalError>
    where
        T: Serialize,
    {
        Self::new(superseded, superseding)
    }

    /// Compute the SHA-256 digest of a record's full content (id,
    /// temporal fields, and JSON-serialized value). Two records with
    /// different values produce different digests.
    fn digest_record<T: Serialize>(
        record: &BitemporalRecord<T>,
    ) -> Result<String, crate::BitemporalError> {
        let value = serde_json::to_value(record)
            .map_err(|e| crate::BitemporalError::SerializationError(e.to_string()))?;
        let encoded = boundary_compiler::Canonicalizer::new()
            .canonicalize_bytes(&value)
            .map_err(|e| crate::BitemporalError::SerializationError(e.to_string()))?;
        let mut digest = Sha256::new();
        digest.update(b"recursiveintell:bitemporal-record:v2\0");
        digest.update((encoded.len() as u64).to_be_bytes());
        digest.update(encoded);
        Ok(format!("{:x}", digest.finalize()))
    }
}

fn legacy_supersession_receipt_schema() -> String {
    "supersession_receipt_v1".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_record_temporal_fields() {
        let now = Utc::now();
        let record = BitemporalRecord::<String> {
            id: "test1".to_string(),
            valid_time: now,
            recorded_time: now,
            value: "hello".to_string(),
        };
        assert_eq!(record.id(), "test1");
        assert_eq!(record.valid_time(), now);
        assert_eq!(record.recorded_time(), now);
    }

    #[test]
    fn test_supersession_receipt_digest() {
        let now = Utc::now();
        let superseded = BitemporalRecord {
            id: "v1".to_string(),
            valid_time: now,
            recorded_time: now,
            value: (),
        };
        let superseding = BitemporalRecord {
            id: "v2".to_string(),
            valid_time: now,
            recorded_time: now,
            value: (),
        };
        let receipt = SupersessionReceipt::new(superseded, superseding).unwrap();
        assert!(!receipt.receipt_digest.is_empty());
        assert_eq!(receipt.superseded.superseded_id, "v1");
        assert_eq!(receipt.superseding_id, "v2");
    }

    #[test]
    fn legacy_v1_receipt_wire_decodes_without_fabricating_event_identity() {
        let now = Utc::now();
        let wire = serde_json::json!({
            "superseding_id": "logical",
            "superseding_recorded_time": now,
            "superseded": {
                "superseded_id": "logical",
                "superseded_recorded_time": now
            },
            "superseding_digest": "new-digest",
            "superseded_digest": "old-digest",
            "receipt_digest": "receipt-digest"
        });
        let receipt: SupersessionReceipt = serde_json::from_value(wire).unwrap();
        assert_eq!(receipt.schema_version, "supersession_receipt_v1");
        assert!(receipt.superseding_event_id.is_none());
        assert!(receipt.superseded.superseded_event_id.is_none());
    }
}
