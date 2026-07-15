use serde::{Deserialize, Serialize};

use crate::{FibQuantError, Result};

use super::{
    profile::{KvAxisPolicyV1, KvProtectedPolicyV1},
    shape::{KvRole, KvTensorShapeV1},
};

/// Named compression strategy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvCompressionStrategyV1 {
    /// Raw reference path.
    Raw,
    /// FibQuant each token/head vector.
    FibQuantPerToken,
    /// FibQuant channel/group vectors. Declared for policy baselines.
    FibQuantPerChannel,
    /// KIVI-style role split: keys channel-wise, values token-wise.
    RoleAwareKiviStyleBaseline,
    /// Experimental FibQuant role-aware profile.
    ExperimentalFibQuantRoleAware,
}

/// Policy configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvCompressionPolicyV1 {
    /// Strategy.
    pub strategy: KvCompressionStrategyV1,
    /// Protected raw regions.
    pub protected_policy: KvProtectedPolicyV1,
    /// Require a calibration digest/budget before compression.
    pub require_calibration: bool,
    /// Allow raw fallback decisions.
    pub allow_raw_fallback: bool,
}

impl KvCompressionPolicyV1 {
    /// Conservative raw policy.
    pub fn raw() -> Self {
        Self {
            strategy: KvCompressionStrategyV1::Raw,
            protected_policy: KvProtectedPolicyV1::none(),
            require_calibration: false,
            allow_raw_fallback: true,
        }
    }

    /// Role-aware baseline policy.
    pub fn role_aware_baseline() -> Self {
        Self {
            strategy: KvCompressionStrategyV1::RoleAwareKiviStyleBaseline,
            protected_policy: KvProtectedPolicyV1 {
                first_tokens_raw: 0,
                last_tokens_raw: 1,
                raw_layers: Vec::new(),
                raw_heads: Vec::new(),
            },
            require_calibration: false,
            allow_raw_fallback: true,
        }
    }
}

/// Decision action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvDecisionActionV1 {
    /// Store raw.
    KeepRaw,
    /// Compress using the selected axis.
    Compress,
    /// Calibration is needed before compression.
    NeedCalibration,
    /// Quarantine stale or mismatched artifacts.
    Quarantine,
}

/// Decision reason.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvDecisionReasonV1 {
    /// Explicit raw strategy.
    RawStrategy,
    /// Protected token, layer, or head.
    ProtectedRegion,
    /// Unsupported shape/layout for selected strategy.
    UnsupportedShape,
    /// Missing calibration or quality budget.
    CalibrationMissing,
    /// Key role selects key-oriented axis.
    KeyRoleAxis,
    /// Value role selects value-oriented axis.
    ValueRoleAxis,
    /// Experimental role-aware FibQuant selection.
    ExperimentalRoleAware,
}

/// Policy decision for one vector/block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvCompressionDecisionV1 {
    /// Action to take.
    pub action: KvDecisionActionV1,
    /// Selected axis if compressing.
    pub axis_policy: KvAxisPolicyV1,
    /// Reasons for the action.
    pub reasons: Vec<KvDecisionReasonV1>,
}

/// Decide compression for a single logical vector.
pub fn decide_kv_compression(
    policy: &KvCompressionPolicyV1,
    shape: &KvTensorShapeV1,
    layer: u32,
    head: u32,
    token: u32,
    has_calibration: bool,
) -> Result<KvCompressionDecisionV1> {
    shape.validate()?;
    policy.protected_policy.validate_for_shape(shape)?;
    if layer >= shape.layers || head >= shape.kv_heads || token >= shape.tokens {
        return Err(FibQuantError::CorruptPayload(
            "kv policy index outside shape".into(),
        ));
    }
    if policy
        .protected_policy
        .is_protected(shape, layer, head, token)
    {
        return Ok(KvCompressionDecisionV1 {
            action: KvDecisionActionV1::KeepRaw,
            axis_policy: KvAxisPolicyV1::Raw,
            reasons: vec![KvDecisionReasonV1::ProtectedRegion],
        });
    }
    if policy.require_calibration && !has_calibration {
        return Ok(KvCompressionDecisionV1 {
            action: if policy.allow_raw_fallback {
                KvDecisionActionV1::KeepRaw
            } else {
                KvDecisionActionV1::NeedCalibration
            },
            axis_policy: KvAxisPolicyV1::Raw,
            reasons: vec![KvDecisionReasonV1::CalibrationMissing],
        });
    }
    match policy.strategy {
        KvCompressionStrategyV1::Raw => Ok(KvCompressionDecisionV1 {
            action: KvDecisionActionV1::KeepRaw,
            axis_policy: KvAxisPolicyV1::Raw,
            reasons: vec![KvDecisionReasonV1::RawStrategy],
        }),
        KvCompressionStrategyV1::FibQuantPerToken => Ok(KvCompressionDecisionV1 {
            action: KvDecisionActionV1::Compress,
            axis_policy: KvAxisPolicyV1::PerToken,
            reasons: role_reason(shape.role),
        }),
        KvCompressionStrategyV1::FibQuantPerChannel => Ok(KvCompressionDecisionV1 {
            action: KvDecisionActionV1::Compress,
            axis_policy: KvAxisPolicyV1::PerChannel,
            reasons: role_reason(shape.role),
        }),
        KvCompressionStrategyV1::RoleAwareKiviStyleBaseline => {
            let axis_policy = match shape.role {
                KvRole::Key => KvAxisPolicyV1::PerChannel,
                KvRole::Value => KvAxisPolicyV1::PerToken,
            };
            Ok(KvCompressionDecisionV1 {
                action: KvDecisionActionV1::Compress,
                axis_policy,
                reasons: role_reason(shape.role),
            })
        }
        KvCompressionStrategyV1::ExperimentalFibQuantRoleAware => {
            let axis_policy = match shape.role {
                KvRole::Key => KvAxisPolicyV1::PerChannel,
                KvRole::Value => KvAxisPolicyV1::PerToken,
            };
            Ok(KvCompressionDecisionV1 {
                action: KvDecisionActionV1::Compress,
                axis_policy,
                reasons: vec![KvDecisionReasonV1::ExperimentalRoleAware],
            })
        }
    }
}

fn role_reason(role: KvRole) -> Vec<KvDecisionReasonV1> {
    match role {
        KvRole::Key => vec![KvDecisionReasonV1::KeyRoleAxis],
        KvRole::Value => vec![KvDecisionReasonV1::ValueRoleAxis],
    }
}
