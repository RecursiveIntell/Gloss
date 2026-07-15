//! Import lifecycle receipts for rebuildable semantic-memory derived artifacts.
//!
//! The bridge does not own semantic-memory storage. These types let import callers
//! record whether an import requested or observed post-import derived artifact
//! lifecycle work, including the proveKV/poly-kv candidate pool. The artifact is
//! candidate-only; exact f32 rerank remains owned by semantic-memory.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Schema version for bridge-owned derived artifact lifecycle receipt fragments.
pub const BRIDGE_DERIVED_ARTIFACT_STATUS_V1_SCHEMA: &str = "bridge_derived_artifact_status_v1";

/// Artifact family used for semantic-memory proveKV/poly-kv pool generations.
pub const SEMANTIC_MEMORY_PROVEKV_POOL_ARTIFACT_FAMILY: &str = "semantic_memory_provekv_pool";

/// Status summary for a derived artifact observed/requested after import.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BridgeDerivedArtifactStatusV1 {
    pub schema_version: String,
    pub artifact_family: String,
    pub requested: bool,
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding_snapshot_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manifest_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Candidate artifacts must never become verification authority.
    pub candidate_only: bool,
    /// Semantic-memory authoritative f32 exact rerank is required before final use.
    pub exact_f32_rerank_required: bool,
}

impl BridgeDerivedArtifactStatusV1 {
    pub fn provekv_pool_requested() -> Self {
        Self {
            schema_version: BRIDGE_DERIVED_ARTIFACT_STATUS_V1_SCHEMA.into(),
            artifact_family: SEMANTIC_MEMORY_PROVEKV_POOL_ARTIFACT_FAMILY.into(),
            requested: true,
            status: "requested".into(),
            generation_id: None,
            embedding_snapshot_digest: None,
            manifest_digest: None,
            reason: Some("post_import_rebuild_requested".into()),
            candidate_only: true,
            exact_f32_rerank_required: true,
        }
    }

    pub fn provekv_pool_disabled(reason: impl Into<String>) -> Self {
        Self {
            schema_version: BRIDGE_DERIVED_ARTIFACT_STATUS_V1_SCHEMA.into(),
            artifact_family: SEMANTIC_MEMORY_PROVEKV_POOL_ARTIFACT_FAMILY.into(),
            requested: false,
            status: "disabled".into(),
            generation_id: None,
            embedding_snapshot_digest: None,
            manifest_digest: None,
            reason: Some(reason.into()),
            candidate_only: true,
            exact_f32_rerank_required: true,
        }
    }

    pub fn provekv_pool_ready(
        generation_id: impl Into<String>,
        embedding_snapshot_digest: impl Into<String>,
        manifest_digest: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: BRIDGE_DERIVED_ARTIFACT_STATUS_V1_SCHEMA.into(),
            artifact_family: SEMANTIC_MEMORY_PROVEKV_POOL_ARTIFACT_FAMILY.into(),
            requested: true,
            status: "ready".into(),
            generation_id: Some(generation_id.into()),
            embedding_snapshot_digest: Some(embedding_snapshot_digest.into()),
            manifest_digest: Some(manifest_digest.into()),
            reason: None,
            candidate_only: true,
            exact_f32_rerank_required: true,
        }
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.schema_version != BRIDGE_DERIVED_ARTIFACT_STATUS_V1_SCHEMA {
            return Err(format!(
                "unsupported schema_version '{}'",
                self.schema_version
            ));
        }
        if self.artifact_family.is_empty() {
            return Err("artifact_family must not be empty".into());
        }
        if self.status.is_empty() {
            return Err("status must not be empty".into());
        }
        if !self.candidate_only {
            return Err("derived artifacts are candidate-only at this bridge boundary".into());
        }
        if !self.exact_f32_rerank_required {
            return Err("proveKV/poly-kv candidate artifacts require exact f32 rerank".into());
        }
        if self.status == "ready" {
            if self.generation_id.as_deref().unwrap_or_default().is_empty() {
                return Err("ready artifact requires generation_id".into());
            }
            if self
                .embedding_snapshot_digest
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                return Err("ready artifact requires embedding_snapshot_digest".into());
            }
            if self
                .manifest_digest
                .as_deref()
                .unwrap_or_default()
                .is_empty()
            {
                return Err("ready artifact requires manifest_digest".into());
            }
        }
        Ok(())
    }
}

/// Bridge import options for post-import derived artifact lifecycle orchestration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BridgeImportOptions {
    #[serde(default)]
    pub rebuild_semantic_vector_artifacts: bool,
    #[serde(default)]
    pub rebuild_provekv_pool_artifacts: bool,
}

/// Deterministic receipt helper for bridge callers that request post-import work
/// but do not own the async semantic-memory rebuild executor.
pub fn requested_post_import_artifacts(
    options: &BridgeImportOptions,
) -> Vec<BridgeDerivedArtifactStatusV1> {
    let mut artifacts = Vec::new();
    if options.rebuild_provekv_pool_artifacts {
        artifacts.push(BridgeDerivedArtifactStatusV1::provekv_pool_requested());
    }
    artifacts
}
