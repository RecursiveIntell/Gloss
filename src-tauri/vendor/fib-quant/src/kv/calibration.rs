use serde::{Deserialize, Serialize};

use crate::{digest::json_digest, FibQuantError, Result};

use super::{
    profile::KvAxisPolicyV1,
    receipt::kv_tensor_digest,
    shape::{KvRole, KvTensorShapeV1},
};

pub const KV_CALIBRATION_SCHEMA: &str = "fib_quant_kv_calibration_summary_v1";

/// Synthetic/fixture-driven KV calibration summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvCalibrationSummaryV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Shape digest.
    pub shape_digest: String,
    /// Source tensor digest.
    pub source_digest: String,
    /// Role.
    pub role: KvRole,
    /// Number of vectors inspected.
    pub vector_count: u32,
    /// Mean vector norm.
    pub mean_norm: f64,
    /// Maximum vector norm.
    pub max_norm: f64,
    /// Count of vectors above the outlier norm threshold.
    pub outlier_count: u32,
    /// Recommended compression axis.
    pub recommended_axis: KvAxisPolicyV1,
    /// Recommended first raw tokens.
    pub recommended_first_tokens_raw: u32,
    /// Recommended last raw tokens.
    pub recommended_last_tokens_raw: u32,
    /// Recommended bits per coordinate when known.
    pub recommended_bits_per_coord: Option<f64>,
    /// Summary digest.
    pub calibration_digest: String,
}

/// Calibrate a bounded synthetic/fixture tensor.
pub fn calibrate_kv_tensor(
    shape: &KvTensorShapeV1,
    values: &[f32],
    outlier_norm_threshold: f64,
) -> Result<KvCalibrationSummaryV1> {
    shape.validate()?;
    if values.len() != shape.element_count()? {
        return Err(FibQuantError::CorruptPayload(
            "calibration tensor length mismatch".into(),
        ));
    }
    if !outlier_norm_threshold.is_finite() || outlier_norm_threshold < 0.0 {
        return Err(FibQuantError::CorruptPayload(
            "outlier threshold must be finite and nonnegative".into(),
        ));
    }
    let mut norm_sum = 0.0;
    let mut max_norm = 0.0f64;
    let mut outlier_count = 0u32;
    for vector in values.chunks_exact(shape.head_dim as usize) {
        if vector.iter().any(|value| !value.is_finite()) {
            return Err(FibQuantError::CorruptPayload(
                "calibration tensor contains non-finite value".into(),
            ));
        }
        let norm = vector
            .iter()
            .map(|value| {
                let value = f64::from(*value);
                value * value
            })
            .sum::<f64>()
            .sqrt();
        norm_sum += norm;
        max_norm = max_norm.max(norm);
        if norm > outlier_norm_threshold {
            outlier_count += 1;
        }
    }
    let vector_count = shape.vector_count()? as u32;
    let recommended_axis = match shape.role {
        KvRole::Key => KvAxisPolicyV1::PerChannel,
        KvRole::Value => KvAxisPolicyV1::PerToken,
    };
    let mut summary = KvCalibrationSummaryV1 {
        schema_version: KV_CALIBRATION_SCHEMA.into(),
        shape_digest: shape.digest()?,
        source_digest: kv_tensor_digest(values)?,
        role: shape.role,
        vector_count,
        mean_norm: norm_sum / f64::from(vector_count),
        max_norm,
        outlier_count,
        recommended_axis,
        recommended_first_tokens_raw: 0,
        recommended_last_tokens_raw: 1.min(shape.tokens),
        recommended_bits_per_coord: None,
        calibration_digest: String::new(),
    };
    summary.calibration_digest = summary.compute_digest()?;
    Ok(summary)
}

impl KvCalibrationSummaryV1 {
    /// Validate and recompute digest.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != KV_CALIBRATION_SCHEMA {
            return Err(FibQuantError::CorruptPayload(
                "invalid kv calibration schema".into(),
            ));
        }
        if self.vector_count == 0 {
            return Err(FibQuantError::ZeroDimension);
        }
        for (name, value) in [("mean_norm", self.mean_norm), ("max_norm", self.max_norm)] {
            if !value.is_finite() || value < 0.0 {
                return Err(FibQuantError::CorruptPayload(format!(
                    "{name} must be finite and nonnegative"
                )));
            }
        }
        if let Some(bits) = self.recommended_bits_per_coord {
            if !bits.is_finite() || bits <= 0.0 {
                return Err(FibQuantError::CorruptPayload(
                    "recommended bits must be finite and positive".into(),
                ));
            }
        }
        if self.calibration_digest != self.compute_digest()? {
            return Err(FibQuantError::CorruptPayload(
                "kv calibration digest mismatch".into(),
            ));
        }
        Ok(())
    }

    /// Compute digest excluding the digest field.
    pub fn compute_digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct DigestView<'a> {
            schema_version: &'a str,
            shape_digest: &'a str,
            source_digest: &'a str,
            role: KvRole,
            vector_count: u32,
            mean_norm: f64,
            max_norm: f64,
            outlier_count: u32,
            recommended_axis: KvAxisPolicyV1,
            recommended_first_tokens_raw: u32,
            recommended_last_tokens_raw: u32,
            recommended_bits_per_coord: Option<f64>,
        }
        json_digest(
            KV_CALIBRATION_SCHEMA,
            &DigestView {
                schema_version: &self.schema_version,
                shape_digest: &self.shape_digest,
                source_digest: &self.source_digest,
                role: self.role,
                vector_count: self.vector_count,
                mean_norm: self.mean_norm,
                max_norm: self.max_norm,
                outlier_count: self.outlier_count,
                recommended_axis: self.recommended_axis,
                recommended_first_tokens_raw: self.recommended_first_tokens_raw,
                recommended_last_tokens_raw: self.recommended_last_tokens_raw,
                recommended_bits_per_coord: self.recommended_bits_per_coord,
            },
        )
    }
}
