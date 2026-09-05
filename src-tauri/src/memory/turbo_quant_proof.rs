//! Gloss acceptance of evidence produced by the canonical semantic-memory runtime.
//! This module never implements a codec or promotes a configured backend to an observed one.
use crate::db::notebook_db::SemanticMemoryProjectionStatus;
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
