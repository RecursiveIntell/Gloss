use serde::{Deserialize, Serialize};

use crate::{digest::json_digest, rotation::ROTATION_ALGORITHM_VERSION, FibQuantError, Result};

pub const PROFILE_SCHEMA: &str = "fib_quant_profile_v1";
/// Maximum ambient dimension accepted by the alpha profile validator.
pub const MAX_AMBIENT_DIM: usize = 16_384;
/// Maximum block dimension accepted by the alpha profile validator.
pub const MAX_BLOCK_DIM: usize = 256;
/// Maximum codebook size accepted by the alpha profile validator.
pub const MAX_CODEBOOK_SIZE: usize = 1 << 20;
/// Maximum Lloyd training samples accepted by the alpha profile validator.
pub const MAX_TRAINING_SAMPLES: u32 = 10_000_000;
/// Maximum number of scalar values in a dense rotation matrix.
pub const MAX_ROTATION_MATRIX_VALUES: usize = 16_777_216;
/// Maximum number of scalar values in an `N x k` codebook.
pub const MAX_CODEBOOK_VALUES: usize = 67_108_864;
/// Maximum bits in a packed fixed-rate payload.
pub const MAX_PACKED_INDEX_BITS: usize = 1 << 34;

const RATE_TOLERANCE: f64 = 1.0e-12;
const MAX_LLOYD_RESTARTS: u32 = 1_024;
const MAX_LLOYD_ITERATIONS: u32 = 100_000;

/// Norm payload representation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum NormFormat {
    /// Paper path: fp16 scalar norm side header.
    Fp16Paper,
    /// Reference/test path: f32 scalar norm side header.
    #[doc(hidden)]
    F32Reference,
}

/// Source used for training samples.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum SourceMode {
    /// Direct spherical-Beta sampler.
    CanonicalSphericalBeta,
    /// Normalized Gaussian projection reference sampler.
    #[doc(hidden)]
    ReferenceGaussianProjection,
}

/// Radius initialization method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum RadiusMethod {
    /// Bennett-Gersho Beta-quantile radii.
    BetaQuantile,
    /// Paper closed form for k=2.
    K2ClosedForm,
    /// Explicit large-d single-shell initialization.
    #[doc(hidden)]
    LargeDSingleShellExplicit,
}

/// Direction initialization method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum DirectionMethod {
    /// Planar Fibonacci spiral.
    FibonacciSpiral,
    /// Fibonacci sphere.
    FibonacciSphere,
    /// Roberts-Kronecker rank-one sequence.
    RobertsKronecker,
}

/// Empty-cell handling during Lloyd-Max refinement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum EmptyCellPolicy {
    /// Split the occupied cell with highest distortion.
    SplitHighestDistortion,
    /// Fail if any cell is empty.
    FailClosed,
}

/// Lloyd-Max refinement mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum LloydMode {
    /// Run multi-restart Lloyd-Max refinement (paper path).
    Refine,
    /// Skip refinement entirely — use the radial-angular initialization as-is.
    /// Faster codebook construction (~6ms saved) at the cost of higher MSE.
    /// The receipt will reflect this choice via `best_mse == init_mse`.
    Skip,
}

/// Stable profile for paper-faithful FibQuant codebooks and payloads.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FibQuantProfileV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Ambient vector dimension `d`.
    pub ambient_dim: u32,
    /// Block dimension `k`.
    pub block_dim: u32,
    /// Codebook size `N`.
    pub codebook_size: u32,
    /// Paper dense rate `log2(N) / k`.
    pub paper_rate_bits_per_coord: f64,
    /// Practical fixed-rate index width `ceil(log2(N))`.
    pub wire_index_bits: u8,
    /// Practical wire rate `wire_index_bits / k`.
    pub wire_bits_per_coord: f64,
    /// Norm header format.
    pub norm_format: NormFormat,
    /// Seed for ambient rotation.
    pub rotation_seed: u64,
    /// Rotation generation algorithm identity.
    pub rotation_algorithm_version: String,
    /// Seed for codebook construction and Lloyd training.
    pub codebook_seed: u64,
    /// Codebook algorithm/version string.
    pub codebook_version: String,
    /// Training source mode.
    pub source_mode: SourceMode,
    /// Radius method.
    pub radius_method: RadiusMethod,
    /// Direction method.
    pub direction_method: DirectionMethod,
    /// Number of Lloyd restarts.
    pub lloyd_restarts: u32,
    /// Number of Lloyd iterations per restart.
    pub lloyd_iterations: u32,
    /// Number of training samples.
    pub training_samples: u32,
    /// Empty-cell repair policy.
    pub empty_cell_policy: EmptyCellPolicy,
    /// Lloyd-Max refinement mode: Refine (paper path) or Skip (fast path).
    pub lloyd_mode: LloydMode,
}

