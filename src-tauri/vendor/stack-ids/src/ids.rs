//! Opaque ID newtypes for cross-crate identity.
//!
//! Each newtype wraps a `String` and is intentionally opaque — callers
//! should not parse the inner value. IDs are assigned by their respective
//! authority crates; `stack-ids` only provides the type contract.

use serde::{Deserialize, Serialize};

/// Macro to generate an opaque string-wrapper ID type with standard impls.
macro_rules! define_id {
    (
        $(#[$meta:meta])*
        $name:ident
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl $name {
            /// Create from any string-like value.
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            /// Generate a new random UUID v4 ID.
            pub fn generate() -> Self {
                Self(uuid::Uuid::new_v4().to_string())
            }

            /// Borrow as a string slice.
            pub fn as_str(&self) -> &str {
                &self.0
            }

            /// Returns true if the inner string is empty.
            pub fn is_empty(&self) -> bool {
                self.0.is_empty()
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(&self.0)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self(value)
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self(value.to_string())
            }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

define_id!(
    /// Opaque identifier for an import/export envelope.
    ///
    /// Assigned by the exporting authority (e.g. Forge). Stable across
    /// re-exports of the same logical unit.
    EnvelopeId
);

define_id!(
    /// Opaque identifier for a claim (knowledge assertion).
    ///
    /// Assigned by the projection importer. Stable within a claim lineage.
    ClaimId
);

define_id!(
    /// Opaque identifier for a specific version of a claim.
    ///
    /// Each mutation to a claim's validity, status, or content produces
    /// a new version with a new `ClaimVersionId`. The `ClaimId` remains
    /// stable across versions.
    ClaimVersionId
);

define_id!(
    /// Opaque identifier for an entity (person, concept, code unit, etc.).
    ///
    /// The inner string is intentionally unstructured. Use domain-specific
    /// constructors to create these (e.g. code_entity_id).
    EntityId
);

define_id!(
    /// Opaque identifier for an episode (causal record).
    ///
    /// Assigned by the episode creator. Stable within the episode's lifecycle.
    EpisodeId
);

define_id!(
    /// Opaque identifier for a logical retry family within one retry-owner boundary.
    ///
    /// One `AttemptId` exists per logical retry family. Retries inside that
    /// boundary produce new `TrialId`s, NOT new `AttemptId`s. A new `AttemptId`
    /// is created only when the retry owner changes (e.g. node-level retry
    /// after transport retries are exhausted) or on explicit replay/re-enqueue.
    AttemptId
);

define_id!(
    /// Opaque identifier for a concrete execution within a logical retry family.
    ///
    /// Every retry within one owner boundary creates a new `TrialId` under
    /// the same `AttemptId`. Each `TrialId` is globally unique (UUID v4).
    TrialId
);

define_id!(
    /// Opaque identifier for a stored artifact (patch, snapshot, file).
    ///
    /// Assigned by the artifact store. Stable across reads.
    ArtifactId
);

define_id!(
    /// Opaque identifier for a projection instance.
    ///
    /// Identifies a specific derived view (entity registry entry, temporal
    /// claim set, etc.) within a scope.
    ProjectionId
);

define_id!(
    /// Opaque identifier for a relation (edge between entities).
    ///
    /// Assigned by the projection importer. Stable within a relation lineage.
    /// Each mutation produces a new `RelationVersionId` under the same `RelationId`.
    RelationId
);

define_id!(
    /// Opaque identifier for a relation version.
    ///
    /// Each mutation to a relation produces a new version with a new
    /// `RelationVersionId`. The `RelationId` remains stable across versions.
    RelationVersionId
);

define_id!(
    /// Opaque identifier for an import batch produced by the bridge.
    ///
    /// Assigned by the bridge transformation pipeline. Unique per batch.
    ImportBatchId
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_creation_and_display() {
        let id = EnvelopeId::new("env-001");
        assert_eq!(id.as_str(), "env-001");
        assert_eq!(id.to_string(), "env-001");
        assert!(!id.is_empty());
    }

    #[test]
    fn id_from_string() {
        let id: ClaimId = "claim-123".into();
        assert_eq!(id.as_str(), "claim-123");
    }

    #[test]
    fn id_generate_is_unique() {
        let a = AttemptId::generate();
        let b = AttemptId::generate();
        assert_ne!(a, b);
    }

    #[test]
    fn id_empty_check() {
        let id = EntityId::new("");
        assert!(id.is_empty());

        let id = EntityId::new("e-1");
        assert!(!id.is_empty());
    }

    #[test]
    fn id_serde_roundtrip() {
        let id = TrialId::new("trial-42");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"trial-42\"");
        let back: TrialId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn id_ordering() {
        let a = EnvelopeId::new("aaa");
        let b = EnvelopeId::new("bbb");
        assert!(a < b);
    }
}
