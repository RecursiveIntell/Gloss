//! Bounded evidence-gap retrieval and state-aware reranking.
//!
//! This is an interpretation layer over the existing search, graph, authority, and witnessed
//! retrieval paths. It does not create a second evidence store and it never treats an empty,
//! stale, conflicted, or budget-limited result as a confident negative.

use crate::authority_contracts::{RetrievalWitnessV1, StageOutcomeV1};
use crate::origin_authority::{
    AuthorityScopeV1, GovernedAccessRequestV1, OriginAuthorityRecordV1, OriginRiskV1,
};
use crate::state_epistemics::{
    answer_policy_for, resolve_dependency_states, AnswerPolicyDecision, DependencyResolutionV1,
    DependencyState, PremiseStatus, StateResolutionMode, StateResolutionReceiptV1,
};
use crate::types::{GraphEdgeType, SearchContext, SearchResult, SearchSource, SearchSourceType};
use crate::{MemoryError, MemoryStore, StateView};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const EVIDENCE_GAP_V1: &str = "evidence_gap_v1";
pub const EVIDENCE_PACKET_V1: &str = "evidence_packet_v1";
pub const EVIDENCE_ROUTE_RECEIPT_V1: &str = "evidence_route_receipt_v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceTerminalOutcomeV1 {
    Sufficient,
    EvidenceInsufficient,
    BudgetExceeded,
    Conflict,
    InvalidLineage,
}

