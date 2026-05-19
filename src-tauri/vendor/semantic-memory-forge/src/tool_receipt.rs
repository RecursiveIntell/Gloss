use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use stack_ids::{AttemptId, ContentDigest, TraceCtx, TrialId};

pub const FORGE_TOOL_RECEIPT_V1_SCHEMA: &str = "forge_tool_receipt_v1";
pub const FORGE_TOOL_RECEIPT_V2_SCHEMA: &str = "forge_tool_receipt_v2";

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeToolReceiptV1 {
    pub schema_version: String,
    pub receipt_id: String,
    pub tool_run_id: String,
    pub tool_name: String,
    pub tool_version: String,
    pub backend_kind: String,
    pub input_digest: ContentDigest,
    pub output_digest_or_refs: Value,
    pub policy_hash: ContentDigest,
    pub approval_state: String,
    pub host_identity: String,
    pub started_at: String,
    pub finished_at: String,
    pub trace_ctx: TraceCtx,
    pub attempt_id: AttemptId,
    pub trial_id: TrialId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    pub retry_owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
    pub raw_payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeToolBudgetContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_budget_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_budget_units: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ForgeToolReceiptV2 {
    pub schema_version: String,
    pub receipt_id: String,
    pub tool_run_id: String,
    pub tool_name: String,
    pub tool_version: String,
    pub backend_kind: String,
    pub input_digest: ContentDigest,
    pub output_digest_or_refs: Value,
    pub policy_hash: ContentDigest,
    pub approval_state: String,
    pub host_identity: String,
    pub started_at: String,
    pub finished_at: String,
    pub trace_ctx: TraceCtx,
    pub attempt_id: AttemptId,
    pub trial_id: TrialId,
    pub planner_stage: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deadline: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget_context: Option<ForgeToolBudgetContext>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub family_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_parent_receipt_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error_class: Option<String>,
    pub retry_owner: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_link: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_call_id: Option<String>,
    pub raw_payload: Value,
}
