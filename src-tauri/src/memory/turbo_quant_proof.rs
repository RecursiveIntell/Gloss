//! Gloss acceptance of evidence produced by the canonical semantic-memory runtime.
//! This module never implements a codec or promotes a configured backend to an observed one.
use crate::db::notebook_db::SemanticMemoryProjectionStatus;
use serde::Serialize;
use serde_json::Value;

pub const TURBO_QUANT_BACKEND: &str = "turbo_quant_candidate_then_exact_f32";

/// Missing fields are unknown evidence, not zero faults. Raw-f32 fallback is
/// useful recovery, but cannot prove that TurboQuant supplied the candidates.
pub fn has_fresh_turbo_quant_proof(receipt: &Value) -> bool {
    receipt.get("candidate_backend").and_then(Value::as_str) == Some(TURBO_QUANT_BACKEND)
        && receipt.get("exact_rerank").and_then(Value::as_bool) == Some(true)
        && ["exact_rerank_count", "approximate_scanned_count", "approximate_returned_count"]
            .iter().all(|key| receipt.get(*key).and_then(Value::as_u64).is_some_and(|n| n > 0))
        && ["artifact_corruption_count", "vector_artifact_missing_count", "vector_artifact_stale_count"]
            .iter().all(|key| receipt.get(*key).and_then(Value::as_u64) == Some(0))
        && ["artifact_generation_id", "vector_artifact_manifest_digest"]
            .iter().all(|key| receipt.get(*key).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty()))
        && receipt.get("fallback") == Some(&Value::Null)
        // The canonical receipt omits fallback_reason when it is None.
        && receipt.get("fallback_reason").is_none_or(Value::is_null)
}

#[derive(Debug, Default)]
pub struct ProjectionArtifactProof {
    pub generation_id: Option<String>,
    pub manifest_digest: Option<String>,
    pub missing_sources: usize,
    pub stale_sources: usize,
    pub probe_matches: bool,
}

/// Evaluate every canonical source that currently owns chunks. Probe evidence
/// is reusable only for the exact artifact generation and digest it observed.
pub fn projection_artifact_proof(
    source_chunk_counts: &[(String, usize)],
    statuses: &[SemanticMemoryProjectionStatus],
    probe: Option<&Value>,
) -> ProjectionArtifactProof {
    let mut proof = ProjectionArtifactProof::default();
    if source_chunk_counts.is_empty() {
        proof.missing_sources = 1;
        return proof;
    }
    for (source_id, canonical_chunk_count) in source_chunk_counts {
        let Some(status) = statuses
            .iter()
            .find(|status| &status.source_id == source_id)
        else {
            proof.missing_sources += 1;
            continue;
        };
        let (Some(generation), Some(digest)) = (
            status
                .artifact_generation_id
                .as_deref()
                .filter(|s| !s.trim().is_empty()),
            status
                .vector_artifact_manifest_digest
                .as_deref()
                .filter(|s| !s.trim().is_empty()),
        ) else {
            proof.missing_sources += 1;
            continue;
        };
        if status.status != "synced"
            || status.chunk_count == 0
            || status.chunk_count != *canonical_chunk_count
            || status.projected_chunk_count < status.chunk_count
            || status.healthy_link_count < status.chunk_count
            || status.degraded_link_count > 0
        {
            proof.stale_sources += 1;
        }
        match (&proof.generation_id, &proof.manifest_digest) {
            (None, None) => {
                proof.generation_id = Some(generation.to_string());
                proof.manifest_digest = Some(digest.to_string());
            }
            (Some(previous_generation), Some(previous_digest))
                if previous_generation == generation && previous_digest == digest => {}
            _ => proof.stale_sources += 1,
        }
    }
    if proof.missing_sources == 0 && proof.stale_sources == 0 {
        proof.probe_matches = probe.is_some_and(|receipt| {
            has_fresh_turbo_quant_proof(receipt)
                && receipt
                    .get("artifact_generation_id")
                    .and_then(Value::as_str)
                    == proof.generation_id.as_deref()
                && receipt
                    .get("vector_artifact_manifest_digest")
                    .and_then(Value::as_str)
                    == proof.manifest_digest.as_deref()
        });
    } else {
        // Mixed or incomplete source generations have no single current identity.
        proof.generation_id = None;
        proof.manifest_digest = None;
    }
    proof
}