/// Compatibility spelling for callers that name the terminal result after the controller.
pub type EvidenceGapOutcomeV1 = EvidenceTerminalOutcomeV1;
pub type EvidenceTerminalOutcome = EvidenceTerminalOutcomeV1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceGapReasonV1 {
    MissingRequiredEvidence,
    MissingStateSupport,
    MissingOriginAuthority,
    NoCandidates,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceGapV1 {
    pub schema_version: String,
    pub gap_id: String,
    pub reason: EvidenceGapReasonV1,
    pub requirement: String,
    pub missing_items: Vec<String>,
    pub diagnosed_by: Vec<String>,
    pub follow_up_queries: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceRetrievalRouteV1 {
    InitialHybrid,
    Lexical,
    Vector,
    Graph,
    Episodic,
    DirectId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRouteReceiptV1 {
    pub schema_version: String,
    pub route: EvidenceRetrievalRouteV1,
    pub query: Option<String>,
    pub stage_outcome: StageOutcomeV1,
    pub result_ids: Vec<String>,
    pub order_changed: bool,
    pub budget_unit: usize,
    pub note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateRerankWeightsV1 {
    pub state_relevance: f64,
    pub support: f64,
    pub origin_authority: f64,
}

impl Default for StateRerankWeightsV1 {
    fn default() -> Self {
        Self {
            // State relevance is intentionally dominant: stale lexical matches must not win
            // merely because they have a higher BM25/vector fusion score.
            state_relevance: 2.0,
            support: 0.5,
            origin_authority: 0.25,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StateRerankCandidateV1 {
    pub result: SearchResult,
    pub state_status: PremiseStatus,
    pub state_relevance: f64,
    pub support_score: f64,
    pub origin_authority_score: f64,
    pub original_score: f64,
    pub reranked_score: f64,
    pub origin_label_digest: Option<String>,
}

impl StateRerankCandidateV1 {
    pub fn from_result(
        result: SearchResult,
        state_status: PremiseStatus,
        state_relevance: f64,
    ) -> Self {
        let support_score = match (result.bm25_rank.is_some(), result.vector_rank.is_some()) {
            (true, true) => 1.0,
            (true, false) | (false, true) => 0.7,
            (false, false) => 0.25,
        };
        let original_score = result.score;
        let state_relevance = state_relevance.min(state_relevance_for_status(state_status));
        Self {
            result,
            state_status,
            state_relevance,
            support_score,
            origin_authority_score: 0.5,
            original_score,
            reranked_score: original_score,
            origin_label_digest: None,
        }
    }

    fn with_origin(mut self, origin: Option<&OriginAuthorityRecordV1>) -> Self {
        self.origin_label_digest = origin.map(|record| record.label_digest.clone());
        self.origin_authority_score =
            origin.map_or(0.0, |record| origin_authority_score(&record.label));
        self
    }
}

/// Rerank while retaining the original SearchResult, including its lexical rank, vector rank,
/// cosine similarity, and fused score. The new score is additive and separately inspectable.
pub fn rerank_state_aware(
    mut candidates: Vec<StateRerankCandidateV1>,
    weights: StateRerankWeightsV1,
) -> Vec<StateRerankCandidateV1> {
    for candidate in &mut candidates {
        candidate.reranked_score = candidate.original_score
            + weights.state_relevance * candidate.state_relevance
            + weights.support * candidate.support_score
            + weights.origin_authority * candidate.origin_authority_score;
    }
    candidates.sort_by(|a, b| {
        b.reranked_score.total_cmp(&a.reranked_score).then_with(|| {
            a.result
                .source
                .result_id()
                .cmp(&b.result.source.result_id())
        })
    });
    candidates
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePacketItemV1 {
    pub result: SearchResult,
    pub state_status: PremiseStatus,
    pub state_relevance: f64,
    pub support_score: f64,
    pub origin_authority_score: f64,
    pub original_score: f64,
    pub reranked_score: f64,
    pub origin_label_digest: Option<String>,
}

impl From<StateRerankCandidateV1> for EvidencePacketItemV1 {
    fn from(value: StateRerankCandidateV1) -> Self {
        Self {
            result: value.result,
            state_status: value.state_status,
            state_relevance: value.state_relevance,
            support_score: value.support_score,
            origin_authority_score: value.origin_authority_score,
            original_score: value.original_score,
            reranked_score: value.reranked_score,
            origin_label_digest: value.origin_label_digest,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceAblationReceiptV1 {
    pub schema_version: String,
    pub baseline_result_ids: Vec<String>,
    pub gap_loop_result_ids: Vec<String>,
    pub packet_digest_without_gap_loop: String,
    pub packet_digest_with_gap_loop: String,
    pub answer_without_gap_loop: Option<String>,
    pub answer_with_gap_loop: Option<String>,
    pub baseline_outcome: EvidenceTerminalOutcomeV1,
    pub final_outcome: EvidenceTerminalOutcomeV1,
    pub packet_changed: bool,
    pub order_changed: bool,
    pub answer_changed: bool,
    pub improved: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidenceGapRequestV1 {
    pub schema_version: String,
    pub request_id: String,
    pub query: String,
    pub top_k: usize,
    pub namespaces: Option<Vec<String>>,
    pub state_mode: StateResolutionMode,
    pub budget: usize,
    pub follow_up_limit: usize,
    pub required_evidence: Vec<String>,
    pub direct_ids: Vec<String>,
    pub access_request: Option<GovernedAccessRequestV1>,
    pub rerank_weights: StateRerankWeightsV1,
}

impl EvidenceGapRequestV1 {
    pub fn new(query: impl Into<String>, state_view: StateView) -> Self {
        let state_mode = match state_view {
            StateView::Current => StateResolutionMode::Current,
            StateView::HistoricalAt(value) => StateResolutionMode::HistoricalAt(value),
            StateView::RecordedAsOf(value) => StateResolutionMode::HistoricalAt(value),
            StateView::IncludeSuperseded => StateResolutionMode::Trajectory { points: Vec::new() },
        };
        let query = query.into();
        Self {
            schema_version: EVIDENCE_GAP_V1.into(),
            request_id: format!("evidence-gap:{}", uuid::Uuid::new_v4()),
            query,
            top_k: 8,
            namespaces: None,
            state_mode,
            budget: 8,
            follow_up_limit: 4,
            required_evidence: Vec::new(),
            direct_ids: Vec::new(),
            access_request: None,
            rerank_weights: StateRerankWeightsV1::default(),
        }
    }

    pub fn from_mode(query: impl Into<String>, state_mode: StateResolutionMode) -> Self {
        let mut request = Self::new(query, state_mode.state_view());
        request.state_mode = state_mode;
        request
    }

    pub fn with_required_evidence(mut self, evidence: Vec<String>) -> Self {
        self.required_evidence = evidence;
        self
    }

    pub fn with_access_request(mut self, request: GovernedAccessRequestV1) -> Self {
        self.access_request = Some(request);
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvidencePacketV1 {
    pub schema_version: String,
    pub packet_id: String,
    pub request_id: String,
    pub query: String,
    pub state_view: StateView,
    pub outcome: EvidenceTerminalOutcomeV1,
    pub items: Vec<EvidencePacketItemV1>,
    pub gaps: Vec<EvidenceGapV1>,
    pub routes: Vec<EvidenceRouteReceiptV1>,
    pub answer: Option<String>,
    pub answer_policy: AnswerPolicyDecision,
    pub state_resolution_receipt: StateResolutionReceiptV1,
    pub retrieval_witness: RetrievalWitnessV1,
    pub ablation: EvidenceAblationReceiptV1,
    pub packet_digest: String,
}

fn origin_authority_score(label: &crate::OriginAuthorityLabelV1) -> f64 {
    if label.revocation_status != crate::RevocationStatusV1::Active {
        return 0.0;
    }
    let risk = match label.risk {
        OriginRiskV1::Low => 1.0,
        OriginRiskV1::Medium => 0.75,
        OriginRiskV1::High => 0.5,
        OriginRiskV1::Critical => 0.25,
    };
    let scope = match label.scopes.recall {
        AuthorityScopeV1::Universal => 1.0,
        AuthorityScopeV1::Audience => 0.8,
        AuthorityScopeV1::PrincipalOnly => 0.5,
        AuthorityScopeV1::Denied => 0.0,
    };
    risk * scope
}

fn state_relevance(status: PremiseStatus) -> f64 {
    state_relevance_for_status(status)
}

fn state_relevance_for_status(status: PremiseStatus) -> f64 {
    match status {
        PremiseStatus::Supported => 1.0,
        PremiseStatus::Unsupported => 0.35,
        PremiseStatus::Ambiguous => 0.15,
        PremiseStatus::Stale => 0.05,
        PremiseStatus::Contradicted => 0.0,
    }
}

fn normalized(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn digest<T: Serialize>(value: &T) -> Result<String, MemoryError> {
    let bytes = serde_json::to_vec(value).map_err(|error| {
        MemoryError::Other(format!("evidence digest serialization failed: {error}"))
    })?;
    Ok(format!("blake3:{}", blake3::hash(&bytes).to_hex()))
}

fn ids(results: &[SearchResult]) -> Vec<String> {
    results
        .iter()
        .map(|result| result.source.result_id())
        .collect()
}

fn state_status_for(result: &SearchResult, resolution: &DependencyResolutionV1) -> PremiseStatus {
    match resolution.status(&result.source.result_id()) {
        Some(DependencyState::Invalid) => PremiseStatus::Stale,
        Some(DependencyState::Uncertain) => PremiseStatus::Ambiguous,
        Some(DependencyState::Pending) => PremiseStatus::Unsupported,
        Some(DependencyState::Valid) | None => PremiseStatus::Supported,
    }
}

fn edge_from_stored(edge: &crate::StoredGraphEdge) -> Option<crate::StateDependencyEdgeV1> {
    let graph_type = edge
        .edge_type_parsed
        .clone()
        .or_else(|| serde_json::from_str(&edge.edge_type).ok())?;
    let GraphEdgeType::Entity { relation } = graph_type else {
        return None;
    };
    match relation.as_str() {
        "invalidates" => Some(crate::StateDependencyEdgeV1::invalidates(
            &edge.source,
            &edge.target,
        )),
        "weakens" => Some(crate::StateDependencyEdgeV1::weakens(
            &edge.source,
            &edge.target,
        )),
        "requires_reevaluation" => Some(crate::StateDependencyEdgeV1::requires_reevaluation(
            &edge.source,
            &edge.target,
        )),
        "scope_changes" => Some(crate::StateDependencyEdgeV1::scope_changes(
            &edge.source,
            &edge.target,
            "unknown",
            "unknown",
        )),
        "derived_from_state" => Some(crate::StateDependencyEdgeV1::derived_from_state(
            &edge.source,
            &edge.target,
        )),
        _ => None,
    }
}

fn required_satisfied(
    results: &[SearchResult],
    required: &[String],
    retrieval_queries: &[String],
) -> bool {
    required.iter().all(|requirement| {
        let needle = normalized(requirement);
        results.iter().any(|result| {
            normalized(&result.content).contains(&needle)
                && retrieval_queries
                    .iter()
                    .any(|query| normalized(query).contains(&needle))
        })
    })
}

fn evidence_sufficient(
    results: &[SearchResult],
    required: &[String],
    retrieval_queries: &[String],
    resolution: &DependencyResolutionV1,
) -> bool {
    !results.is_empty()
        && required_satisfied(results, required, retrieval_queries)
        && results.iter().any(|result| {
            matches!(
                state_status_for(result, resolution),
                PremiseStatus::Supported
            )
        })
}

fn diagnose_gaps(
    results: &[SearchResult],
    required: &[String],
    retrieval_queries: &[String],
    resolution: &DependencyResolutionV1,
    origin_denied: bool,
    query: &str,
) -> Vec<EvidenceGapV1> {
    let mut gaps = Vec::new();
    let missing = required
        .iter()
        .filter(|requirement| {
            !results.iter().any(|result| {
                normalized(&result.content).contains(&normalized(requirement))
                    && retrieval_queries
                        .iter()
                        .any(|q| normalized(q).contains(&normalized(requirement)))
            })
        })
        .cloned()
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        gaps.push(EvidenceGapV1 {
            schema_version: EVIDENCE_GAP_V1.into(),
            gap_id: format!(
                "gap:{}",
                blake3::hash(missing.join("\n").as_bytes()).to_hex()
            ),
            reason: EvidenceGapReasonV1::MissingRequiredEvidence,
            requirement: "required evidence terms".into(),
            missing_items: missing.clone(),
            diagnosed_by: vec!["content_and_route_coverage".into()],
            follow_up_queries: missing.clone(),
        });
    }
    if origin_denied {
        gaps.push(EvidenceGapV1 {
            schema_version: EVIDENCE_GAP_V1.into(),
            gap_id: "gap:origin-authority".into(),
            reason: EvidenceGapReasonV1::MissingOriginAuthority,
            requirement: "origin-authorized recall".into(),
            missing_items: vec!["origin authority denied or absent".into()],
            diagnosed_by: vec!["governed_direct_id_or_search".into()],
            follow_up_queries: Vec::new(),
        });
    }
    if !results.is_empty()
        && results
            .iter()
            .all(|result| state_status_for(result, resolution) != PremiseStatus::Supported)
    {
        gaps.push(EvidenceGapV1 {
            schema_version: EVIDENCE_GAP_V1.into(),
            gap_id: "gap:state-support".into(),
            reason: EvidenceGapReasonV1::MissingStateSupport,
            requirement: "current state support".into(),
            missing_items: vec!["a supported current premise".into()],
            diagnosed_by: vec!["state_dependency_resolution".into()],
            follow_up_queries: vec![query.to_string()],
        });
    }
    if results.is_empty() && !origin_denied && missing.is_empty() {
        gaps.push(EvidenceGapV1 {
            schema_version: EVIDENCE_GAP_V1.into(),
            gap_id: "gap:no-candidates".into(),
            reason: EvidenceGapReasonV1::NoCandidates,
            requirement: "retrievable evidence".into(),
            missing_items: vec![query.to_string()],
            diagnosed_by: vec!["hybrid_search_empty".into()],
            follow_up_queries: vec![query.to_string()],
        });
    }
    gaps
}

fn resolution_status(
    resolution: &DependencyResolutionV1,
    results: &[SearchResult],
) -> PremiseStatus {
    if resolution.invalid_lineage || resolution.unresolved_conflict {
        PremiseStatus::Ambiguous
    } else if results.is_empty() {
        PremiseStatus::Unsupported
    } else if results
        .iter()
        .any(|result| state_status_for(result, resolution) == PremiseStatus::Supported)
    {
        PremiseStatus::Supported
    } else if results
        .iter()
        .all(|result| state_status_for(result, resolution) == PremiseStatus::Stale)
    {
        PremiseStatus::Stale
    } else {
        PremiseStatus::Unsupported
    }
}

fn packet_digest_input(items: &[EvidencePacketItemV1]) -> Vec<serde_json::Value> {
    items
        .iter()
        .map(|item| {
            serde_json::json!({
                "id": item.result.source.result_id(),
                "original_score": item.original_score,
                "reranked_score": item.reranked_score,
                "state_status": item.state_status,
                "state_relevance": item.state_relevance,
                "support_score": item.support_score,
                "origin_authority_score": item.origin_authority_score,
            })
        })
        .collect()
}

impl MemoryStore {
    /// Retrieve a bounded evidence packet. Every terminal result includes both a state-resolution
    /// receipt and a witnessed retrieval envelope at answer level.
    pub async fn retrieve_evidence(
        &self,
        request: EvidenceGapRequestV1,
    ) -> Result<EvidencePacketV1, MemoryError> {
        let state_view = request.state_mode.state_view();
        let namespaces = request
            .namespaces
            .as_ref()
            .map(|values| values.iter().map(String::as_str).collect::<Vec<_>>());
        let namespace_refs = namespaces.as_deref();
        let mut routes = Vec::new();
        let mut consumed = 0usize;
        let mut origin_denied = false;
        let mut retrieval_queries = vec![request.query.clone()];
        let mut results = Vec::new();

        if request.budget > 0 {
            consumed += 1;
            let initial = if matches!(state_view, StateView::Current) {
                let mut context = SearchContext::default_now();
                context.request_id = Some(request.request_id.clone());
                context.receipt_mode = crate::ReceiptMode::ReturnReceipt;
                self.search_with_context(
                    &request.query,
                    Some(request.top_k),
                    namespace_refs,
                    None,
                    context,
                )
                .await?
                .results
            } else {
                self.search_with_view(
                    &request.query,
                    Some(request.top_k),
                    namespace_refs,
                    None,
                    state_view.clone(),
                )
                .await?
            };
            let (admitted, denied) = self
                .admit_results(initial, request.access_request.as_ref())
                .await?;
            origin_denied |= denied;
            results.extend(admitted);
            routes.push(EvidenceRouteReceiptV1 {
                schema_version: EVIDENCE_ROUTE_RECEIPT_V1.into(),
                route: EvidenceRetrievalRouteV1::InitialHybrid,
                query: Some(request.query.clone()),
                stage_outcome: StageOutcomeV1::Applied,
                result_ids: ids(&results),
                order_changed: false,
                budget_unit: consumed,
                note: None,
            });
        }

        let (baseline_resolution, baseline_edges) = self
            .resolve_evidence_state(&results, request.budget)
            .await?;
        let baseline_gaps = diagnose_gaps(
            &results,
            &request.required_evidence,
            &retrieval_queries,
            &baseline_resolution,
            origin_denied,
            &request.query,
        );
        let baseline_invalid = baseline_resolution.invalid_lineage;
        let baseline_conflict = baseline_resolution.unresolved_conflict;
        let baseline_sufficient = request.budget > 0
            && !baseline_invalid
            && !baseline_conflict
            && evidence_sufficient(
                &results,
                &request.required_evidence,
                &retrieval_queries,
                &baseline_resolution,
            );
        let baseline_outcome = if request.budget == 0 {
            EvidenceTerminalOutcomeV1::BudgetExceeded
        } else if baseline_invalid {
            EvidenceTerminalOutcomeV1::InvalidLineage
        } else if baseline_conflict {
            EvidenceTerminalOutcomeV1::Conflict
        } else if baseline_sufficient {
            EvidenceTerminalOutcomeV1::Sufficient
        } else {
            EvidenceTerminalOutcomeV1::EvidenceInsufficient
        };
        let baseline_items = self
            .build_rerank_candidates(
                &results,
                &baseline_resolution,
                request.access_request.as_ref(),
            )
            .await?;
        let baseline_packet_digest = digest(&packet_digest_input(
            &baseline_items
                .iter()
                .cloned()
                .map(EvidencePacketItemV1::from)
                .collect::<Vec<_>>(),
        ))?;
        let baseline_answer = baseline_items
            .first()
            .filter(|_| baseline_sufficient)
            .map(|candidate| candidate.result.content.clone());

        let mut gaps = baseline_gaps;
        let mut final_outcome = baseline_outcome;
        if request.budget > 0 && !baseline_invalid && !baseline_conflict && !baseline_sufficient {
            let follow_limit = request.follow_up_limit.min(4);
            let follow_queries = gaps
                .iter()
                .flat_map(|gap| gap.follow_up_queries.iter().cloned())
                .filter(|query| !query.trim().is_empty())
                .collect::<Vec<_>>();
            for query in follow_queries.into_iter().take(follow_limit) {
                if consumed >= request.budget {
                    break;
                }
                consumed += 1;
                retrieval_queries.push(query.clone());
                let before = ids(&results);
                let found = if matches!(state_view, StateView::Current) {
                    self.search_fts_only(
                        &query,
                        Some(request.top_k),
                        namespace_refs,
                        Some(&[SearchSourceType::Facts, SearchSourceType::Chunks]),
                    )
                    .await?
                } else {
                    self.search_with_view(
                        &query,
                        Some(request.top_k),
                        namespace_refs,
                        None,
                        state_view.clone(),
                    )
                    .await?
                };
                let (admitted, denied) = self
                    .admit_results(found, request.access_request.as_ref())
                    .await?;
                origin_denied |= denied;
                merge_results(&mut results, admitted);
                let after = ids(&results);
                routes.push(EvidenceRouteReceiptV1 {
                    schema_version: EVIDENCE_ROUTE_RECEIPT_V1.into(),
                    route: EvidenceRetrievalRouteV1::Lexical,
                    query: Some(query.clone()),
                    stage_outcome: StageOutcomeV1::Applied,
                    result_ids: after.clone(),
                    order_changed: before != after,
                    budget_unit: consumed,
                    note: Some("required evidence follow-up".into()),
                });
                let (resolution, _) = self
                    .resolve_evidence_state(&results, request.budget)
                    .await?;
                if evidence_sufficient(
                    &results,
                    &request.required_evidence,
                    &retrieval_queries,
                    &resolution,
                ) {
                    final_outcome = EvidenceTerminalOutcomeV1::Sufficient;
                    break;
                }
            }
        }

        // A bounded vector follow-up is useful when lexical evidence is absent. It is deliberately
        // separate from the lexical route so the receipt proves which lane changed the packet.
        if request.budget > consumed
            && !matches!(
                final_outcome,
                EvidenceTerminalOutcomeV1::Sufficient
                    | EvidenceTerminalOutcomeV1::Conflict
                    | EvidenceTerminalOutcomeV1::InvalidLineage
            )
            && request.follow_up_limit >= 2
        {
            consumed += 1;
            let before = ids(&results);
            let found = if matches!(state_view, StateView::Current) {
                let mut context = SearchContext::default_now();
                context.request_id = Some(format!("{}:vector", request.request_id));
                context.receipt_mode = crate::ReceiptMode::ReturnReceipt;
                self.search_vector_only_with_context(
                    &request.query,
                    Some(request.top_k),
                    namespace_refs,
                    None,
                    context,
                )
                .await?
                .results
            } else {
                Vec::new()
            };
            let (admitted, denied) = self
                .admit_results(found, request.access_request.as_ref())
                .await?;
            origin_denied |= denied;
            merge_results(&mut results, admitted);
            let after = ids(&results);
            routes.push(EvidenceRouteReceiptV1 {
                schema_version: EVIDENCE_ROUTE_RECEIPT_V1.into(),
                route: EvidenceRetrievalRouteV1::Vector,
                query: Some(request.query.clone()),
                stage_outcome: StageOutcomeV1::Applied,
                result_ids: after.clone(),
                order_changed: before != after,
                budget_unit: consumed,
                note: Some("bounded vector follow-up".into()),
            });
        }

        // Graph expansion is bounded by both the caller budget and the graph API node cap.
        if request.budget > consumed
            && !matches!(
                final_outcome,
                EvidenceTerminalOutcomeV1::Conflict | EvidenceTerminalOutcomeV1::InvalidLineage
            )
            && request.follow_up_limit >= 3
        {
            consumed += 1;
            let before = ids(&results);
            let graph_results = self
                .graph_follow_up(&results, request.access_request.as_ref(), request.top_k)
                .await?;
            let (admitted, denied) = self
                .admit_results(graph_results, request.access_request.as_ref())
                .await?;
            origin_denied |= denied;
            merge_results(&mut results, admitted);
            let after = ids(&results);
            routes.push(EvidenceRouteReceiptV1 {
                schema_version: EVIDENCE_ROUTE_RECEIPT_V1.into(),
                route: EvidenceRetrievalRouteV1::Graph,
                query: None,
                stage_outcome: StageOutcomeV1::Applied,
                result_ids: after.clone(),
                order_changed: before != after,
                budget_unit: consumed,
                note: Some("bounded state-neighborhood expansion".into()),
            });
        }

        if request.budget > consumed
            && !matches!(
                final_outcome,
                EvidenceTerminalOutcomeV1::Conflict | EvidenceTerminalOutcomeV1::InvalidLineage
            )
            && request.follow_up_limit >= 4
        {
            consumed += 1;
            let before = ids(&results);
            let found = if matches!(state_view, StateView::Current) {
                self.search_with_view(
                    &request.query,
                    Some(request.top_k),
                    namespace_refs,
                    Some(&[SearchSourceType::Episodes]),
                    state_view.clone(),
                )
                .await?
            } else {
                self.search_with_view(
                    &request.query,
                    Some(request.top_k),
                    namespace_refs,
                    Some(&[SearchSourceType::Episodes]),
                    state_view.clone(),
                )
                .await?
            };
            let (admitted, denied) = self
                .admit_results(found, request.access_request.as_ref())
                .await?;
            origin_denied |= denied;
            merge_results(&mut results, admitted);
            let after = ids(&results);
            routes.push(EvidenceRouteReceiptV1 {
                schema_version: EVIDENCE_ROUTE_RECEIPT_V1.into(),
                route: EvidenceRetrievalRouteV1::Episodic,
                query: Some(request.query.clone()),
                stage_outcome: StageOutcomeV1::Applied,
                result_ids: after.clone(),
                order_changed: before != after,
                budget_unit: consumed,
                note: Some("bounded episodic follow-up".into()),
            });
        }

        if !request.direct_ids.is_empty() {
            let direct = self
                .direct_id_results(&request.direct_ids, request.access_request.as_ref())
                .await?;
            if direct.1 {
                origin_denied = true;
            }
            merge_results(&mut results, direct.0);
            routes.push(EvidenceRouteReceiptV1 {
                schema_version: EVIDENCE_ROUTE_RECEIPT_V1.into(),
                route: EvidenceRetrievalRouteV1::DirectId,
                query: None,
                stage_outcome: if origin_denied {
                    StageOutcomeV1::Degraded
                } else {
                    StageOutcomeV1::Applied
                },
                result_ids: ids(&results),
                order_changed: false,
                budget_unit: 0,
                note: Some("direct IDs remain subject to governed origin admission".into()),
            });
        }

        let (resolution, edges) = self
            .resolve_evidence_state(&results, request.budget)
            .await?;
        let state_status = resolution_status(&resolution, &results);
        let sufficient = request.budget > 0
            && !resolution.invalid_lineage
            && !resolution.unresolved_conflict
            && evidence_sufficient(
                &results,
                &request.required_evidence,
                &retrieval_queries,
                &resolution,
            );
        final_outcome = if resolution.invalid_lineage {
            EvidenceTerminalOutcomeV1::InvalidLineage
        } else if resolution.unresolved_conflict {
            EvidenceTerminalOutcomeV1::Conflict
        } else if request.budget == 0
            || (consumed >= request.budget && !sufficient && !results.is_empty())
        {
            EvidenceTerminalOutcomeV1::BudgetExceeded
        } else if sufficient {
            EvidenceTerminalOutcomeV1::Sufficient
        } else {
            EvidenceTerminalOutcomeV1::EvidenceInsufficient
        };
        gaps = diagnose_gaps(
            &results,
            &request.required_evidence,
            &retrieval_queries,
            &resolution,
            origin_denied,
            &request.query,
        );

        let mut candidates = self
            .build_rerank_candidates(&results, &resolution, request.access_request.as_ref())
            .await?;
        candidates = rerank_state_aware(candidates, request.rerank_weights.clone());
        candidates.truncate(request.top_k);
        let items = candidates
            .into_iter()
            .map(EvidencePacketItemV1::from)
            .collect::<Vec<_>>();
        let final_packet_digest = digest(&packet_digest_input(&items))?;
        let answer_policy = answer_policy_for(
            state_status,
            sufficient,
            matches!(final_outcome, EvidenceTerminalOutcomeV1::Conflict),
            matches!(final_outcome, EvidenceTerminalOutcomeV1::InvalidLineage),
            matches!(final_outcome, EvidenceTerminalOutcomeV1::BudgetExceeded),
        );
        let answer = if matches!(final_outcome, EvidenceTerminalOutcomeV1::Sufficient) {
            items.first().map(|item| item.result.content.clone())
        } else {
            None
        };
        let baseline_ids = baseline_items
            .iter()
            .map(|candidate| candidate.result.source.result_id())
            .collect::<Vec<_>>();
        let final_ids = items
            .iter()
            .map(|item| item.result.source.result_id())
            .collect::<Vec<_>>();
        let baseline_digest = baseline_packet_digest.clone();
        let packet_changed = baseline_digest != final_packet_digest;
        let order_changed = baseline_ids != final_ids;
        let answer_changed = baseline_answer != answer;
        let improved = matches!(final_outcome, EvidenceTerminalOutcomeV1::Sufficient)
            && !matches!(baseline_outcome, EvidenceTerminalOutcomeV1::Sufficient)
            && (packet_changed || answer_changed);
        let ablation = EvidenceAblationReceiptV1 {
            schema_version: EVIDENCE_PACKET_V1.into(),
            baseline_result_ids: baseline_ids,
            gap_loop_result_ids: final_ids,
            packet_digest_without_gap_loop: baseline_digest,
            packet_digest_with_gap_loop: final_packet_digest.clone(),
            answer_without_gap_loop: baseline_answer,
            answer_with_gap_loop: answer.clone(),
            baseline_outcome,
            final_outcome,
            packet_changed,
            order_changed,
            answer_changed,
            improved,
        };

        let resolved_ids = items
            .iter()
            .map(|item| item.result.source.result_id())
            .collect::<Vec<_>>();
        let alternative_ids = resolved_ids.iter().skip(1).cloned().collect::<Vec<_>>();
        let mut state_receipt = StateResolutionReceiptV1::new(
            request.request_id.clone(),
            state_view.clone(),
            request.state_mode.clone(),
            state_status,
            answer_policy.disposition,
            resolved_ids,
            alternative_ids,
            sufficient,
            matches!(final_outcome, EvidenceTerminalOutcomeV1::Conflict),
            matches!(final_outcome, EvidenceTerminalOutcomeV1::InvalidLineage),
            matches!(final_outcome, EvidenceTerminalOutcomeV1::BudgetExceeded),
        )?;
        state_receipt.stage_outcomes.push((
            "evidence_gap".into(),
            if matches!(final_outcome, EvidenceTerminalOutcomeV1::Sufficient) {
                StageOutcomeV1::Applied
            } else if matches!(final_outcome, EvidenceTerminalOutcomeV1::BudgetExceeded) {
                StageOutcomeV1::BudgetExceeded
            } else {
                StageOutcomeV1::Degraded
            },
        ));
        state_receipt = state_receipt.with_dependency_edge_digests(
            edges
                .iter()
                .filter_map(edge_from_stored)
                .map(|edge| edge.digest())
                .collect(),
        )?;
        let witness = crate::state_epistemics::witnessed_retrieval(
            self,
            request.request_id.clone(),
            &request.query,
            &items
                .iter()
                .map(|item| item.result.clone())
                .collect::<Vec<_>>(),
        )
        .await?;
        let packet_id = format!("evidence-packet:{final_packet_digest}");
        Ok(EvidencePacketV1 {
            schema_version: EVIDENCE_PACKET_V1.into(),
            packet_id,
            request_id: request.request_id,
            query: request.query,
            state_view,
            outcome: final_outcome,
            items,
            gaps,
            routes,
            answer,
            answer_policy,
            state_resolution_receipt: state_receipt,
            retrieval_witness: witness,
            ablation,
            packet_digest: final_packet_digest,
        })
    }

    pub async fn retrieve_evidence_gap(
        &self,
        request: EvidenceGapRequestV1,
    ) -> Result<EvidencePacketV1, MemoryError> {
        self.retrieve_evidence(request).await
    }

    async fn admit_results(
        &self,
        results: Vec<SearchResult>,
        access: Option<&GovernedAccessRequestV1>,
    ) -> Result<(Vec<SearchResult>, bool), MemoryError> {
        let Some(request) = access else {
            // Evidence packets are governed public output. Missing access context is a denial,
            // not a legacy/raw fallback: otherwise a cache or follow-up route could disclose
            // content that direct governed retrieval correctly withholds.
            return Ok((Vec::new(), !results.is_empty()));
        };
        let mut admitted = Vec::new();
        let mut denied = false;
        for result in results {
            let allowed = match &result.source {
                SearchSource::Fact { fact_id, .. } => {
                    self.authority()
                        .get_fact_governed(fact_id, request.clone())
                        .await?
                        .decision
                        .allowed
                }
                _ => false,
            };
            if allowed {
                admitted.push(result);
            } else {
                denied = true;
            }
        }
        Ok((admitted, denied))
    }

    async fn direct_id_results(
        &self,
        direct_ids: &[String],
        access: Option<&GovernedAccessRequestV1>,
    ) -> Result<(Vec<SearchResult>, bool), MemoryError> {
        let mut results = Vec::new();
        let mut denied = false;
        for raw_id in direct_ids {
            let fact_id = raw_id.strip_prefix("fact:").unwrap_or(raw_id);
            let fact = if let Some(request) = access {
                let governed = self
                    .authority()
                    .get_fact_governed(fact_id, request.clone())
                    .await?;
                if !governed.decision.allowed {
                    denied = true;
                    None
                } else {
                    governed.fact
                }
            } else {
                denied = true;
                None
            };
            if let Some(fact) = fact {
                results.push(SearchResult {
                    content: fact.content,
                    source: SearchSource::Fact {
                        fact_id: fact.id,
                        namespace: fact.namespace,
                    },
                    score: 0.0,
                    bm25_rank: None,
                    vector_rank: None,
                    cosine_similarity: None,
                });
            }
        }
        Ok((results, denied))
    }

    async fn graph_follow_up(
        &self,
        results: &[SearchResult],
        access: Option<&GovernedAccessRequestV1>,
        top_k: usize,
    ) -> Result<Vec<SearchResult>, MemoryError> {
        let seeds = results
            .iter()
            .filter_map(|result| match &result.source {
                SearchSource::Fact { fact_id, .. } => Some(format!("fact:{fact_id}")),
                _ => None,
            })
            .collect::<Vec<_>>();
        if seeds.is_empty() {
            return Ok(Vec::new());
        }
        let edges = self
            .list_graph_edges_for_neighborhood(seeds, 1, top_k.clamp(1, 32))
            .await?;
        let mut ids = BTreeSet::new();
        for edge in edges {
            for endpoint in [&edge.source, &edge.target] {
                if endpoint.starts_with("fact:") {
                    ids.insert(endpoint.trim_start_matches("fact:").to_string());
                }
            }
        }
        let (facts, _) = self
            .direct_id_results(
                &ids.into_iter()
                    .map(|id| format!("fact:{id}"))
                    .collect::<Vec<_>>(),
                access,
            )
            .await?;
        Ok(facts)
    }

    async fn resolve_evidence_state(
        &self,
        results: &[SearchResult],
        budget: usize,
    ) -> Result<(DependencyResolutionV1, Vec<crate::StoredGraphEdge>), MemoryError> {
        let roots = results
            .iter()
            .filter_map(|result| match &result.source {
                SearchSource::Fact { fact_id, .. } => Some(format!("fact:{fact_id}")),
                _ => None,
            })
            .collect::<Vec<_>>();
        if roots.is_empty() {
            return Ok((
                resolve_dependency_states(&[], &[], budget.max(1)),
                Vec::new(),
            ));
        }
        let edges = self
            .list_graph_edges_for_neighborhood(roots.clone(), 2, budget.clamp(1, 128))
            .await?;
        let typed = edges
            .iter()
            .filter_map(edge_from_stored)
            .collect::<Vec<_>>();
        Ok((
            resolve_dependency_states(&typed, &roots, budget.max(1)),
            edges,
        ))
    }

    async fn build_rerank_candidates(
        &self,
        results: &[SearchResult],
        resolution: &DependencyResolutionV1,
        access: Option<&GovernedAccessRequestV1>,
    ) -> Result<Vec<StateRerankCandidateV1>, MemoryError> {
        let mut candidates = Vec::new();
        for result in results {
            let status = state_status_for(result, resolution);
            let origin = match (&result.source, access) {
                (SearchSource::Fact { fact_id, .. }, _) => {
                    self.authority().get_origin_authority(fact_id).await?
                }
                _ => None,
            };
            let candidate = StateRerankCandidateV1::from_result(
                result.clone(),
                status,
                state_relevance(status),
            )
            .with_origin(origin.as_ref());
            candidates.push(candidate);
        }
        Ok(candidates)
    }
}

fn merge_results(results: &mut Vec<SearchResult>, additions: Vec<SearchResult>) {
    let mut seen = results
        .iter()
        .map(|result| result.source.result_id())
        .collect::<BTreeSet<_>>();
    for result in additions {
        if seen.insert(result.source.result_id()) {
            results.push(result);
        }
    }
}
