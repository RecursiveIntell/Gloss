//! Deterministic state-resolution and answer-disposition contracts.
//!
//! This module interprets the existing authoritative facts and graph edges. It
//! does not maintain a second memory store: dependency edges are encoded in
//! the existing `graph_edges` table and resolution reads the existing search
//! and state-view APIs.

use crate::authority_contracts::{
    AuthoritySnapshotId, RetrievalEpoch, RetrievalWitnessV1, StageOutcomeV1,
};
use crate::knowledge::StateView;
use crate::types::{GraphEdgeType, SearchResult, SearchSource};
use crate::{MemoryError, MemoryStore};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const STATE_RESOLUTION_RECEIPT_V1: &str = "state_resolution_receipt_v1";
pub const STATE_RESOLVED_RETRIEVAL_V1: &str = "state_resolved_retrieval_v1";

/// The resolution request, kept separate from the compatible `StateView` wire enum.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateResolutionMode {
    Current,
    HistoricalAt(String),
    Transition { from: String, to: String },
    Trajectory { points: Vec<String> },
}

impl StateResolutionMode {
    pub fn historical_at(as_of: impl Into<String>) -> Self {
        Self::HistoricalAt(as_of.into())
    }

    pub fn transition(from: impl Into<String>, to: impl Into<String>) -> Self {
        Self::Transition {
            from: from.into(),
            to: to.into(),
        }
    }

    pub fn trajectory(points: Vec<String>) -> Self {
        Self::Trajectory { points }
    }

    /// Map a resolution request to the existing authoritative fact view.
    pub fn state_view(&self) -> StateView {
        match self {
            Self::Current => StateView::Current,
            Self::HistoricalAt(as_of) => StateView::HistoricalAt(as_of.clone()),
            Self::Transition { .. } | Self::Trajectory { .. } => StateView::IncludeSuperseded,
        }
    }
}

/// Explicit epistemic classification of a premise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PremiseStatus {
    Supported,
    Stale,
    Contradicted,
    Unsupported,
    Ambiguous,
}

/// The only answer dispositions emitted by state resolution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AnswerDisposition {
    Answer,
    CorrectPremise,
    DiscloseConflict,
    Abstain,
    RequestEvidence,
}

/// Compatibility name used by policy callers.
pub type AnswerPolicy = AnswerDisposition;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DependencyState {
    Valid,
    Invalid,
    Uncertain,
    Pending,
}

/// Typed state-dependency relation stored through the existing graph API.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum StateDependencyEdgeV1 {
    Invalidates {
        source_fact_id: String,
        target_fact_id: String,
    },
    Weakens {
        source_fact_id: String,
        target_fact_id: String,
    },
    RequiresReevaluation {
        source_fact_id: String,
        target_fact_id: String,
    },
    ScopeChanges {
        source_fact_id: String,
        target_fact_id: String,
        from_scope: String,
        to_scope: String,
    },
    DerivedFromState {
        source_fact_id: String,
        target_fact_id: String,
    },
}

impl StateDependencyEdgeV1 {
    pub fn invalidates(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self::Invalidates {
            source_fact_id: source.into(),
            target_fact_id: target.into(),
        }
    }

    pub fn weakens(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self::Weakens {
            source_fact_id: source.into(),
            target_fact_id: target.into(),
        }
    }

    pub fn requires_reevaluation(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self::RequiresReevaluation {
            source_fact_id: source.into(),
            target_fact_id: target.into(),
        }
    }

    pub fn scope_changes(
        source: impl Into<String>,
        target: impl Into<String>,
        from_scope: impl Into<String>,
        to_scope: impl Into<String>,
    ) -> Self {
        Self::ScopeChanges {
            source_fact_id: source.into(),
            target_fact_id: target.into(),
            from_scope: from_scope.into(),
            to_scope: to_scope.into(),
        }
    }

    pub fn derived_from_state(source: impl Into<String>, target: impl Into<String>) -> Self {
        Self::DerivedFromState {
            source_fact_id: source.into(),
            target_fact_id: target.into(),
        }
    }

