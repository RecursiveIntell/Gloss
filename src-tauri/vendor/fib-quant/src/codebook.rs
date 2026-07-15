use serde::{Deserialize, Serialize};

use crate::{
    digest::json_digest,
    directions::directions_for_method,
    lloyd::{refine_codebook, LloydReportV1},
    profile::{FibQuantProfileV1, LloydMode, RadiusMethod},
    rotation::StoredRotation,
    spherical_beta::{radius_quantile, radius_quantile_k2_closed_form},
    FibQuantError, Result,
};

pub const CODEBOOK_SCHEMA: &str = "fib_codebook_v1";

/// Persisted FibQuant codebook in row-major codeword order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FibCodebookV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Profile used to create this codebook.
    pub profile: FibQuantProfileV1,
    /// Digest of `profile`.
    pub profile_digest: String,
    /// Digest of profile, report, and row-major codewords.
    pub codebook_digest: String,
    /// Digest of deterministic ambient rotation identity and matrix.
    pub rotation_digest: String,
    /// Row-major `N x k` f32 codeword storage.
    pub codewords: Vec<f32>,
    /// Initial codebook training MSE before Lloyd-Max.
    pub init_mse: f64,
    /// Best training MSE after Lloyd-Max.
    pub training_mse: f64,
    /// Lloyd-Max refinement receipt.
    pub refinement_report: LloydReportV1,
}

impl FibCodebookV1 {
    /// Build a full refined codebook for a profile.
    pub fn build(profile: FibQuantProfileV1) -> Result<Self> {
        profile.validate()?;
        let profile_digest = profile.digest()?;
        let rotation_digest =
            StoredRotation::new(profile.ambient_dim as usize, profile.rotation_seed)?.digest()?;
        let initial = build_initial_codebook(&profile)?;
        let refined = match profile.lloyd_mode {
            LloydMode::Refine => refine_codebook(&profile, &initial)?,
            LloydMode::Skip => crate::lloyd::skip_refinement(&profile, &initial)?,
        };
        let mut codebook = Self {
            schema_version: CODEBOOK_SCHEMA.into(),
            profile,
            profile_digest,
            codebook_digest: String::new(),
            rotation_digest,
            codewords: refined.codewords,
            init_mse: refined.init_mse,
            training_mse: refined.training_mse,
            refinement_report: refined.report,
        };
        codebook.codebook_digest = codebook.compute_digest()?;
        Ok(codebook)
    }

    /// Validate shape and digest fields.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != CODEBOOK_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "codebook schema_version {}, expected {CODEBOOK_SCHEMA}",
                self.schema_version
            )));
        }
        self.profile.validate()?;
        self.refinement_report
            .validate_against_profile(&self.profile)?;
        let expected_profile = self.profile.digest()?;
        if self.profile_digest != expected_profile {
            return Err(FibQuantError::ProfileDigestMismatch {
                expected: expected_profile,
                actual: self.profile_digest.clone(),
            });
        }
        let expected_rotation = StoredRotation::new(
            self.profile.ambient_dim as usize,
            self.profile.rotation_seed,
        )?
        .digest()?;
        if self.rotation_digest != expected_rotation {
            return Err(FibQuantError::RotationDigestMismatch {
                expected: expected_rotation,
                actual: self.rotation_digest.clone(),
            });
        }
        let expected_codebook = self.compute_digest()?;
        if self.codebook_digest != expected_codebook {
            return Err(FibQuantError::CodebookDigestMismatch {
                expected: expected_codebook,
                actual: self.codebook_digest.clone(),
            });
        }
        let expected_len = (self.profile.codebook_size as usize)
            .checked_mul(self.profile.block_dim as usize)
            .ok_or_else(|| {
                FibQuantError::ResourceLimitExceeded("codebook value count overflow".into())
            })?;
        if self.codewords.len() != expected_len {
            return Err(FibQuantError::CorruptPayload(format!(
                "codebook has {} values, expected {expected_len}",
                self.codewords.len()
            )));
        }
        if self.codewords.iter().any(|value| !value.is_finite()) {
            return Err(FibQuantError::CorruptPayload(
                "codebook contains non-finite value".into(),
            ));
        }
        Ok(())
    }

    /// Return codeword `index` as f64 values.
    pub fn codeword(&self, index: usize) -> Result<Vec<f64>> {
        let n = self.profile.codebook_size as usize;
        let k = self.profile.block_dim as usize;
        if index >= n {
            return Err(FibQuantError::IndexOutOfRange {
                index: index as u32,
                codebook_size: n as u32,
            });
        }
        Ok(self.codewords[index * k..(index + 1) * k]
            .iter()
            .map(|value| f64::from(*value))
            .collect())
    }

    /// Deterministic codebook digest.
    pub fn compute_digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct DigestView<'a> {
            schema_version: &'a str,
            profile_digest: &'a str,
            rotation_digest: &'a str,
            codewords: &'a [f32],
            init_mse: f64,
            training_mse: f64,
            refinement_report: &'a LloydReportV1,
        }
        json_digest(
            CODEBOOK_SCHEMA,
            &DigestView {
                schema_version: &self.schema_version,
                profile_digest: &self.profile_digest,
                rotation_digest: &self.rotation_digest,
                codewords: &self.codewords,
                init_mse: self.init_mse,
                training_mse: self.training_mse,
                refinement_report: &self.refinement_report,
            },
        )
    }
}

/// Build deterministic radial-angular initialization.
pub fn build_initial_codebook(profile: &FibQuantProfileV1) -> Result<Vec<f64>> {
    profile.validate()?;
    let d = profile.ambient_dim as usize;
    let k = profile.block_dim as usize;
    let n = profile.codebook_size as usize;
    let directions = directions_for_method(k, n, &profile.direction_method)?;
    let value_count = n.checked_mul(k).ok_or_else(|| {
        FibQuantError::ResourceLimitExceeded("codebook value count overflow".into())
    })?;
    let mut codewords = Vec::with_capacity(value_count);
    for (idx, direction) in directions.iter().enumerate() {
        let radius = radius_for_method(profile, d, k, idx + 1, n)?;
        for value in direction {
            let code = radius * value;
            if !code.is_finite() {
                return Err(FibQuantError::NumericalFailure(
                    "non-finite initialized codeword".into(),
                ));
            }
            codewords.push(code);
        }
    }
    Ok(codewords)
}

fn radius_for_method(
    profile: &FibQuantProfileV1,
    d: usize,
    k: usize,
    idx: usize,
    n: usize,
) -> Result<f64> {
    match profile.radius_method {
        RadiusMethod::K2ClosedForm if k == 2 => {
            let q = (idx as f64 - 0.5) / n as f64;
            radius_quantile_k2_closed_form(d, q)
        }
        RadiusMethod::BetaQuantile if k >= 3 => radius_quantile(d, k, idx, n),
        _ => Err(FibQuantError::CorruptPayload(format!(
            "radius method {:?} is not supported for k={k}",
            profile.radius_method
        ))),
    }
}
