//! Authorized selective forgetting with dependency-closed invalidation receipts.

use crate::authority_contracts::{
    AuthorityFaultStage, AuthorityPermit, AuthoritySnapshotId, RetrievalEpoch,
};
use crate::db::with_transaction;
use crate::{MemoryError, MemoryStore};
use chrono::Utc;
use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeSet, VecDeque};
use std::sync::{Arc, Mutex};

pub const FORGETTING_CLOSURE_RECEIPT_V1: &str = "forgetting_closure_receipt_v1";
const FORGOTTEN_CONTENT: &str = "[FORGOTTEN]";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgettingClosureRequestV1 {
    pub schema_version: String,
    pub root_fact_ids: Vec<String>,
    pub namespace: String,
    pub reason: String,
    pub closure_budget: usize,
}

impl ForgettingClosureRequestV1 {
    pub fn new(
        mut root_fact_ids: Vec<String>,
        namespace: impl Into<String>,
        reason: impl Into<String>,
        closure_budget: usize,
    ) -> Self {
        for id in &mut root_fact_ids {
            if let Some(stripped) = id.strip_prefix("fact:") {
                *id = stripped.to_string();
            }
        }
        root_fact_ids.sort();
        root_fact_ids.dedup();
        Self {
            schema_version: "forgetting_closure_request_v1".into(),
            root_fact_ids,
            namespace: namespace.into(),
            reason: reason.into(),
            closure_budget,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ForgettingDispositionV1 {
    Applied,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ForgettingSurfaceRefV1 {
    pub surface: String,
    pub artifact_id: String,
}

impl ForgettingSurfaceRefV1 {
    fn new(surface: impl Into<String>, artifact_id: impl Into<String>) -> Self {
        Self {
            surface: surface.into(),
            artifact_id: artifact_id.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgettingVerificationV1 {
    pub surface: String,
    pub passed: bool,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgettingEpochsV1 {
    pub authority: RetrievalEpoch,
    pub projection: u64,
    pub cache: u64,
    pub export: u64,
    pub replay: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForgettingClosureReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub caller_idempotency_key: String,
    pub principal: String,
    pub namespace: String,
    pub root_fact_ids: Vec<String>,
    pub affected_canonical_ids: Vec<String>,
    pub removed_surfaces: Vec<ForgettingSurfaceRefV1>,
    pub invalidated_surfaces: Vec<ForgettingSurfaceRefV1>,
    pub deferred_surfaces: Vec<ForgettingSurfaceRefV1>,
    pub not_tested_surfaces: Vec<ForgettingSurfaceRefV1>,
    pub verification: Vec<ForgettingVerificationV1>,
    pub disposition: ForgettingDispositionV1,
    pub before_snapshot_id: AuthoritySnapshotId,
    pub after_snapshot_id: AuthoritySnapshotId,
    pub before_epoch: RetrievalEpoch,
    pub after_epoch: RetrievalEpoch,
    pub before_epochs: ForgettingEpochsV1,
    pub after_epochs: ForgettingEpochsV1,
    pub reason_digest: String,
    pub receipt_digest: String,
    pub committed_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ArtifactNode {
    kind: String,
    id: String,
}

struct ClosurePlan {
    canonical_ids: Vec<String>,
    artifacts: Vec<ArtifactNode>,
    graph_edge_ids: Vec<String>,
    derivation_edge_ids: Vec<i64>,
    search_receipt_ids: Vec<String>,
    vector_item_keys: Vec<String>,
}

pub(crate) async fn forget(
    store: &MemoryStore,
    permit: AuthorityPermit,
    caller_idempotency_key: String,
    request: ForgettingClosureRequestV1,
) -> Result<ForgettingClosureReceiptV1, MemoryError> {
    validate(&permit, &caller_idempotency_key, &request)?;
    // Clearing early is safe on rollback and prevents a committed closure from retaining a
    // poisoned or inaccessible cache entry.
    store.clear_search_cache_strict()?;
    let fault = store.inner.authority_fault.clone();
    let receipt = store
        .with_write_conn(move |conn| {
            // Safety: closure discovery, physical scrubbing, invalidation, epoch bumps, and the
            // immutable receipt either all commit or all roll back.
            with_transaction(conn, |tx| {
                execute_forgetting(tx, &permit, &caller_idempotency_key, &request, &fault)
            })
        })
        .await?;
    store.clear_search_cache_strict()?;
    Ok(receipt)
}

pub(crate) async fn get_receipt(
    store: &MemoryStore,
    caller_idempotency_key: &str,
) -> Result<Option<ForgettingClosureReceiptV1>, MemoryError> {
    let key = caller_idempotency_key.to_string();
    store
        .with_read_conn(move |conn| {
            let raw: Option<String> = conn
                .query_row(
                    "SELECT receipt_json FROM forgetting_closure_receipts
                     WHERE caller_idempotency_key = ?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()?;
            raw.map(|json| {
                serde_json::from_str(&json).map_err(|error| MemoryError::CorruptData {
                    table: "forgetting_closure_receipts",
                    row_id: key.clone(),
                    detail: format!("invalid forgetting receipt: {error}"),
                })
            })
            .transpose()
        })
        .await
}

fn validate(
    permit: &AuthorityPermit,
    key: &str,
    request: &ForgettingClosureRequestV1,
) -> Result<(), MemoryError> {
    if permit.capability != AuthorityPermit::FORGET_CAPABILITY
        || permit.principal.trim().is_empty()
        || permit.caller_id.trim().is_empty()
        || permit.origin_authority.is_none()
        || key.trim().is_empty()
    {
        return Err(MemoryError::AuthorityUnauthorized {
            operation: "forget".into(),
            principal: permit.principal.clone(),
        });
    }
    if request.schema_version != "forgetting_closure_request_v1"
        || request.root_fact_ids.is_empty()
        || request.root_fact_ids.iter().any(|id| id.trim().is_empty())
        || request.namespace.trim().is_empty()
        || request.reason.trim().is_empty()
    {
        return Err(MemoryError::ForgettingClosureIncomplete {
            detail: "request requires version, roots, namespace, and reason".into(),
        });
    }
    if request.closure_budget == 0 {
        return Err(MemoryError::ForgettingBudgetExceeded {
            budget: 0,
            required: 1,
        });
    }
    Ok(())
}

fn execute_forgetting(
    tx: &Transaction<'_>,
    permit: &AuthorityPermit,
    key: &str,
    request: &ForgettingClosureRequestV1,
    fault: &Arc<Mutex<Option<AuthorityFaultStage>>>,
) -> Result<ForgettingClosureReceiptV1, MemoryError> {
    let reason_digest = digest(&request.reason)?;
    let payload_digest = digest(&(
        &permit.principal,
        &permit.caller_id,
        &request.schema_version,
        &request.root_fact_ids,
        &request.namespace,
        &reason_digest,
        request.closure_budget,
    ))?;
    if let Some((stored_payload, receipt_json)) = tx
        .query_row(
            "SELECT payload_digest, receipt_json FROM forgetting_closure_receipts
             WHERE caller_idempotency_key = ?1",
            params![key],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?
    {
        if stored_payload != payload_digest {
            return Err(MemoryError::AuthorityIdempotencyConflict { key: key.into() });
        }
        return serde_json::from_str(&receipt_json).map_err(|error| MemoryError::CorruptData {
            table: "forgetting_closure_receipts",
            row_id: key.into(),
            detail: format!("invalid forgetting receipt: {error}"),
        });
    }

    let plan = compute_closure(tx, permit, request)?;
    let before_epochs = load_epochs(tx)?;
    let before_snapshot_id = snapshot_id(tx, before_epochs.authority.0)?;
    let receipt_id = uuid::Uuid::new_v4().to_string();
    let committed_at = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();

    fault_gate(fault, AuthorityFaultStage::BeforeForgettingMutation)?;
    let (mut removed, mut invalidated) = apply_plan(
        tx,
        &plan,
        permit,
        key,
        &receipt_id,
        &committed_at,
        &reason_digest,
    )?;
    fault_gate(fault, AuthorityFaultStage::AfterForgettingMutation)?;
    let after_epochs = bump_epochs(tx, &before_epochs)?;
    let after_snapshot_id = snapshot_id(tx, after_epochs.authority.0)?;
    removed.sort();
    removed.dedup();
    invalidated.sort();
    invalidated.dedup();
    let verification = verify_plan(tx, &plan)?;
    if verification.iter().any(|check| !check.passed) {
        return Err(MemoryError::ForgettingClosureIncomplete {
            detail: verification
                .iter()
                .filter(|check| !check.passed)
                .map(|check| format!("{}: {}", check.surface, check.detail))
                .collect::<Vec<_>>()
                .join("; "),
        });
    }

    let mut receipt = ForgettingClosureReceiptV1 {
        schema_version: FORGETTING_CLOSURE_RECEIPT_V1.into(),
        receipt_id: receipt_id.clone(),
        caller_idempotency_key: key.into(),
        principal: permit.principal.clone(),
        namespace: request.namespace.clone(),
        root_fact_ids: request.root_fact_ids.clone(),
        affected_canonical_ids: plan.canonical_ids.clone(),
        removed_surfaces: removed,
        invalidated_surfaces: invalidated,
        deferred_surfaces: Vec::new(),
        not_tested_surfaces: Vec::new(),
        verification,
        disposition: ForgettingDispositionV1::Applied,
        before_snapshot_id,
        after_snapshot_id,
        before_epoch: before_epochs.authority,
        after_epoch: after_epochs.authority,
        before_epochs,
        after_epochs,
        reason_digest,
        receipt_digest: String::new(),
        committed_at,
    };
    receipt.receipt_digest = digest(&receipt)?;
    let receipt_json = serde_json::to_string(&receipt)
        .map_err(|error| MemoryError::Other(format!("serialize forgetting receipt: {error}")))?;
    fault_gate(fault, AuthorityFaultStage::BeforeForgettingReceipt)?;
    tx.execute(
        "INSERT INTO forgetting_closure_receipts
         (receipt_id, caller_idempotency_key, payload_digest, receipt_json, receipt_digest, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![receipt.receipt_id, key, payload_digest, receipt_json,
                receipt.receipt_digest, receipt.committed_at],
    )?;
    fault_gate(fault, AuthorityFaultStage::AfterForgettingReceipt)?;
    Ok(receipt)
}

fn compute_closure(
    tx: &Transaction<'_>,
    permit: &AuthorityPermit,
    request: &ForgettingClosureRequestV1,
) -> Result<ClosurePlan, MemoryError> {
    let mut artifacts = BTreeSet::new();
    let mut queue = VecDeque::new();
    for id in &request.root_fact_ids {
        let node = ArtifactNode {
            kind: "fact".into(),
            id: id.clone(),
        };
        artifacts.insert(node.clone());
        queue.push_back(node);
    }
    let mut dependency_graph_edges = BTreeSet::new();
    let mut derivation_edges = BTreeSet::new();

    while let Some(node) = queue.pop_front() {
        if artifacts.len() > request.closure_budget {
            return Err(MemoryError::ForgettingBudgetExceeded {
                budget: request.closure_budget,
                required: artifacts.len(),
            });
        }
        if node.kind == "fact" {
            validate_fact_scope(tx, &node.id, &request.namespace, &permit.principal)?;
            let target = format!("fact:{}", node.id);
            let mut stmt = tx.prepare(
                "SELECT id, source, edge_type FROM graph_edges
                 WHERE target = ?1 AND is_invalidated = 0 ORDER BY id",
            )?;
            let rows = stmt
                .query_map(params![target], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })?
                .collect::<Result<Vec<_>, _>>()?;
            for (edge_id, source, edge_type) in rows {
                let relation = graph_relation(&edge_type)?;
                if matches!(
                    relation.as_deref(),
                    Some("derived_from_state" | "supersedes" | "redacts")
                ) {
                    dependency_graph_edges.insert(edge_id);
                    let source_id = source.strip_prefix("fact:").ok_or_else(|| {
                        MemoryError::ForgettingClosureIncomplete {
                            detail: format!("dependency source '{source}' is not a fact"),
                        }
                    })?;
                    let derived = ArtifactNode {
                        kind: "fact".into(),
                        id: source_id.into(),
                    };
                    if artifacts.insert(derived.clone()) {
                        queue.push_back(derived);
                    }
                }
            }
        }

        let mut stmt = tx.prepare(
            "SELECT id, target_kind, target_id FROM derivation_edges
             WHERE source_kind = ?1 AND source_id = ?2 AND is_invalidated = 0 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![node.kind, node.id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;
        for (edge_id, kind, id) in rows {
            if !matches!(
                kind.as_str(),
                "fact"
                    | "procedure"
                    | "claim"
                    | "claim_version"
                    | "relation_version"
                    | "entity"
                    | "entity_alias"
                    | "evidence_ref"
                    | "episode"
            ) {
                return Err(MemoryError::ForgettingClosureIncomplete {
                    detail: format!(
                        "derived artifact kind '{kind}' has no governed forgetting adapter"
                    ),
                });
            }
            derivation_edges.insert(edge_id);
            let derived = ArtifactNode { kind, id };
            if artifacts.insert(derived.clone()) {
                queue.push_back(derived);
            }
        }
    }

    let canonical_ids = artifacts
        .iter()
        .filter(|node| node.kind == "fact")
        .map(|node| node.id.clone())
        .collect::<Vec<_>>();
    let fact_nodes = canonical_ids
        .iter()
        .map(|id| format!("fact:{id}"))
        .collect::<BTreeSet<_>>();

    // Every incident graph edge is invalidated, not only edges used during traversal.
    let mut graph_edge_ids = dependency_graph_edges;
    let mut stmt = tx.prepare(
        "SELECT id, source, target FROM graph_edges WHERE is_invalidated = 0 ORDER BY id",
    )?;
    for row in stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
        ))
    })? {
        let (id, source, target) = row?;
        if fact_nodes.contains(&source) || fact_nodes.contains(&target) {
            graph_edge_ids.insert(id);
        }
    }

    let mut search_receipt_ids = BTreeSet::new();
    let mut stmt = tx.prepare("SELECT receipt_id, result_ids_json FROM search_receipts")?;
    for row in stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })? {
        let (receipt_id, raw_ids) = row?;
        let ids: Vec<String> =
            serde_json::from_str(&raw_ids).map_err(|error| MemoryError::CorruptData {
                table: "search_receipts",
                row_id: receipt_id.clone(),
                detail: format!("invalid result IDs: {error}"),
            })?;
        if ids.iter().any(|id| fact_nodes.contains(id)) {
            search_receipt_ids.insert(receipt_id);
        }
    }

    let mut vector_item_keys = BTreeSet::new();
    let mut stmt = tx.prepare(
        "SELECT DISTINCT item_key FROM derived_vector_artifacts WHERE status = 'active'",
    )?;
    for row in stmt.query_map([], |row| row.get::<_, String>(0))? {
        let item_key = row?;
        if fact_nodes.contains(&item_key) {
            vector_item_keys.insert(item_key);
        }
    }

    Ok(ClosurePlan {
        canonical_ids,
        artifacts: artifacts.into_iter().collect(),
        graph_edge_ids: graph_edge_ids.into_iter().collect(),
        derivation_edge_ids: derivation_edges.into_iter().collect(),
        search_receipt_ids: search_receipt_ids.into_iter().collect(),
        vector_item_keys: vector_item_keys.into_iter().collect(),
    })
}

fn graph_relation(edge_type: &str) -> Result<Option<String>, MemoryError> {
    let value: serde_json::Value = serde_json::from_str(edge_type).map_err(|error| {
        MemoryError::ForgettingClosureIncomplete {
            detail: format!("cannot interpret dependency edge type: {error}"),
        }
    })?;
    Ok(value
        .get("relation")
        .or_else(|| {
            value
                .get("entity")
                .and_then(|nested| nested.get("relation"))
        })
        .and_then(|relation| relation.as_str())
        .map(str::to_string))
}

fn validate_fact_scope(
    tx: &Transaction<'_>,
    fact_id: &str,
    namespace: &str,
    principal: &str,
) -> Result<(), MemoryError> {
    let row: Option<(String, Option<String>)> = tx
        .query_row(
            "SELECT f.namespace, o.label_json FROM facts f
             LEFT JOIN origin_authority_labels o ON o.fact_id = f.id WHERE f.id = ?1",
            params![fact_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((actual_namespace, Some(label_json))) = row else {
        return Err(MemoryError::ForgettingClosureIncomplete {
            detail: format!("canonical fact '{fact_id}' is missing or lacks origin authority"),
        });
    };
    let label: crate::OriginAuthorityLabelV1 =
        serde_json::from_str(&label_json).map_err(|error| MemoryError::CorruptData {
            table: "origin_authority_labels",
            row_id: fact_id.into(),
            detail: format!("invalid origin label: {error}"),
        })?;
    if actual_namespace != namespace || label.origin_principal != principal {
        return Err(MemoryError::ForgettingClosureIncomplete {
            detail: format!("fact '{fact_id}' is outside authorized namespace/principal closure"),
        });
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn apply_plan(
    tx: &Transaction<'_>,
    plan: &ClosurePlan,
    permit: &AuthorityPermit,
    key: &str,
    receipt_id: &str,
    committed_at: &str,
    reason_digest: &str,
) -> Result<(Vec<ForgettingSurfaceRefV1>, Vec<ForgettingSurfaceRefV1>), MemoryError> {
    let mut removed = Vec::new();
    let mut invalidated = Vec::new();
    for fact_id in &plan.canonical_ids {
        let (rowid, content, namespace, has_embedding, has_q8): (i64, String, String, bool, bool) =
            tx.query_row(
                "SELECT rm.rowid, f.content, f.namespace,
                    f.embedding IS NOT NULL, f.embedding_q8 IS NOT NULL
             FROM facts f JOIN facts_rowid_map rm ON rm.fact_id = f.id WHERE f.id = ?1",
                params![fact_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )?;
        let content_digest = digest(&content)?;
        tx.execute(
            "INSERT INTO facts_fts(facts_fts, rowid, content) VALUES('delete', ?1, ?2)",
            params![rowid, content],
        )?;
        tx.execute(
            "INSERT INTO facts_fts(rowid, content) VALUES(?1, ?2)",
            params![rowid, FORGOTTEN_CONTENT],
        )?;
        let tombstone_metadata = serde_json::json!({
            "forgetting_tombstone": {
                "receipt_id": receipt_id,
                "content_digest": content_digest,
            }
        })
        .to_string();
        tx.execute(
            "UPDATE facts SET content = ?1, source = NULL, embedding = NULL, embedding_q8 = NULL,
             metadata = ?2, updated_at = ?3 WHERE id = ?4",
            params![FORGOTTEN_CONTENT, tombstone_metadata, committed_at, fact_id],
        )?;
        tx.execute(
            "INSERT INTO forgotten_facts
             (fact_id, receipt_id, namespace, content_digest, forgotten_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![fact_id, receipt_id, namespace, content_digest, committed_at],
        )?;
        tx.execute(
            "UPDATE authority_versions SET is_redacted = 1 WHERE fact_id = ?1",
            params![fact_id],
        )?;
        tx.execute(
            "INSERT INTO origin_authority_revocations
             (revocation_id, fact_id, caller_idempotency_key, principal,
              revocation_reference, revoked_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                uuid::Uuid::new_v4().to_string(),
                fact_id,
                format!("{key}:{fact_id}"),
                permit.principal,
                format!("forgetting:{reason_digest}"),
                committed_at,
            ],
        )?;
        tx.execute(
            "INSERT INTO pending_index_ops
             (item_key, entity_type, op_kind, attempt_count, last_error, updated_at)
             VALUES (?1, 'fact', 'delete', 0, NULL, ?2)
             ON CONFLICT(item_key) DO UPDATE SET op_kind = 'delete', attempt_count = 0,
                 last_error = NULL, updated_at = excluded.updated_at",
            params![format!("fact:{fact_id}"), committed_at],
        )?;
        removed.push(ForgettingSurfaceRefV1::new("canonical_content", fact_id));
        removed.push(ForgettingSurfaceRefV1::new("fts", fact_id));
        if has_embedding {
            removed.push(ForgettingSurfaceRefV1::new("vector", fact_id));
        }
        if has_q8 {
            removed.push(ForgettingSurfaceRefV1::new("quantized_vector", fact_id));
        }
        invalidated.push(ForgettingSurfaceRefV1::new("origin_authority", fact_id));
        invalidated.push(ForgettingSurfaceRefV1::new(
            "ann_index",
            format!("fact:{fact_id}"),
        ));
    }

    for edge_id in &plan.graph_edge_ids {
        tx.execute(
            "UPDATE graph_edges SET is_invalidated = 1, invalidated_at = ?1,
             invalidation_reason = ?2 WHERE id = ?3 AND is_invalidated = 0",
            params![committed_at, format!("forgetting:{reason_digest}"), edge_id],
        )?;
        invalidated.push(ForgettingSurfaceRefV1::new("graph", edge_id));
    }
    for edge_id in &plan.derivation_edge_ids {
        tx.execute(
            "UPDATE derivation_edges SET is_invalidated = 1, invalidated_at = ?1,
             invalidation_reason = ?2 WHERE id = ?3 AND is_invalidated = 0",
            params![committed_at, format!("forgetting:{reason_digest}"), edge_id],
        )?;
        invalidated.push(ForgettingSurfaceRefV1::new(
            "projection_derivation",
            edge_id.to_string(),
        ));
    }
    for item_key in &plan.vector_item_keys {
        tx.execute(
            "UPDATE derived_vector_artifacts SET status = 'invalidated'
             WHERE item_key = ?1 AND status = 'active'",
            params![item_key],
        )?;
        invalidated.push(ForgettingSurfaceRefV1::new("derived_vector", item_key));
    }
    for artifact in &plan.artifacts {
        if artifact.kind != "fact" && scrub_projection_artifact(tx, artifact)? {
            removed.push(ForgettingSurfaceRefV1::new(
                format!("projection_payload:{}", artifact.kind),
                &artifact.id,
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO forgetting_artifact_invalidations
             (surface_kind, artifact_id, receipt_id, invalidated_at) VALUES (?1, ?2, ?3, ?4)",
            params![artifact.kind, artifact.id, receipt_id, committed_at],
        )?;
        if artifact.kind != "fact" {
            invalidated.push(ForgettingSurfaceRefV1::new(
                format!("projection:{}", artifact.kind),
                &artifact.id,
            ));
        }
    }
    for search_receipt_id in &plan.search_receipt_ids {
        tx.execute(
            "INSERT OR IGNORE INTO forgetting_artifact_invalidations
             (surface_kind, artifact_id, receipt_id, invalidated_at)
             VALUES ('search_receipt', ?1, ?2, ?3)",
            params![search_receipt_id, receipt_id, committed_at],
        )?;
        invalidated.push(ForgettingSurfaceRefV1::new("replay", search_receipt_id));
        invalidated.push(ForgettingSurfaceRefV1::new("export", search_receipt_id));
    }
    invalidated.push(ForgettingSurfaceRefV1::new("search_cache", "all"));
    Ok((removed, invalidated))
}

fn scrub_projection_artifact(
    tx: &Transaction<'_>,
    artifact: &ArtifactNode,
) -> Result<bool, MemoryError> {
    let changed = match artifact.kind.as_str() {
        "claim_version" => tx.execute(
            "UPDATE claim_versions SET content = ?1, subject_entity_id = '[FORGOTTEN]',
             predicate = '[FORGOTTEN]', object_anchor = 'null', metadata = NULL,
             preferred_open = 0, claim_state = 'retracted', freshness = 'superseded'
             WHERE claim_version_id = ?2",
            params![FORGOTTEN_CONTENT, artifact.id],
        )?,
        "relation_version" => tx.execute(
            "UPDATE relation_versions SET subject_entity_id = '[FORGOTTEN]',
             predicate = '[FORGOTTEN]', object_anchor = 'null', metadata = NULL,
             preferred_open = 0, freshness = 'superseded'
             WHERE relation_version_id = ?1",
            params![artifact.id],
        )?,
        "entity" | "entity_alias" => tx.execute(
            "UPDATE entity_aliases SET alias_text = ?1, match_evidence = NULL,
             alias_source = '[FORGOTTEN]' WHERE canonical_entity_id = ?2",
            params![FORGOTTEN_CONTENT, artifact.id],
        )?,
        // Evidence handles are capabilities, so physical removal is safer than a reusable marker.
        "evidence_ref" => tx.execute(
            "DELETE FROM evidence_refs WHERE fetch_handle = ?1",
            params![artifact.id],
        )?,
        "episode" => tx.execute(
            "UPDATE episode_links SET document_id = '[FORGOTTEN]', cause_ids = '[]',
             effect_type = '[FORGOTTEN]', outcome = '[FORGOTTEN]', metadata = NULL
             WHERE episode_id = ?1",
            params![artifact.id],
        )?,
        // Procedures are immutable. Forgetting their factual evidence source records a durable
        // invalidation in forgetting_artifact_invalidations; governed procedural retrieval checks
        // that table uniformly across search/direct/cache/export/replay paths.
        "procedure" => 0,
        _ => 0,
    };
    Ok(changed > 0)
}

fn verify_plan(
    tx: &Transaction<'_>,
    plan: &ClosurePlan,
) -> Result<Vec<ForgettingVerificationV1>, MemoryError> {
    let mut checks = Vec::new();
    for fact_id in &plan.canonical_ids {
        let (content, embedding_absent, q8_absent, forgotten): (String, bool, bool, bool) = tx
            .query_row(
                "SELECT f.content, f.embedding IS NULL, f.embedding_q8 IS NULL,
                        EXISTS(SELECT 1 FROM forgotten_facts ff WHERE ff.fact_id = f.id)
                 FROM facts f WHERE f.id = ?1",
                params![fact_id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )?;
        checks.push(ForgettingVerificationV1 {
            surface: format!("direct_id_raw:{fact_id}"),
            passed: content == FORGOTTEN_CONTENT && embedding_absent && q8_absent && forgotten,
            detail: "content tombstoned; dense and quantized vectors absent".into(),
        });
        let fts_rows: i64 = tx.query_row(
            "SELECT COUNT(*) FROM facts_fts
             JOIN facts_rowid_map rm ON rm.rowid = facts_fts.rowid
             WHERE rm.fact_id = ?1",
            params![fact_id],
            |row| row.get(0),
        )?;
        checks.push(ForgettingVerificationV1 {
            surface: format!("fts:{fact_id}"),
            passed: fts_rows == 1,
            detail: format!("FTS tombstone rows after exact delete/reinsert: {fts_rows}"),
        });
        let revocations: i64 = tx.query_row(
            "SELECT COUNT(*) FROM origin_authority_revocations WHERE fact_id = ?1",
            params![fact_id],
            |row| row.get(0),
        )?;
        checks.push(ForgettingVerificationV1 {
            surface: format!("governed_direct_export:{fact_id}"),
            passed: forgotten && revocations > 0,
            detail: format!("forgetting tombstone present; revocations: {revocations}"),
        });
        checks.push(ForgettingVerificationV1 {
            surface: format!("ordinary_current_historical:{fact_id}"),
            passed: forgotten,
            detail: "all StateView policies exclude forgotten_facts".into(),
        });
        let pending_delete: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM pending_index_ops
             WHERE item_key = ?1 AND op_kind = 'delete')",
            params![format!("fact:{fact_id}")],
            |row| row.get(0),
        )?;
        let active_vectors: i64 = tx.query_row(
            "SELECT COUNT(*) FROM derived_vector_artifacts
             WHERE item_key = ?1 AND status = 'active'",
            params![format!("fact:{fact_id}")],
            |row| row.get(0),
        )?;
        checks.push(ForgettingVerificationV1 {
            surface: format!("vector_ann:{fact_id}"),
            passed: embedding_absent && q8_absent && active_vectors == 0 && pending_delete,
            detail: format!(
                "source vectors absent; active derived vectors: {active_vectors}; ANN delete queued"
            ),
        });
        let active_edges: i64 = tx.query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE is_invalidated = 0
             AND (source = ?1 OR target = ?1)",
            params![format!("fact:{fact_id}")],
            |row| row.get(0),
        )?;
        checks.push(ForgettingVerificationV1 {
            surface: format!("graph:{fact_id}"),
            passed: active_edges == 0,
            detail: format!("active incident edges: {active_edges}"),
        });
    }
    let mut active_derivations = 0_i64;
    for id in &plan.derivation_edge_ids {
        active_derivations += tx.query_row(
            "SELECT COUNT(*) FROM derivation_edges WHERE id = ?1 AND is_invalidated = 0",
            params![id],
            |row| row.get::<_, i64>(0),
        )?;
    }
    checks.push(ForgettingVerificationV1 {
        surface: "projection_derivation".into(),
        passed: active_derivations == 0,
        detail: format!("active affected derivations: {active_derivations}"),
    });
    let invalidated_artifacts: i64 = tx.query_row(
        "SELECT COUNT(*) FROM forgetting_artifact_invalidations WHERE receipt_id = (
             SELECT receipt_id FROM forgotten_facts WHERE fact_id = ?1
         )",
        params![plan.canonical_ids.first().ok_or_else(|| {
            MemoryError::ForgettingClosureIncomplete {
                detail: "closure unexpectedly contains no canonical facts".into(),
            }
        })?],
        |row| row.get(0),
    )?;
    checks.push(ForgettingVerificationV1 {
        surface: "projection_export_replay".into(),
        passed: invalidated_artifacts >= plan.artifacts.len() as i64,
        detail: format!("durable artifact invalidations: {invalidated_artifacts}"),
    });
    checks.push(ForgettingVerificationV1 {
        surface: "cache".into(),
        passed: true,
        detail: "strict in-process cache clear completed before transactional mutation".into(),
    });
    Ok(checks)
}

fn load_epochs(tx: &Transaction<'_>) -> Result<ForgettingEpochsV1, MemoryError> {
    let values: (i64, i64, i64, i64, i64) = tx.query_row(
        "SELECT retrieval_epoch, projection_epoch, cache_epoch, export_epoch, replay_epoch
         FROM authority_state WHERE id = 1",
        [],
        |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        },
    )?;
    let convert = |value: i64| {
        u64::try_from(value).map_err(|_| MemoryError::Other("negative forgetting epoch".into()))
    };
    Ok(ForgettingEpochsV1 {
        authority: RetrievalEpoch(convert(values.0)?),
        projection: convert(values.1)?,
        cache: convert(values.2)?,
        export: convert(values.3)?,
        replay: convert(values.4)?,
    })
}

fn bump_epochs(
    tx: &Transaction<'_>,
    before: &ForgettingEpochsV1,
) -> Result<ForgettingEpochsV1, MemoryError> {
    let next = |value: u64| {
        value
            .checked_add(1)
            .ok_or_else(|| MemoryError::Other("forgetting epoch overflow".into()))
    };
    let after = ForgettingEpochsV1 {
        authority: RetrievalEpoch(next(before.authority.0)?),
        projection: next(before.projection)?,
        cache: next(before.cache)?,
        export: next(before.export)?,
        replay: next(before.replay)?,
    };
    let changed = tx.execute(
        "UPDATE authority_state SET retrieval_epoch = ?1, projection_epoch = ?2,
         cache_epoch = ?3, export_epoch = ?4, replay_epoch = ?5
         WHERE id = 1 AND retrieval_epoch = ?6 AND projection_epoch = ?7
           AND cache_epoch = ?8 AND export_epoch = ?9 AND replay_epoch = ?10",
        params![
            after.authority.0 as i64,
            after.projection as i64,
            after.cache as i64,
            after.export as i64,
            after.replay as i64,
            before.authority.0 as i64,
            before.projection as i64,
            before.cache as i64,
            before.export as i64,
            before.replay as i64,
        ],
    )?;
    if changed != 1 {
        return Err(MemoryError::ForgettingClosureIncomplete {
            detail: "authority/projection/cache/export/replay epochs changed concurrently".into(),
        });
    }
    Ok(after)
}

fn snapshot_id(tx: &Transaction<'_>, epoch: u64) -> Result<AuthoritySnapshotId, MemoryError> {
    let mut stmt = tx
        .prepare("SELECT lineage_id, active_head_id FROM authority_lineages ORDER BY lineage_id")?;
    let heads = stmt
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(AuthoritySnapshotId(format!(
        "epoch:{epoch}:{}",
        digest(&(epoch, heads))?
    )))
}

fn digest<T: Serialize>(value: &T) -> Result<String, MemoryError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| MemoryError::DigestError(format!("serialize digest input: {error}")))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

fn fault_gate(
    fault: &Arc<Mutex<Option<AuthorityFaultStage>>>,
    stage: AuthorityFaultStage,
) -> Result<(), MemoryError> {
    let mut guard = fault
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.as_ref() == Some(&stage) {
        *guard = None;
        return Err(MemoryError::AuthorityFaultInjected { stage });
    }
    Ok(())
}
