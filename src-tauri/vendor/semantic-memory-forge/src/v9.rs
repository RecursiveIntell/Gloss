use crate::VerificationSummary;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use stack_ids::{AttemptId, ContentDigest, EnvelopeId, EpisodeId, ScopeKey, TraceCtx, TrialId};

pub const EPISODE_BUNDLE_V1_SCHEMA: &str = "episode_bundle_v1";
pub const EXECUTION_CONTEXT_V1_SCHEMA: &str = "execution_context_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DispatchOutcomeV1 {
    Succeeded,
    Failed,
    Degraded,
    Cancelled,
    Exhausted,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecutionContextV1 {
    pub schema_version: String,
    pub trace_ctx: TraceCtx,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<TrialId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_class: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub queue_hops: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget_units: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub degradation_markers: Vec<String>,
    pub dispatch_outcome: DispatchOutcomeV1,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_route: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancellation_reason: Option<String>,
    /// Backpointer to the tool receipt that produced this execution context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_receipt_ref: Option<String>,
    /// Backpointer to the provider call that fulfilled this execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_ref: Option<String>,
    /// Backpointer to the replay parent execution context, if this is a retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_parent_ref: Option<String>,
}

impl ExecutionContextV1 {
    /// Creates an execution context anchored to the supplied trace context.
    pub fn new(trace_ctx: TraceCtx) -> Self {
        Self {
            schema_version: EXECUTION_CONTEXT_V1_SCHEMA.into(),
            trace_ctx,
            attempt_id: None,
            trial_id: None,
            replay_link: None,
            workload_class: None,
            queue_hops: Vec::new(),
            deadline: None,
            cost_budget_units: None,
            degradation_markers: Vec::new(),
            dispatch_outcome: DispatchOutcomeV1::Succeeded,
            environment_fingerprint: None,
            provider_route: None,
            cancellation_reason: None,
            tool_receipt_ref: None,
            provider_call_ref: None,
            replay_parent_ref: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EpisodeBundleV1 {
    pub schema_version: String,
    pub bundle_id: String,
    pub episode_id: EpisodeId,
    pub primary_document_id: String,
    pub namespace: String,
    pub scope_key: ScopeKey,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_from: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    pub exported_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_at: Option<String>,
    pub source_envelope_id: EnvelopeId,
    pub content_digest: ContentDigest,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_evidence_pointers: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub source_receipt_digests: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claim_version_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relation_version_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_summary: Option<VerificationSummary>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refutation_artifact_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub control_plane_refs: Vec<String>,
    pub execution_context: ExecutionContextV1,
    pub thin_export: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supersedes_bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence_bundle_id: Option<String>,
}