    pub fn source(&self) -> &str {
        match self {
            Self::Invalidates { source_fact_id, .. }
            | Self::Weakens { source_fact_id, .. }
            | Self::RequiresReevaluation { source_fact_id, .. }
            | Self::ScopeChanges { source_fact_id, .. }
            | Self::DerivedFromState { source_fact_id, .. } => source_fact_id,
        }
    }

    pub fn target(&self) -> &str {
        match self {
            Self::Invalidates { target_fact_id, .. }
            | Self::Weakens { target_fact_id, .. }
            | Self::RequiresReevaluation { target_fact_id, .. }
            | Self::ScopeChanges { target_fact_id, .. }
            | Self::DerivedFromState { target_fact_id, .. } => target_fact_id,
        }
    }

    pub fn relation(&self) -> &'static str {
        match self {
            Self::Invalidates { .. } => "invalidates",
            Self::Weakens { .. } => "weakens",
            Self::RequiresReevaluation { .. } => "requires_reevaluation",
            Self::ScopeChanges { .. } => "scope_changes",
            Self::DerivedFromState { .. } => "derived_from_state",
        }
    }

    pub fn digest(&self) -> String {
        blake3::hash(&serde_json::to_vec(self).expect("state edge is serializable"))
            .to_hex()
            .to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyResolutionV1 {
    pub states: BTreeMap<String, DependencyState>,
    pub premise_status: PremiseStatus,
    pub invalid_lineage: bool,
    pub unresolved_conflict: bool,
    pub budget_exhausted: bool,
    pub visited_nodes: Vec<String>,
}

impl DependencyResolutionV1 {
    pub fn status(&self, id: &str) -> Option<DependencyState> {
        self.states.get(id).copied()
    }
}

fn set_state(states: &mut BTreeMap<String, DependencyState>, id: &str, next: DependencyState) {
    let rank = |state: DependencyState| match state {
        DependencyState::Valid => 0,
        DependencyState::Pending => 1,
        DependencyState::Uncertain => 2,
        DependencyState::Invalid => 3,
    };
    let replace = states
        .get(id)
        .map_or(true, |current| rank(next) > rank(*current));
    if replace {
        states.insert(id.to_string(), next);
    }
}

fn has_cycle(edges: &[StateDependencyEdgeV1]) -> bool {
    let mut adjacency: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for edge in edges {
        adjacency
            .entry(edge.source())
            .or_default()
            .push(edge.target());
    }
    fn visit<'a>(
        node: &'a str,
        adjacency: &BTreeMap<&'a str, Vec<&'a str>>,
        visiting: &mut BTreeSet<&'a str>,
        visited: &mut BTreeSet<&'a str>,
    ) -> bool {
        if visiting.contains(node) {
            return true;
        }
        if visited.contains(node) {
            return false;
        }
        visiting.insert(node);
        if let Some(next) = adjacency.get(node) {
            if next
                .iter()
                .any(|child| visit(child, adjacency, visiting, visited))
            {
                return true;
            }
        }
        visiting.remove(node);
        visited.insert(node);
        false
    }
    let mut visiting = BTreeSet::new();
    let mut visited = BTreeSet::new();
    adjacency
        .keys()
        .any(|node| visit(node, &adjacency, &mut visiting, &mut visited))
}

