use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    ApplicabilityContextId, CompiledObligationSetId, CompositionConflictSetId,
    CompositionReceiptId, EffectiveConstitutionId, ProfileExceptionBundleId, ProfileSetId,
};

/// Canonical v25 constitutional citation shared by effect and downstream consumer artifacts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct V25ConstitutionCitation {
    pub applicability_context_id: ApplicabilityContextId,
    pub profile_set_id: ProfileSetId,
    pub composition_receipt_id: CompositionReceiptId,
    pub effective_constitution_id: EffectiveConstitutionId,
    pub compiled_obligation_set_id: CompiledObligationSetId,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub composition_conflict_set_id: Option<CompositionConflictSetId>,
    #[serde(default)]
    pub profile_exception_bundle_ids: Vec<ProfileExceptionBundleId>,
}

impl V25ConstitutionCitation {
    /// Builds a complete v25 constitutional citation for a published artifact.
    pub fn new(
        applicability_context_id: ApplicabilityContextId,
        profile_set_id: ProfileSetId,
        composition_receipt_id: CompositionReceiptId,
        effective_constitution_id: EffectiveConstitutionId,
        compiled_obligation_set_id: CompiledObligationSetId,
        composition_conflict_set_id: Option<CompositionConflictSetId>,
        profile_exception_bundle_ids: Vec<ProfileExceptionBundleId>,
    ) -> Self {
        Self {
            applicability_context_id,
            profile_set_id,
            composition_receipt_id,
            effective_constitution_id,
            compiled_obligation_set_id,
            composition_conflict_set_id,
            profile_exception_bundle_ids,
        }
    }
}

impl Default for V25ConstitutionCitation {
    fn default() -> Self {
        Self {
            applicability_context_id: ApplicabilityContextId::new(""),
            profile_set_id: ProfileSetId::new(""),
            composition_receipt_id: CompositionReceiptId::new(""),
            effective_constitution_id: EffectiveConstitutionId::new(""),
            compiled_obligation_set_id: CompiledObligationSetId::new(""),
            composition_conflict_set_id: None,
            profile_exception_bundle_ids: Vec::new(),
        }
    }
}
