//! Error types for the bridge crate.

use semantic_memory_forge::ExportEnvelopeError;
use thiserror::Error;

/// Errors produced by the forge-memory-bridge.
#[derive(Debug, Error)]
pub enum BridgeError {
    /// The export envelope is structurally invalid.
    #[error("invalid envelope: {reason}")]
    InvalidEnvelope { reason: String },

    /// Schema version mismatch.
    #[error("incompatible version: expected {expected}, got {actual}")]
    IncompatibleVersion { expected: String, actual: String },

    /// Content digest does not match computed value.
    #[error("digest mismatch: expected {expected}, got {actual}")]
    DigestMismatch { expected: String, actual: String },

    /// Failed to compute content digest.
    #[error("digest computation failed: {reason}")]
    DigestComputationFailed { reason: String },

    /// A record in the envelope is malformed.
    #[error("invalid record: {reason}")]
    InvalidRecord { reason: String },

    /// Transformation from export to import failed.
    #[error("transform failed: {reason}")]
    TransformFailed { reason: String },

    /// A legacy import record is missing an episode identity that the bridge
    /// cannot synthesize without violating the episode-first identity law.
    #[error("missing episode identity in legacy import: {record_context}")]
    MissingEpisodeIdentity { record_context: String },

    /// A canonical V3 claim omitted its stable claim identity.
    #[error("canonical V3 claim is missing claim_id at record {record_index}")]
    MissingCanonicalClaimId { record_index: usize },

    /// A canonical V3 claim omitted its version identity.
    #[error("canonical V3 claim is missing claim_version_id at record {record_index}")]
    MissingCanonicalClaimVersionId { record_index: usize },

    /// A canonical V3 relation omitted its version identity.
    #[error("canonical V3 relation is missing relation_version_id at record {record_index}")]
    MissingCanonicalRelationVersionId { record_index: usize },
}

impl BridgeError {
    /// Stable error kind discriminant.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidEnvelope { .. } => "invalid_envelope",
            Self::IncompatibleVersion { .. } => "incompatible_version",
            Self::DigestMismatch { .. } => "digest_mismatch",
            Self::DigestComputationFailed { .. } => "digest_computation_failed",
            Self::InvalidRecord { .. } => "invalid_record",
            Self::TransformFailed { .. } => "transform_failed",
            Self::MissingEpisodeIdentity { .. } => "missing_episode_identity",
            Self::MissingCanonicalClaimId { .. } => "missing_canonical_claim_id",
            Self::MissingCanonicalClaimVersionId { .. } => "missing_canonical_claim_version_id",
            Self::MissingCanonicalRelationVersionId { .. } => {
                "missing_canonical_relation_version_id"
            }
        }
    }
}

/// First-class bridge import failure artifact for replayability and audit.
///
/// LIB-C005: Bridge failures must become replayable artifacts, not just logged errors.
/// This artifact captures the envelope identity, error classification, and provenance
/// required to diagnose and replay failed imports.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, schemars::JsonSchema)]
pub struct BridgeImportFailureArtifact {
    /// Stable schema version for this artifact family.
    pub schema_version: String,
    /// Source envelope ID that failed to import.
    pub source_envelope_id: String,
    /// Source authority (e.g. "forge").
    pub source_authority: String,
    /// Scope namespace of the attempted import.
    pub scope_namespace: String,
    /// Machine-readable error kind (matches `BridgeError::kind()`).
    pub error_kind: String,
    /// Human-readable error description.
    pub error_message: String,
    /// When the failure occurred.
    pub failed_at: String,
}

/// Schema version constant for `BridgeImportFailureArtifact`.
pub const BRIDGE_IMPORT_FAILURE_ARTIFACT_V1_SCHEMA: &str = "bridge_import_failure_artifact_v1";

impl BridgeImportFailureArtifact {
    /// Constructs a failure artifact from a `BridgeError` and provenance context.
    pub fn from_error(
        error: &BridgeError,
        source_envelope_id: &str,
        source_authority: &str,
        scope_namespace: &str,
    ) -> Self {
        Self {
            schema_version: BRIDGE_IMPORT_FAILURE_ARTIFACT_V1_SCHEMA.into(),
            source_envelope_id: source_envelope_id.into(),
            source_authority: source_authority.into(),
            scope_namespace: scope_namespace.into(),
            error_kind: error.kind().into(),
            error_message: error.to_string(),
            failed_at: chrono::Utc::now().to_rfc3339(),
        }
    }
}

impl From<ExportEnvelopeError> for BridgeError {
    fn from(value: ExportEnvelopeError) -> Self {
        match value {
            ExportEnvelopeError::InvalidEnvelope { reason } => Self::InvalidEnvelope { reason },
            ExportEnvelopeError::IncompatibleVersion { expected, actual } => {
                Self::IncompatibleVersion { expected, actual }
            }
            ExportEnvelopeError::DigestMismatch { expected, actual } => {
                Self::DigestMismatch { expected, actual }
            }
            ExportEnvelopeError::DigestComputationFailed { reason } => {
                Self::DigestComputationFailed { reason }
            }
        }
    }
}