impl FibQuantProfileV1 {
    /// Build a validated paper profile with method choices derived from `k`.
    pub fn paper_default(
        ambient_dim: usize,
        block_dim: usize,
        codebook_size: usize,
        seed: u64,
    ) -> Result<Self> {
        validate_profile_parts(ambient_dim, block_dim, codebook_size)?;
        let direction_method = match block_dim {
            2 => DirectionMethod::FibonacciSpiral,
            3 => DirectionMethod::FibonacciSphere,
            _ => DirectionMethod::RobertsKronecker,
        };
        let radius_method = if block_dim == 2 {
            RadiusMethod::K2ClosedForm
        } else {
            RadiusMethod::BetaQuantile
        };
        let wire_index_bits = wire_index_bits(codebook_size)?;
        let profile = Self {
            schema_version: PROFILE_SCHEMA.into(),
            ambient_dim: ambient_dim as u32,
            block_dim: block_dim as u32,
            codebook_size: codebook_size as u32,
            paper_rate_bits_per_coord: (codebook_size as f64).log2() / block_dim as f64,
            wire_index_bits,
            wire_bits_per_coord: f64::from(wire_index_bits) / block_dim as f64,
            norm_format: NormFormat::Fp16Paper,
            rotation_seed: seed,
            rotation_algorithm_version: ROTATION_ALGORITHM_VERSION.into(),
            codebook_seed: seed.wrapping_add(0x9e37_79b9_7f4a_7c15),
            codebook_version: "fib-quant:paper-core-v1".into(),
            source_mode: SourceMode::CanonicalSphericalBeta,
            radius_method,
            direction_method,
            lloyd_restarts: 4,
            lloyd_iterations: 25,
            training_samples: default_training_samples(codebook_size)?,
            empty_cell_policy: EmptyCellPolicy::SplitHighestDistortion,
            lloyd_mode: LloydMode::Refine,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Validate the complete profile.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PROFILE_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "profile schema_version {}, expected {PROFILE_SCHEMA}",
                self.schema_version
            )));
        }
        validate_profile_parts(
            self.ambient_dim as usize,
            self.block_dim as usize,
            self.codebook_size as usize,
        )?;
        validate_resource_bounds(
            self.ambient_dim as usize,
            self.block_dim as usize,
            self.codebook_size as usize,
            self.training_samples,
            self.wire_index_bits,
        )?;
        if self.norm_format != NormFormat::Fp16Paper {
            return Err(FibQuantError::CorruptPayload(
                "paper profile requires fp16 norm side header".into(),
            ));
        }
        if self.source_mode != SourceMode::CanonicalSphericalBeta {
            return Err(FibQuantError::CorruptPayload(
                "paper profile requires canonical spherical-Beta source mode".into(),
            ));
        }
        if self.rotation_algorithm_version != ROTATION_ALGORITHM_VERSION {
            return Err(FibQuantError::CorruptPayload(format!(
                "rotation_algorithm_version {}, expected {ROTATION_ALGORITHM_VERSION}",
                self.rotation_algorithm_version
            )));
        }
        let expected_bits = wire_index_bits(self.codebook_size as usize)?;
        if self.wire_index_bits != expected_bits {
            return Err(FibQuantError::CorruptPayload(format!(
                "wire_index_bits {} does not match ceil(log2(N)) {expected_bits}",
                self.wire_index_bits
            )));
        }
        let k = self.block_dim as usize;
        let expected_paper_rate = (self.codebook_size as f64).log2() / k as f64;
        validate_rate(
            "paper_rate_bits_per_coord",
            self.paper_rate_bits_per_coord,
            expected_paper_rate,
        )?;
        let expected_wire_rate = f64::from(self.wire_index_bits) / k as f64;
        validate_rate(
            "wire_bits_per_coord",
            self.wire_bits_per_coord,
            expected_wire_rate,
        )?;
        validate_method_pair(k, &self.radius_method, &self.direction_method)?;
        if self.lloyd_restarts == 0 || self.lloyd_restarts > MAX_LLOYD_RESTARTS {
            return Err(FibQuantError::CorruptPayload(format!(
                "lloyd_restarts {} outside supported range 1..={MAX_LLOYD_RESTARTS}",
                self.lloyd_restarts
            )));
        }
        if self.lloyd_iterations == 0 || self.lloyd_iterations > MAX_LLOYD_ITERATIONS {
            return Err(FibQuantError::CorruptPayload(format!(
                "lloyd_iterations {} outside supported range 1..={MAX_LLOYD_ITERATIONS}",
                self.lloyd_iterations
            )));
        }
        if self.training_samples < self.codebook_size
            || self.training_samples > MAX_TRAINING_SAMPLES
        {
            return Err(FibQuantError::CorruptPayload(format!(
                "training_samples {} outside supported range {}..={MAX_TRAINING_SAMPLES}",
                self.training_samples, self.codebook_size
            )));
        }
        Ok(())
    }

    /// Stable digest over all explicit profile fields.
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        json_digest(PROFILE_SCHEMA, self)
    }

    /// Number of `k`-blocks per vector.
    pub fn block_count(&self) -> u32 {
        self.ambient_dim / self.block_dim
    }
}

/// Return the fixed wire width for one index in `[0, N)`.
pub fn wire_index_bits(codebook_size: usize) -> Result<u8> {
    if codebook_size < 2 {
        return Err(FibQuantError::InvalidCodebookSize(codebook_size));
    }
    let bits = usize::BITS - (codebook_size - 1).leading_zeros();
    u8::try_from(bits).map_err(|_| FibQuantError::InvalidCodebookSize(codebook_size))
}

