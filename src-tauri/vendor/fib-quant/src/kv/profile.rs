use serde::{Deserialize, Serialize};

use crate::{
    digest::json_digest, profile::FibQuantProfileV1, rotation::StoredRotation, FibQuantError,
    Result,
};

use super::{
    layout::KvPageGeometryV1,
    shape::{KvRole, KvTensorShapeV1},
};

pub const KV_PROFILE_SCHEMA: &str = "fib_quant_kv_compression_profile_v1";

/// Compression axis policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvAxisPolicyV1 {
    /// Keep vectors raw.
    Raw,
    /// Compress each token/head vector independently.
    PerToken,
    /// Compress channel vectors across tokens. Planned for backend-specific paths.
    PerChannel,
    /// Key per-channel, value per-token baseline.
    RoleAwareKiviStyle,
}

/// Fallback mode for unsupported or rejected regions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvFallbackModeV1 {
    /// Store raw f32 blocks.
    KeepRaw,
    /// Fail the operation.
    FailClosed,
}

/// Protected raw regions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvProtectedPolicyV1 {
    /// First N tokens are raw.
    pub first_tokens_raw: u32,
    /// Last N tokens are raw.
    pub last_tokens_raw: u32,
    /// Layers kept raw.
    pub raw_layers: Vec<u32>,
    /// KV heads kept raw.
    pub raw_heads: Vec<u32>,
}

impl KvProtectedPolicyV1 {
    /// No protected regions.
    pub fn none() -> Self {
        Self {
            first_tokens_raw: 0,
            last_tokens_raw: 0,
            raw_layers: Vec::new(),
            raw_heads: Vec::new(),
        }
    }

    /// Whether a vector falls in a protected raw region.
    pub fn is_protected(&self, shape: &KvTensorShapeV1, layer: u32, head: u32, token: u32) -> bool {
        token < self.first_tokens_raw
            || token.saturating_add(self.last_tokens_raw) >= shape.tokens
            || self.raw_layers.contains(&layer)
            || self.raw_heads.contains(&head)
    }

    pub(crate) fn validate_for_shape(&self, shape: &KvTensorShapeV1) -> Result<()> {
        if self.first_tokens_raw > shape.tokens || self.last_tokens_raw > shape.tokens {
            return Err(FibQuantError::CorruptPayload(
                "protected token count exceeds shape tokens".into(),
            ));
        }
        if self.raw_layers.iter().any(|layer| *layer >= shape.layers) {
            return Err(FibQuantError::CorruptPayload(
                "protected layer outside shape".into(),
            ));
        }
        if self.raw_heads.iter().any(|head| *head >= shape.kv_heads) {
            return Err(FibQuantError::CorruptPayload(
                "protected head outside shape".into(),
            ));
        }
        Ok(())
    }
}

/// Fallback declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvFallbackPolicyV1 {
    /// Fallback mode.
    pub mode: KvFallbackModeV1,
    /// Whether raw fallback blocks are always allowed.
    pub raw_fallback_available: bool,
}

impl KvFallbackPolicyV1 {
    /// Conservative raw fallback.
    pub fn keep_raw() -> Self {
        Self {
            mode: KvFallbackModeV1::KeepRaw,
            raw_fallback_available: true,
        }
    }
}

/// Quality budget used by policy and receipts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvQualityBudgetV1 {
    /// Maximum key logit MSE.
    pub max_logit_mse: Option<f64>,
    /// Maximum attention total variation distance.
    pub max_attention_tv: Option<f64>,
    /// Maximum top-k disagreement rate.
    pub max_topk_disagreement: Option<f64>,
    /// Maximum value aggregation MSE.
    pub max_value_aggregation_mse: Option<f64>,
    /// Fallback mode on violation.
    pub fallback_on_violation: KvFallbackModeV1,
}

impl KvQualityBudgetV1 {
    /// Unknown budget; policies should prefer calibration or raw fallback.
    pub fn unavailable() -> Self {
        Self {
            max_logit_mse: None,
            max_attention_tv: None,
            max_topk_disagreement: None,
            max_value_aggregation_mse: None,
            fallback_on_violation: KvFallbackModeV1::KeepRaw,
        }
    }

    /// Whether any quantitative budget is present.
    pub fn has_any_metric(&self) -> bool {
        self.max_logit_mse.is_some()
            || self.max_attention_tv.is_some()
            || self.max_topk_disagreement.is_some()
            || self.max_value_aggregation_mse.is_some()
    }

    pub(crate) fn validate(&self) -> Result<()> {
        for (name, value) in [
            ("max_logit_mse", self.max_logit_mse),
            ("max_attention_tv", self.max_attention_tv),
            ("max_topk_disagreement", self.max_topk_disagreement),
            ("max_value_aggregation_mse", self.max_value_aggregation_mse),
        ] {
            if let Some(value) = value {
                if !value.is_finite() || value < 0.0 {
                    return Err(FibQuantError::CorruptPayload(format!(
                        "{name} must be finite and nonnegative"
                    )));
                }
            }
        }
        Ok(())
    }
}