#[derive(Debug, Clone, Serialize)]
pub struct VectorArtifactStatus {
    pub compiled_turbo_quant: bool,
    pub runtime_turbo_quant_enabled: bool,
    pub candidate_backend: Option<String>,
    pub artifact_generation_id: Option<String>,
    pub vector_artifact_manifest_digest: Option<String>,
    pub vector_artifact_missing_count: usize,
    pub vector_artifact_stale_count: usize,
    pub exact_rerank: bool,
    pub exact_rerank_count: usize,
    pub last_receipt_id: Option<String>,
    pub last_error: Option<String>,
}

impl VectorArtifactStatus {
    /// Admission for a requested profile depends on proof, not current activation.
    pub fn turbo_quant_proof_ready(&self) -> bool {
        self.compiled_turbo_quant
            && self
                .candidate_backend
                .as_deref()
                .is_some_and(|backend| backend == TURBO_QUANT_BACKEND)
            && self
                .artifact_generation_id
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self
                .vector_artifact_manifest_digest
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            && self.vector_artifact_missing_count == 0
            && self.vector_artifact_stale_count == 0
            && self.exact_rerank
            && self.exact_rerank_count > 0
    }

    /// Current effective status additionally requires runtime activation.
    pub fn turbo_quant_effective(&self) -> bool {
        self.runtime_turbo_quant_enabled && self.turbo_quant_proof_ready()
    }
}

#[cfg(test)]
mod readiness_tests {
    use super::*;
    fn ready() -> VectorArtifactStatus {
        VectorArtifactStatus {
            compiled_turbo_quant: true,
            runtime_turbo_quant_enabled: false,
            candidate_backend: Some(TURBO_QUANT_BACKEND.into()),
            artifact_generation_id: Some("gen".into()),
            vector_artifact_manifest_digest: Some("digest".into()),
            vector_artifact_missing_count: 0,
            vector_artifact_stale_count: 0,
            exact_rerank: true,
            exact_rerank_count: 2,
            last_receipt_id: Some("probe".into()),
            last_error: None,
        }
    }
    #[test]
    fn strict_prospective_proof_does_not_require_current_activation() {
        let mut status = ready();
        assert!(status.turbo_quant_proof_ready());
        assert!(!status.turbo_quant_effective());
        status.runtime_turbo_quant_enabled = true;
        assert!(status.turbo_quant_proof_ready());
        assert!(status.turbo_quant_effective());
        for mutate in [
            |s: &mut VectorArtifactStatus| s.vector_artifact_stale_count = 1,
            |s: &mut VectorArtifactStatus| s.vector_artifact_missing_count = 1,
            |s: &mut VectorArtifactStatus| s.artifact_generation_id = Some(" ".into()),
            |s: &mut VectorArtifactStatus| s.vector_artifact_manifest_digest = None,
            |s: &mut VectorArtifactStatus| s.exact_rerank = false,
            |s: &mut VectorArtifactStatus| {
                s.candidate_backend = Some("unproven_turbo_quant".into())
            },
        ] {
            let mut status = ready();
            mutate(&mut status);
            assert!(!status.turbo_quant_proof_ready());
        }
    }
    #[test]
    fn retained_proof_must_match_current_source_generation() {
        let status = SemanticMemoryProjectionStatus {
            notebook_id: "nb".into(),
            source_id: "source".into(),
            status: "synced".into(),
            chunk_count: 1,
            projected_chunk_count: 1,
            healthy_link_count: 1,
            degraded_link_count: 0,
            last_receipt_id: Some("receipt".into()),
            last_error: None,
            artifact_generation_id: Some("gen".into()),
            vector_artifact_manifest_digest: Some("digest".into()),
            updated_at: String::new(),
        };
        let mut proof = serde_json::json!({"candidate_backend": TURBO_QUANT_BACKEND, "exact_rerank": true, "exact_rerank_count": 1, "approximate_scanned_count": 1, "approximate_returned_count": 1, "artifact_corruption_count": 0, "vector_artifact_missing_count": 0, "vector_artifact_stale_count": 0, "artifact_generation_id": "gen", "vector_artifact_manifest_digest": "digest", "fallback": null});
        assert!(
            projection_artifact_proof(&[("source".into(), 1)], &[status.clone()], Some(&proof))
                .probe_matches
        );
        assert!(
            !projection_artifact_proof(
                &[("other-source".into(), 1)],
                &[status.clone()],
                Some(&proof)
            )
            .probe_matches
        );
        proof["artifact_generation_id"] = serde_json::json!("other-generation");
        assert!(
            !projection_artifact_proof(&[("source".into(), 1)], &[status], Some(&proof))
                .probe_matches
        );
    }
}