/// Resolve the existing dependency graph with a deterministic node budget.
pub fn resolve_dependency_states(
    edges: &[StateDependencyEdgeV1],
    roots: &[String],
    budget: usize,
) -> DependencyResolutionV1 {
    let mut states = BTreeMap::new();
    let mut visited = BTreeSet::new();
    for root in roots {
        if !root.trim().is_empty() {
            states.insert(root.clone(), DependencyState::Valid);
        }
    }
    let mut invalid_lineage = has_cycle(edges);
    let mut target_invalidators: BTreeMap<&str, usize> = BTreeMap::new();
    for edge in edges {
        if edge.source().trim().is_empty() || edge.target().trim().is_empty() {
            invalid_lineage = true;
        }
        if matches!(edge, StateDependencyEdgeV1::Invalidates { .. }) {
            *target_invalidators.entry(edge.target()).or_default() += 1;
        }
    }
    let unresolved_conflict = target_invalidators.values().any(|count| *count > 1);
    invalid_lineage |= unresolved_conflict;

    if budget == 0 && !states.is_empty() {
        return DependencyResolutionV1 {
            states: states
                .into_keys()
                .map(|id| (id, DependencyState::Pending))
                .collect(),
            premise_status: PremiseStatus::Unsupported,
            invalid_lineage,
            unresolved_conflict,
            budget_exhausted: true,
            visited_nodes: Vec::new(),
        };
    }

    let mut changed = true;
    while changed {
        changed = false;
        for edge in edges {
            if visited.len() >= budget && budget > 0 {
                break;
            }
            let source_state = states.get(edge.source()).copied();
            let target_state = states.get(edge.target()).copied();
            if source_state.is_some() || target_state.is_some() {
                visited.insert(edge.source().to_string());
                visited.insert(edge.target().to_string());
            }
            let before = states.clone();
            match edge {
                StateDependencyEdgeV1::Invalidates { .. } => {
                    // An invalidation is authoritative for the target head even
                    // when the invalidating successor is outside the requested
                    // root set (the common stale-premise shape).
                    if source_state.is_some() || target_state.is_some() {
                        set_state(&mut states, edge.target(), DependencyState::Invalid);
                    }
                }
                StateDependencyEdgeV1::Weakens { .. }
                | StateDependencyEdgeV1::RequiresReevaluation { .. }
                | StateDependencyEdgeV1::ScopeChanges { .. } => {
                    if source_state.is_some() || target_state.is_some() {
                        set_state(&mut states, edge.target(), DependencyState::Uncertain);
                    }
                }
                StateDependencyEdgeV1::DerivedFromState { .. } => {
                    if let Some(state) = target_state {
                        set_state(&mut states, edge.source(), state);
                    }
                }
            }
            changed |= before != states;
        }
        if budget > 0 && visited.len() >= budget {
            break;
        }
    }
    let budget_exhausted = budget > 0 && (visited.len() >= budget && edges.len() > visited.len());
    let aggregate = if invalid_lineage || unresolved_conflict {
        PremiseStatus::Ambiguous
    } else if states.values().any(|s| *s == DependencyState::Invalid) {
        // An invalidation/supersession makes the addressed premise stale. A
        // competing live head is represented separately as ambiguity/conflict.
        PremiseStatus::Stale
    } else if states.values().any(|s| *s == DependencyState::Uncertain) {
        PremiseStatus::Ambiguous
    } else if states.values().any(|s| *s == DependencyState::Pending) || budget_exhausted {
        PremiseStatus::Unsupported
    } else if states.is_empty() {
        PremiseStatus::Unsupported
    } else {
        PremiseStatus::Supported
    };
    DependencyResolutionV1 {
        states,
        premise_status: aggregate,
        invalid_lineage,
        unresolved_conflict,
        budget_exhausted,
        visited_nodes: visited.into_iter().collect(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerPolicyDecision {
    pub disposition: AnswerDisposition,
    pub confident_negative: bool,
}

/// Map premise and evidence conditions to a safe explicit disposition.
pub fn answer_policy_for(
    premise_status: PremiseStatus,
    evidence_sufficient: bool,
    unresolved_conflict: bool,
    invalid_lineage: bool,
    budget_exhausted: bool,
) -> AnswerPolicyDecision {
    let disposition =
        if invalid_lineage || unresolved_conflict || premise_status == PremiseStatus::Ambiguous {
            AnswerDisposition::DiscloseConflict
        } else if !evidence_sufficient
            || premise_status == PremiseStatus::Unsupported
            || budget_exhausted
        {
            AnswerDisposition::RequestEvidence
        } else {
            match premise_status {
                PremiseStatus::Supported => AnswerDisposition::Answer,
                PremiseStatus::Stale => AnswerDisposition::CorrectPremise,
                PremiseStatus::Contradicted => AnswerDisposition::DiscloseConflict,
                PremiseStatus::Unsupported | PremiseStatus::Ambiguous => {
                    AnswerDisposition::RequestEvidence
                }
            }
        };
    AnswerPolicyDecision {
        disposition,
        // This kernel has no confident-negative disposition. Missing evidence,
        // conflict, invalid lineage, and budget exhaustion therefore cannot be
        // represented as a confident negative answer.
        confident_negative: false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedAssertionV1 {
    pub schema_version: String,
    pub memory_id: String,
    pub content: String,
    pub premise_status: PremiseStatus,
    pub state_view: StateView,
    pub source_kind: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeliefAlternativeV1 {
    pub assertion: ResolvedAssertionV1,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateResolutionReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub request_id: String,
    pub state_view: StateView,
    pub resolution_mode: StateResolutionMode,
    pub premise_status: PremiseStatus,
    pub answer_disposition: AnswerDisposition,
    pub resolved_memory_ids: Vec<String>,
    pub alternative_memory_ids: Vec<String>,
    pub dependency_edge_digests: Vec<String>,
    pub evidence_sufficient: bool,
    pub unresolved_conflict: bool,
    pub invalid_lineage: bool,
    pub budget_exhausted: bool,
    pub evaluated_at: String,
    pub receipt_digest: String,
    pub stage_outcomes: Vec<(String, StageOutcomeV1)>,
    pub degradations: Vec<String>,
}

#[derive(Serialize)]
struct ReceiptDigestInput<'a> {
    schema_version: &'a str,
    request_id: &'a str,
    state_view: &'a StateView,
    resolution_mode: &'a StateResolutionMode,
    premise_status: PremiseStatus,
    answer_disposition: AnswerDisposition,
    resolved_memory_ids: &'a [String],
    alternative_memory_ids: &'a [String],
    dependency_edge_digests: &'a [String],
    evidence_sufficient: bool,
    unresolved_conflict: bool,
    invalid_lineage: bool,
    budget_exhausted: bool,
}

impl StateResolutionReceiptV1 {
    fn digest_for(&self) -> Result<String, MemoryError> {
        let input = ReceiptDigestInput {
            schema_version: &self.schema_version,
            request_id: &self.request_id,
            state_view: &self.state_view,
            resolution_mode: &self.resolution_mode,
            premise_status: self.premise_status,
            answer_disposition: self.answer_disposition,
            resolved_memory_ids: &self.resolved_memory_ids,
            alternative_memory_ids: &self.alternative_memory_ids,
            dependency_edge_digests: &self.dependency_edge_digests,
            evidence_sufficient: self.evidence_sufficient,
            unresolved_conflict: self.unresolved_conflict,
            invalid_lineage: self.invalid_lineage,
            budget_exhausted: self.budget_exhausted,
        };
        Ok(blake3::hash(
            &serde_json::to_vec(&input)
                .map_err(|e| MemoryError::Other(format!("receipt serialization failed: {e}")))?,
        )
        .to_hex()
        .to_string())
    }

    pub fn with_dependency_edge_digests(
        mut self,
        dependency_edge_digests: Vec<String>,
    ) -> Result<Self, MemoryError> {
        self.dependency_edge_digests = dependency_edge_digests;
        self.receipt_digest = self.digest_for()?;
        self.receipt_id = format!("state-resolution:{}", self.receipt_digest);
        Ok(self)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new_at(
        request_id: impl Into<String>,
        state_view: StateView,
        resolution_mode: StateResolutionMode,
        premise_status: PremiseStatus,
        answer_disposition: AnswerDisposition,
        resolved_memory_ids: Vec<String>,
        alternative_memory_ids: Vec<String>,
        evidence_sufficient: bool,
        unresolved_conflict: bool,
        invalid_lineage: bool,
        budget_exhausted: bool,
        evaluated_at: impl Into<String>,
    ) -> Result<Self, MemoryError> {
        let request_id = request_id.into();
        let evaluated_at = evaluated_at.into();
        if request_id.trim().is_empty() {
            return Err(MemoryError::Other(
                "state resolution request ID must not be empty".into(),
            ));
        }
        DateTime::parse_from_rfc3339(&evaluated_at).map_err(|e| {
            MemoryError::Other(format!(
                "invalid state resolution timestamp '{evaluated_at}': {e}"
            ))
        })?;
        validate_state_view(&state_view)?;
        let schema_version = STATE_RESOLUTION_RECEIPT_V1.to_string();
        let input = ReceiptDigestInput {
            schema_version: &schema_version,
            request_id: &request_id,
            state_view: &state_view,
            resolution_mode: &resolution_mode,
            premise_status,
            answer_disposition,
            resolved_memory_ids: &resolved_memory_ids,
            alternative_memory_ids: &alternative_memory_ids,
            dependency_edge_digests: &[],
            evidence_sufficient,
            unresolved_conflict,
            invalid_lineage,
            budget_exhausted,
        };
        let receipt_digest = blake3::hash(
            &serde_json::to_vec(&input)
                .map_err(|e| MemoryError::Other(format!("receipt serialization failed: {e}")))?,
        )
        .to_hex()
        .to_string();
        let receipt_id = format!("state-resolution:{receipt_digest}");
        Ok(Self {
            schema_version,
            receipt_id,
            request_id,
            state_view,
            resolution_mode,
            premise_status,
            answer_disposition,
            resolved_memory_ids,
            alternative_memory_ids,
            dependency_edge_digests: Vec::new(),
            evidence_sufficient,
            unresolved_conflict,
            invalid_lineage,
            budget_exhausted,
            evaluated_at,
            receipt_digest,
            stage_outcomes: vec![("state_resolution".into(), StageOutcomeV1::Applied)],
            degradations: Vec::new(),
        })
    }

    pub fn new(
        request_id: impl Into<String>,
        state_view: StateView,
        resolution_mode: StateResolutionMode,
        premise_status: PremiseStatus,
        answer_disposition: AnswerDisposition,
        resolved_memory_ids: Vec<String>,
        alternative_memory_ids: Vec<String>,
        evidence_sufficient: bool,
        unresolved_conflict: bool,
        invalid_lineage: bool,
        budget_exhausted: bool,
    ) -> Result<Self, MemoryError> {
        Self::new_at(
            request_id,
            state_view,
            resolution_mode,
            premise_status,
            answer_disposition,
            resolved_memory_ids,
            alternative_memory_ids,
            evidence_sufficient,
            unresolved_conflict,
            invalid_lineage,
            budget_exhausted,
            Utc::now().to_rfc3339(),
        )
    }
}

fn validate_state_view(view: &StateView) -> Result<(), MemoryError> {
    if let StateView::HistoricalAt(value) | StateView::RecordedAsOf(value) = view {
        DateTime::parse_from_rfc3339(value).map_err(|e| {
            MemoryError::Other(format!("invalid StateView timestamp '{value}': {e}"))
        })?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResolvedMemoryAnswerV1 {
    pub schema_version: String,
    pub query: String,
    pub answer: Option<String>,
    pub premise_status: PremiseStatus,
    pub answer_disposition: AnswerDisposition,
    pub state_view: StateView,
    pub assertions: Vec<ResolvedAssertionV1>,
    pub alternatives: Vec<BeliefAlternativeV1>,
    pub receipt: StateResolutionReceiptV1,
    pub retrieval_witness: RetrievalWitnessV1,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StateResolvedRetrievalResponseV1<T> {
    pub schema_version: String,
    pub state_view: StateView,
    pub results: Vec<T>,
    pub retrieval_witness: RetrievalWitnessV1,
    pub resolution_receipt: StateResolutionReceiptV1,
}

fn edge_from_stored(edge: &crate::StoredGraphEdge) -> Option<StateDependencyEdgeV1> {
    let graph_type = edge
        .edge_type_parsed
        .clone()
        .or_else(|| serde_json::from_str(&edge.edge_type).ok())?;
    let GraphEdgeType::Entity { relation } = graph_type else {
        return None;
    };
    match relation.as_str() {
        "invalidates" => Some(StateDependencyEdgeV1::invalidates(
            &edge.source,
            &edge.target,
        )),
        "weakens" => Some(StateDependencyEdgeV1::weakens(&edge.source, &edge.target)),
        "requires_reevaluation" => Some(StateDependencyEdgeV1::requires_reevaluation(
            &edge.source,
            &edge.target,
        )),
        "scope_changes" => {
            let metadata = edge
                .metadata
                .as_deref()
                .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok());
            Some(StateDependencyEdgeV1::scope_changes(
                &edge.source,
                &edge.target,
                metadata
                    .as_ref()
                    .and_then(|v| v.get("from_scope"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown"),
                metadata
                    .as_ref()
                    .and_then(|v| v.get("to_scope"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown"),
            ))
        }
        "derived_from_state" => Some(StateDependencyEdgeV1::derived_from_state(
            &edge.source,
            &edge.target,
        )),
        _ => None,
    }
}

fn result_id(result: &SearchResult) -> String {
    result.source.result_id()
}

fn result_status(result: &SearchResult, resolution: &DependencyResolutionV1) -> PremiseStatus {
    let id = match &result.source {
        SearchSource::Fact { fact_id, .. } => format!("fact:{fact_id}"),
        _ => result_id(result),
    };
    match resolution.status(&id) {
        Some(DependencyState::Invalid) => PremiseStatus::Stale,
        Some(DependencyState::Uncertain) => PremiseStatus::Ambiguous,
        Some(DependencyState::Pending) => PremiseStatus::Unsupported,
        Some(DependencyState::Valid) => PremiseStatus::Supported,
        None => PremiseStatus::Supported,
    }
}

pub(crate) async fn witnessed_retrieval(
    store: &MemoryStore,
    request_id: String,
    query: &str,
    results: &[SearchResult],
) -> Result<RetrievalWitnessV1, MemoryError> {
    let query_digest = blake3::hash(query.as_bytes()).to_hex().to_string();
    let result_ids = results.iter().map(result_id).collect::<Vec<_>>();
    let result_digests = results
        .iter()
        .map(|result| blake3::hash(result.content.as_bytes()).to_hex().to_string())
        .collect::<Vec<_>>();
    let (epoch, heads) = store
        .with_read_conn(move |conn| {
            let epoch: i64 = conn.query_row(
                "SELECT retrieval_epoch FROM authority_state WHERE id = 1",
                [],
                |row| row.get(0),
            )?;
            let heads: Vec<(String, String)> = conn
                .prepare(
                    "SELECT lineage_id, active_head_id FROM authority_lineages ORDER BY lineage_id",
                )?
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            let epoch = u64::try_from(epoch)
                .map_err(|_| MemoryError::Other("negative authority epoch".into()))?;
            Ok((epoch, heads))
        })
        .await?;
    let snapshot_digest = blake3::hash(
        &serde_json::to_vec(&(epoch, &heads))
            .map_err(|e| MemoryError::Other(format!("snapshot serialization failed: {e}")))?,
    )
    .to_hex()
    .to_string();
    Ok(RetrievalWitnessV1 {
        schema_version: "retrieval_witness_v1".into(),
        request_id,
        evaluated_at: Utc::now().to_rfc3339(),
        authority_snapshot_id: AuthoritySnapshotId(format!("epoch:{epoch}:{snapshot_digest}")),
        retrieval_epoch: RetrievalEpoch(epoch),
        query_digest,
        config_digest: blake3::hash(b"state-resolution-search-v1")
            .to_hex()
            .to_string(),
        ordered_result_ids: result_ids,
        ordered_result_digests: result_digests,
        stage_outcomes: vec![
            ("retrieval".into(), StageOutcomeV1::Applied),
            ("state_resolution".into(), StageOutcomeV1::Applied),
        ],
        degradations: Vec::new(),
        cached_witness_parent: None,
    })
}

impl MemoryStore {
    /// Resolve a query against the existing search/state/graph APIs and return a
    /// state-labeled answer with a non-optional resolution receipt.
    pub async fn resolve_memory(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        mode: StateResolutionMode,
        budget: usize,
    ) -> Result<ResolvedMemoryAnswerV1, MemoryError> {
        let state_view = mode.state_view();
        let results = self
            .search_with_view(query, top_k, namespaces, None, state_view.clone())
            .await?;
        let result_ids: Vec<String> = results.iter().map(result_id).collect();
        let fact_ids: Vec<String> = results
            .iter()
            .filter_map(|result| match &result.source {
                SearchSource::Fact { fact_id, .. } => Some(format!("fact:{fact_id}")),
                _ => None,
            })
            .collect();
        let stored_edges = if fact_ids.is_empty() {
            Vec::new()
        } else {
            self.list_graph_edges_for_neighborhood(fact_ids.clone(), 2, budget.max(1))
                .await?
        };
        let dependency_edges: Vec<StateDependencyEdgeV1> =
            stored_edges.iter().filter_map(edge_from_stored).collect();
        let mut resolution = resolve_dependency_states(&dependency_edges, &fact_ids, budget);
        let budget_exhausted =
            resolution.budget_exhausted || (budget > 0 && results.len() > budget);
        if budget_exhausted {
            resolution.budget_exhausted = true;
            if resolution.premise_status == PremiseStatus::Supported {
                resolution.premise_status = PremiseStatus::Unsupported;
            }
        }
        let evidence_sufficient = !results.is_empty();
        let decision = answer_policy_for(
            resolution.premise_status,
            evidence_sufficient,
            resolution.unresolved_conflict,
            resolution.invalid_lineage,
            resolution.budget_exhausted,
        );
        let request_id = blake3::hash(query.as_bytes()).to_hex().to_string();
        let retrieval_witness = witnessed_retrieval(self, request_id, query, &results).await?;
        let assertions: Vec<ResolvedAssertionV1> = results
            .iter()
            .map(|result| ResolvedAssertionV1 {
                schema_version: "resolved_assertion_v1".into(),
                memory_id: result_id(result),
                content: result.content.clone(),
                premise_status: result_status(result, &resolution),
                state_view: state_view.clone(),
                source_kind: result.source.source_kind().into(),
            })
            .collect();
        let resolved_memory_ids = assertions
            .iter()
            .map(|a| a.memory_id.clone())
            .collect::<Vec<_>>();
        let alternatives = assertions
            .iter()
            .skip(1)
            .cloned()
            .map(|assertion| BeliefAlternativeV1 {
                assertion,
                reason: "additional witnessed retrieval candidate".into(),
            })
            .collect::<Vec<_>>();
        let alternative_memory_ids = alternatives
            .iter()
            .map(|a| a.assertion.memory_id.clone())
            .collect::<Vec<_>>();
        let mut receipt = StateResolutionReceiptV1::new(
            blake3::hash(query.as_bytes()).to_hex().to_string(),
            state_view.clone(),
            mode,
            resolution.premise_status,
            decision.disposition,
            resolved_memory_ids,
            alternative_memory_ids,
            evidence_sufficient,
            resolution.unresolved_conflict,
            resolution.invalid_lineage,
            resolution.budget_exhausted,
        )?;
        receipt = receipt.with_dependency_edge_digests(
            dependency_edges
                .iter()
                .map(StateDependencyEdgeV1::digest)
                .collect(),
        )?;
        let answer = assertions
            .first()
            .map(|assertion| assertion.content.clone());
        Ok(ResolvedMemoryAnswerV1 {
            schema_version: STATE_RESOLVED_RETRIEVAL_V1.into(),
            query: query.into(),
            answer,
            premise_status: resolution.premise_status,
            answer_disposition: decision.disposition,
            state_view,
            assertions,
            alternatives,
            receipt,
            retrieval_witness,
        })
    }

    /// Persist typed state-dependency relationships through the existing graph store.
    pub async fn add_state_dependency_edge(
        &self,
        edge: StateDependencyEdgeV1,
        weight: f64,
    ) -> Result<crate::StoredGraphEdge, MemoryError> {
        let metadata = match &edge {
            StateDependencyEdgeV1::ScopeChanges {
                from_scope,
                to_scope,
                ..
            } => Some(serde_json::json!({"from_scope": from_scope, "to_scope": to_scope})),
            _ => None,
        };
        self.add_graph_edge(
            edge.source(),
            edge.target(),
            GraphEdgeType::Entity {
                relation: edge.relation().into(),
            },
            weight,
            metadata,
        )
        .await
    }
}
