//! Deterministic admission checks for [`MemoryTransitionCandidateV1`].

use crate::error::MemoryError;
use crate::transition_contracts::*;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) fn verify_candidate(
    tx: &Transaction<'_>,
    candidate: &MemoryTransitionCandidateV1,
) -> Result<MemoryTransitionVerificationV1, MemoryError> {
    let candidate_digest = digest(candidate)?;
    let mut diagnostics = Vec::new();
    if let Err(error) = candidate.validate() {
        diagnostics.push(error);
    }

    let artifacts: BTreeMap<&str, &SourceArtifactV1> = candidate
        .source_artifacts
        .iter()
        .map(|artifact| (artifact.artifact_id.as_str(), artifact))
        .collect();
    if artifacts.len() != candidate.source_artifacts.len() {
        diagnostics.push("source artifact IDs must be unique".into());
    }
    for artifact in &candidate.source_artifacts {
        let actual = blake3::hash(artifact.content.as_bytes())
            .to_hex()
            .to_string();
        if actual != artifact.content_digest {
            diagnostics.push(format!(
                "source artifact '{}' content digest mismatch",
                artifact.artifact_id
            ));
        }
    }
    let source_digest = digest(&candidate.source_artifacts)?;

    let cited: BTreeSet<SourceSpanRefV1> = candidate
        .assertions
        .iter()
        .flat_map(|assertion| assertion.source_spans.iter().cloned())
        .chain(match &candidate.operation {
            TransitionOperation::Retract { source_spans, .. } => source_spans.clone(),
            _ => Vec::new(),
        })
        .collect();
    let mut omitted_spans = Vec::new();
    for required in &candidate.required_source_spans {
        if !cited.contains(required) {
            omitted_spans.push(OmittedSourceSpanV1 {
                span: required.clone(),
                exact_text: resolve_span(&artifacts, required).unwrap_or_default(),
            });
        }
    }
    omitted_spans.sort_by(|a, b| a.span.cmp(&b.span));
    let coverage = VerificationScore {
        satisfied: (candidate.required_source_spans.len() - omitted_spans.len()) as u64,
        total: candidate.required_source_spans.len() as u64,
    };

    let mut unsupported_spans = Vec::new();
    let mut faithful = 0_u64;
    for assertion in &candidate.assertions {
        let exact_parts = assertion
            .source_spans
            .iter()
            .map(|span| resolve_span(&artifacts, span))
            .collect::<Result<Vec<_>, _>>();
        match exact_parts {
            Ok(parts) if parts.concat() == assertion.content => faithful += 1,
            Ok(_) => unsupported_spans.push(UnsupportedAssertionSpanV1 {
                assertion_id: assertion.assertion_id.clone(),
                start_byte: 0,
                end_byte: assertion.content.len(),
                exact_text: assertion.content.clone(),
                reason: "assertion bytes are not the exact concatenation of cited source spans"
                    .into(),
            }),
            Err(reason) => unsupported_spans.push(UnsupportedAssertionSpanV1 {
                assertion_id: assertion.assertion_id.clone(),
                start_byte: 0,
                end_byte: assertion.content.len(),
                exact_text: assertion.content.clone(),
                reason,
            }),
        }
    }
    unsupported_spans.sort_by(|a, b| {
        (&a.assertion_id, a.start_byte, a.end_byte).cmp(&(
            &b.assertion_id,
            b.start_byte,
            b.end_byte,
        ))
    });
    let faithfulness = VerificationScore {
        satisfied: faithful,
        total: candidate.assertions.len() as u64,
    };

    let mut namespaces: BTreeSet<String> = candidate
        .assertions
        .iter()
        .map(|assertion| assertion.namespace.clone())
        .collect();
    let replaced_fact_id = match &candidate.operation {
        TransitionOperation::Supersede { draft } => Some(draft.target_fact_id.clone()),
        TransitionOperation::Retract { target_fact_id, .. } => Some(target_fact_id.clone()),
        TransitionOperation::Append { .. } => None,
    };
    let target_namespace = if let Some(target) = replaced_fact_id.as_deref() {
        tx.query_row(
            "SELECT f.namespace
             FROM authority_versions av
             JOIN facts f ON f.id = av.fact_id
             WHERE av.fact_id = ?1 AND av.is_active = 1",
            params![target],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    } else {
        None
    };
    if let Some(namespace) = &target_namespace {
        namespaces.insert(namespace.clone());
        if let TransitionOperation::Supersede { draft } = &candidate.operation {
            if let Some(assertion) = candidate
                .assertions
                .iter()
                .find(|assertion| assertion.assertion_id == draft.replacement_assertion_id)
            {
                if assertion.namespace != *namespace {
                    diagnostics.push(
                        "replacement assertion namespace must match the target namespace".into(),
                    );
                }
            }
        }
    }
    let mut active_fact_ids = Vec::new();
    for namespace in namespaces {
        let mut stmt = tx.prepare(
            "SELECT av.fact_id
             FROM authority_versions av
             JOIN facts f ON f.id = av.fact_id
             WHERE av.is_active = 1 AND f.namespace = ?1
             ORDER BY av.fact_id",
        )?;
        active_fact_ids.extend(
            stmt.query_map(params![namespace], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?,
        );
    }
    active_fact_ids.sort();
    active_fact_ids.dedup();
    let preserved: BTreeSet<&str> = candidate
        .preserved_active_fact_ids
        .iter()
        .map(String::as_str)
        .collect();
    let omitted_active_fact_ids: Vec<String> = active_fact_ids
        .iter()
        .filter(|id| {
            replaced_fact_id.as_deref() != Some(id.as_str()) && !preserved.contains(id.as_str())
        })
        .cloned()
        .collect();
    let preservation = VerificationScore {
        satisfied: (active_fact_ids.len() - omitted_active_fact_ids.len()) as u64,
        total: active_fact_ids.len() as u64,
    };

    let target_consistent = match replaced_fact_id.as_deref() {
        Some(target) => active_fact_ids.iter().any(|id| id == target),
        None => true,
    };
    if !target_consistent {
        diagnostics.push("supersession/retraction target is not an active head".into());
    }
    let mut projected_active_fact_ids: Vec<String> = active_fact_ids
        .iter()
        .filter(|id| Some(id.as_str()) != replaced_fact_id.as_deref())
        .cloned()
        .collect();
    let projected_candidate_head = match &candidate.operation {
        TransitionOperation::Append { assertion_id } => format!("candidate:{assertion_id}"),
        TransitionOperation::Supersede { draft } => {
            format!("candidate:{}", draft.replacement_assertion_id)
        }
        TransitionOperation::Retract { .. } => {
            format!("candidate:{}:redaction", candidate.candidate_id)
        }
    };
    projected_active_fact_ids.push(projected_candidate_head);
    projected_active_fact_ids.sort();
    let active_head_simulation = ActiveHeadSimulationV1 {
        consistent: target_consistent && omitted_active_fact_ids.is_empty(),
        replaced_fact_id: replaced_fact_id.clone(),
        projected_active_fact_ids,
    };

    let mut requested_fact_ids: Vec<String> = candidate
        .assertions
        .iter()
        .flat_map(|assertion| assertion.dependency_fact_ids.iter().cloned())
        .collect();
    requested_fact_ids.sort();
    requested_fact_ids.dedup();
    let mut missing_or_inactive_fact_ids = Vec::new();
    for fact_id in &requested_fact_ids {
        let active: Option<i64> = tx
            .query_row(
                "SELECT is_active FROM authority_versions WHERE fact_id = ?1",
                params![fact_id],
                |row| row.get(0),
            )
            .optional()?;
        if active != Some(1) || replaced_fact_id.as_deref() == Some(fact_id) {
            missing_or_inactive_fact_ids.push(fact_id.clone());
        }
    }
    if !missing_or_inactive_fact_ids.is_empty() {
        diagnostics.push("assertion dependencies must reference preserved active facts".into());
    }
    let mut preserved_edge_ids = Vec::new();
    if let Some(target) = replaced_fact_id.as_deref() {
        let node = format!("fact:{target}");
        let mut stmt = tx.prepare(
            "SELECT id FROM graph_edges
             WHERE is_invalidated = 0 AND (source = ?1 OR target = ?1)
             ORDER BY id",
        )?;
        preserved_edge_ids = stmt
            .query_map(params![node], |row| row.get(0))?
            .collect::<Result<Vec<_>, _>>()?;
    }
    let dependency_simulation = DependencySimulationV1 {
        requested_fact_ids,
        missing_or_inactive_fact_ids,
        preserved_edge_ids,
    };

    let disposition = if coverage.is_complete()
        && preservation.is_complete()
        && faithfulness.is_complete()
        && unsupported_spans.is_empty()
        && diagnostics.is_empty()
        && active_head_simulation.consistent
    {
        TransitionDisposition::Commit
    } else {
        TransitionDisposition::Quarantine
    };
    let mut verification = MemoryTransitionVerificationV1 {
        schema_version: MEMORY_TRANSITION_VERIFICATION_V1.into(),
        candidate_id: candidate.candidate_id.clone(),
        candidate_digest,
        source_digest,
        coverage,
        preservation,
        faithfulness,
        omitted_spans,
        unsupported_spans,
        omitted_active_fact_ids,
        diagnostics,
        active_head_simulation,
        dependency_simulation,
        disposition,
        verification_digest: String::new(),
    };
    verification.verification_digest = digest(&verification)?;
    Ok(verification)
}

fn resolve_span(
    artifacts: &BTreeMap<&str, &SourceArtifactV1>,
    span: &SourceSpanRefV1,
) -> Result<String, String> {
    let artifact = artifacts
        .get(span.artifact_id.as_str())
        .ok_or_else(|| format!("source artifact '{}' is missing", span.artifact_id))?;
    if span.start_byte >= span.end_byte
        || span.end_byte > artifact.content.len()
        || !artifact.content.is_char_boundary(span.start_byte)
        || !artifact.content.is_char_boundary(span.end_byte)
    {
        return Err(format!(
            "source span {}:{}..{} is outside UTF-8 boundaries",
            span.artifact_id, span.start_byte, span.end_byte
        ));
    }
    Ok(artifact.content[span.start_byte..span.end_byte].to_string())
}

pub(crate) fn digest<T: Serialize>(value: &T) -> Result<String, MemoryError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| MemoryError::DigestError(format!("serialize transition: {error}")))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}