/// KV compression profile binding shape, FibQuant artifacts, policy, and budgets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvCompressionProfileV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Operator-chosen profile identifier.
    pub profile_id: String,
    /// Digest of the logical KV shape.
    pub shape_digest: String,
    /// Embedded FibQuant vector profile.
    pub fib_profile: FibQuantProfileV1,
    /// Digest of `fib_profile`.
    pub fib_profile_digest: String,
    /// Digest of the matching codebook.
    pub codebook_digest: String,
    /// Digest of the matching rotation.
    pub rotation_digest: String,
    /// Role this profile targets.
    pub role_policy: KvRole,
    /// Axis/policy declaration.
    pub axis_policy: KvAxisPolicyV1,
    /// Fixed-size page geometry.
    pub page_geometry: KvPageGeometryV1,
    /// Protected raw regions.
    pub protected_policy: KvProtectedPolicyV1,
    /// Raw/fail fallback policy.
    pub fallback_policy: KvFallbackPolicyV1,
    /// Quality budget.
    pub quality_budget: KvQualityBudgetV1,
    /// Calibration artifact digest or a stable missing marker.
    pub calibration_digest: String,
}

impl KvCompressionProfileV1 {
    /// Build a profile from an already built quantizer identity.
    pub fn from_parts(
        profile_id: impl Into<String>,
        shape: &KvTensorShapeV1,
        fib_profile: FibQuantProfileV1,
        codebook_digest: impl Into<String>,
        axis_policy: KvAxisPolicyV1,
        page_geometry: KvPageGeometryV1,
    ) -> Result<Self> {
        shape.validate_block_dim(fib_profile.block_dim)?;
        fib_profile.validate()?;
        if fib_profile.ambient_dim != shape.head_dim {
            return Err(FibQuantError::CorruptPayload(
                "fib profile ambient_dim must equal kv head_dim for CPU reference codec".into(),
            ));
        }
        let rotation_digest =
            StoredRotation::new(fib_profile.ambient_dim as usize, fib_profile.rotation_seed)?
                .digest()?;
        let profile = Self {
            schema_version: KV_PROFILE_SCHEMA.into(),
            profile_id: profile_id.into(),
            shape_digest: shape.digest()?,
            fib_profile_digest: fib_profile.digest()?,
            role_policy: shape.role,
            fib_profile,
            codebook_digest: codebook_digest.into(),
            rotation_digest,
            axis_policy,
            page_geometry,
            protected_policy: KvProtectedPolicyV1::none(),
            fallback_policy: KvFallbackPolicyV1::keep_raw(),
            quality_budget: KvQualityBudgetV1::unavailable(),
            calibration_digest: "missing:calibration".into(),
        };
        profile.validate_for_shape(shape)?;
        Ok(profile)
    }

    /// Validate profile against the expected shape.
    pub fn validate_for_shape(&self, shape: &KvTensorShapeV1) -> Result<()> {
        if self.schema_version != KV_PROFILE_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "kv profile schema_version {}, expected {KV_PROFILE_SCHEMA}",
                self.schema_version
            )));
        }
        shape.validate_block_dim(self.fib_profile.block_dim)?;
        if self.shape_digest != shape.digest()? {
            return Err(FibQuantError::ProfileDigestMismatch {
                expected: shape.digest()?,
                actual: self.shape_digest.clone(),
            });
        }
        if self.role_policy != shape.role {
            return Err(FibQuantError::CorruptPayload(
                "kv profile role does not match shape role".into(),
            ));
        }
        self.fib_profile.validate()?;
        if self.fib_profile.ambient_dim != shape.head_dim {
            return Err(FibQuantError::CorruptPayload(
                "fib profile ambient_dim must equal kv head_dim".into(),
            ));
        }
        let expected_fib = self.fib_profile.digest()?;
        if self.fib_profile_digest != expected_fib {
            return Err(FibQuantError::ProfileDigestMismatch {
                expected: expected_fib,
                actual: self.fib_profile_digest.clone(),
            });
        }
        let expected_rotation = StoredRotation::new(
            self.fib_profile.ambient_dim as usize,
            self.fib_profile.rotation_seed,
        )?
        .digest()?;
        if self.rotation_digest != expected_rotation {
            return Err(FibQuantError::RotationDigestMismatch {
                expected: expected_rotation,
                actual: self.rotation_digest.clone(),
            });
        }
        if self.codebook_digest.is_empty() {
            return Err(FibQuantError::CorruptPayload(
                "kv profile codebook_digest must be nonempty".into(),
            ));
        }
        self.page_geometry.validate_for_shape(shape)?;
        self.protected_policy.validate_for_shape(shape)?;
        self.quality_budget.validate()?;
        Ok(())
    }

    /// Stable digest for the KV profile.
    pub fn digest(&self, shape: &KvTensorShapeV1) -> Result<String> {
        self.validate_for_shape(shape)?;
        json_digest(KV_PROFILE_SCHEMA, self)
    }
}
