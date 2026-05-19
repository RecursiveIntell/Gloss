use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use stack_ids::{
    ClaimId, CohortContractId, CounterfactualSliceId, EpisodeId, InterventionId, OutcomeSchemaId,
    ScopeKey,
};

pub const INTERVENTION_BUNDLE_V1_SCHEMA: &str = "intervention_bundle_v1";
pub const OUTCOME_SCHEMA_V1_SCHEMA: &str = "outcome_schema_v1";
pub const COHORT_CONTRACT_V1_SCHEMA: &str = "cohort_contract_v1";
pub const COUNTERFACTUAL_SLICE_V1_SCHEMA: &str = "counterfactual_slice_v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct InterventionBundleV1 {
    pub schema_version: String,
    pub intervention_id: InterventionId,
    pub episode_id: EpisodeId,
    pub unit_of_analysis: String,
    pub treatment_definition: String,
    pub baseline_treatment: String,
    pub start_condition: String,
    pub stop_condition: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allowed_scope: Option<ScopeKey>,
    pub valid_from: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub valid_to: Option<String>,
    pub recorded_at: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub related_claim_ids: Vec<ClaimId>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub policy_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub approval_refs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct OutcomeSchemaV1 {
    pub schema_version: String,
    pub outcome_schema_id: OutcomeSchemaId,
    pub name: String,
    pub measurement_definition: String,
    pub time_window: String,
    pub aggregation_rule: String,
    pub degradation_behavior: String,
    pub exactness_requirement: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CohortContractV1 {
    pub schema_version: String,
    pub cohort_contract_id: CohortContractId,
    pub unit_definition: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inclusion_logic: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub exclusion_logic: Vec<String>,
    pub scope_namespace: String,
    pub workload_class: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub environment_constraints: Vec<String>,
    pub replay_linkage: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CounterfactualSliceV1 {
    pub schema_version: String,
    pub counterfactual_slice_id: CounterfactualSliceId,
    pub intervention_id: InterventionId,
    pub baseline_treatment: String,
    pub cohort_contract_id: CohortContractId,
    pub as_of_valid_time: String,
    pub as_of_recorded_time: String,
    pub replayable_data_slice: String,
    pub exactness_target: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modeling_assumptions: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected_failure_modes: Vec<String>,
}