fn validate_profile_parts(
    ambient_dim: usize,
    block_dim: usize,
    codebook_size: usize,
) -> Result<()> {
    if ambient_dim == 0 {
        return Err(FibQuantError::ZeroDimension);
    }
    if block_dim == 0 || block_dim > ambient_dim {
        return Err(FibQuantError::InvalidBlockDim {
            ambient_dim,
            block_dim,
        });
    }
    if ambient_dim == block_dim {
        return Err(FibQuantError::InvalidBlockDim {
            ambient_dim,
            block_dim,
        });
    }
    if ambient_dim % block_dim != 0 {
        return Err(FibQuantError::DimensionNotDivisible {
            ambient_dim,
            block_dim,
        });
    }
    if ambient_dim > MAX_AMBIENT_DIM {
        return Err(FibQuantError::ResourceLimitExceeded(format!(
            "ambient_dim {ambient_dim} exceeds MAX_AMBIENT_DIM {MAX_AMBIENT_DIM}"
        )));
    }
    if block_dim > MAX_BLOCK_DIM {
        return Err(FibQuantError::ResourceLimitExceeded(format!(
            "block_dim {block_dim} exceeds MAX_BLOCK_DIM {MAX_BLOCK_DIM}"
        )));
    }
    if !(2..=MAX_CODEBOOK_SIZE).contains(&codebook_size) {
        return Err(FibQuantError::InvalidCodebookSize(codebook_size));
    }
    Ok(())
}

fn default_training_samples(codebook_size: usize) -> Result<u32> {
    let samples = 30usize
        .checked_mul(codebook_size)
        .ok_or_else(|| FibQuantError::ResourceLimitExceeded("30 * codebook_size overflow".into()))?
        .max(256)
        .min(MAX_TRAINING_SAMPLES as usize);
    u32::try_from(samples)
        .map_err(|_| FibQuantError::ResourceLimitExceeded("training sample count overflow".into()))
}

fn checked_profile_mul(lhs: usize, rhs: usize, label: &str) -> Result<usize> {
    lhs.checked_mul(rhs)
        .ok_or_else(|| FibQuantError::ResourceLimitExceeded(format!("{label} overflow")))
}

fn validate_resource_bounds(
    ambient_dim: usize,
    block_dim: usize,
    codebook_size: usize,
    training_samples: u32,
    wire_index_bits: u8,
) -> Result<()> {
    let rotation_values =
        checked_profile_mul(ambient_dim, ambient_dim, "ambient_dim * ambient_dim")?;
    if rotation_values > MAX_ROTATION_MATRIX_VALUES {
        return Err(FibQuantError::ResourceLimitExceeded(format!(
            "rotation matrix values {rotation_values} exceed MAX_ROTATION_MATRIX_VALUES {MAX_ROTATION_MATRIX_VALUES}"
        )));
    }

    let codebook_values =
        checked_profile_mul(codebook_size, block_dim, "codebook_size * block_dim")?;
    if codebook_values > MAX_CODEBOOK_VALUES {
        return Err(FibQuantError::ResourceLimitExceeded(format!(
            "codebook values {codebook_values} exceed MAX_CODEBOOK_VALUES {MAX_CODEBOOK_VALUES}"
        )));
    }

    checked_profile_mul(
        training_samples as usize,
        block_dim,
        "training_samples * block_dim",
    )?;

    let block_count = ambient_dim / block_dim;
    let packed_bits = checked_profile_mul(
        block_count,
        wire_index_bits as usize,
        "block_count * wire_index_bits",
    )?;
    if packed_bits > MAX_PACKED_INDEX_BITS {
        return Err(FibQuantError::ResourceLimitExceeded(format!(
            "packed index bits {packed_bits} exceed MAX_PACKED_INDEX_BITS {MAX_PACKED_INDEX_BITS}"
        )));
    }
    Ok(())
}

fn validate_rate(name: &str, actual: f64, expected: f64) -> Result<()> {
    if !actual.is_finite() || !expected.is_finite() || (actual - expected).abs() > RATE_TOLERANCE {
        return Err(FibQuantError::CorruptPayload(format!(
            "{name} {actual} does not match expected {expected}"
        )));
    }
    Ok(())
}

fn validate_method_pair(
    block_dim: usize,
    radius: &RadiusMethod,
    direction: &DirectionMethod,
) -> Result<()> {
    let valid = match block_dim {
        2 => {
            radius == &RadiusMethod::K2ClosedForm && direction == &DirectionMethod::FibonacciSpiral
        }
        3 => {
            radius == &RadiusMethod::BetaQuantile && direction == &DirectionMethod::FibonacciSphere
        }
        _ => {
            radius == &RadiusMethod::BetaQuantile && direction == &DirectionMethod::RobertsKronecker
        }
    };
    if valid {
        Ok(())
    } else {
        Err(FibQuantError::CorruptPayload(format!(
            "unsupported radius/direction pair for k={block_dim}: {radius:?}/{direction:?}"
        )))
    }
}
