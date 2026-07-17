use nalgebra::DMatrix;
use rand::SeedableRng;
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, StandardNormal};
use serde::{Deserialize, Serialize};

use crate::{digest::json_digest, profile::MAX_ROTATION_MATRIX_VALUES, FibQuantError, Result};

/// Stable schema marker for deterministic stored rotations.
pub const ROTATION_SCHEMA: &str = "fib_rotation_v1";
/// Algorithm identity for the alpha QR/Gaussian rotation generator.
pub const ROTATION_ALGORITHM_VERSION: &str = "qr-gaussian-chacha8-sign-corrected-v1";

/// Stored deterministic orthogonal rotation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRotation {
    dim: usize,
    seed: u64,
    matrix: Vec<f64>,
}

impl StoredRotation {
    /// Generate a Haar-like orthogonal matrix via QR decomposition.
    pub fn new(dim: usize, seed: u64) -> Result<Self> {
        if dim == 0 {
            return Err(FibQuantError::ZeroDimension);
        }
        let matrix_values = dim.checked_mul(dim).ok_or_else(|| {
            FibQuantError::ResourceLimitExceeded("rotation matrix value count overflow".into())
        })?;
        if matrix_values > MAX_ROTATION_MATRIX_VALUES {
            return Err(FibQuantError::ResourceLimitExceeded(format!(
                "rotation matrix values {matrix_values} exceed MAX_ROTATION_MATRIX_VALUES {MAX_ROTATION_MATRIX_VALUES}"
            )));
        }
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        let data: Vec<f64> = (0..matrix_values)
            .map(|_| StandardNormal.sample(&mut rng))
            .collect();
        let m = DMatrix::from_vec(dim, dim, data);
        let qr = m.qr();
        let mut q = qr.q();
        let r = qr.r();
        for j in 0..dim {
            if r[(j, j)] < 0.0 {
                for i in 0..dim {
                    q[(i, j)] *= -1.0;
                }
            }
        }
        let mut matrix = vec![0.0; matrix_values];
        for row in 0..dim {
            for col in 0..dim {
                matrix[row * dim + col] = q[(row, col)];
            }
        }
        Ok(Self { dim, seed, matrix })
    }

    /// Dimension of this rotation.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Seed used for deterministic generation.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Stable rotation schema marker.
    pub fn rotation_schema(&self) -> &'static str {
        ROTATION_SCHEMA
    }

    /// Rotation algorithm identity.
    pub fn algorithm_version(&self) -> &'static str {
        ROTATION_ALGORITHM_VERSION
    }

    /// Deterministic digest over the rotation identity and matrix values.
    pub fn digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct RotationDigestView<'a> {
            rotation_schema: &'a str,
            algorithm_version: &'a str,
            dim: usize,
            seed: u64,
            matrix: &'a [f64],
        }

        json_digest(
            ROTATION_SCHEMA,
            &RotationDigestView {
                rotation_schema: ROTATION_SCHEMA,
                algorithm_version: ROTATION_ALGORITHM_VERSION,
                dim: self.dim,
                seed: self.seed,
                matrix: &self.matrix,
            },
        )
    }

    /// Apply `y = Pi x`.
    pub fn apply(&self, input: &[f64]) -> Result<Vec<f64>> {
        self.check_dim(input.len())?;
        let mut out = vec![0.0; self.dim];
        for (row, output) in out.iter_mut().enumerate().take(self.dim) {
            *output = self.matrix[row * self.dim..(row + 1) * self.dim]
                .iter()
                .zip(input)
                .map(|(a, b)| a * b)
                .sum();
        }
        Ok(out)
    }

    /// Apply inverse `x = Pi^T y`.
    pub fn apply_inverse(&self, input: &[f64]) -> Result<Vec<f64>> {
        self.check_dim(input.len())?;
        let mut out = vec![0.0; self.dim];
        for (col, output) in out.iter_mut().enumerate().take(self.dim) {
            let mut sum = 0.0;
            for (row, value) in input.iter().enumerate().take(self.dim) {
                sum += self.matrix[row * self.dim + col] * value;
            }
            *output = sum;
        }
        Ok(out)
    }

    /// Return the rotation matrix converted to f32. Useful for callers that
    /// need to reuse the converted matrix across many `apply_inverse_f32_with_matrix`
    /// calls without reconverting each time.
    pub fn matrix_f32(&self) -> Vec<f32> {
        self.matrix.iter().map(|&v| v as f32).collect()
    }

    /// Apply inverse `x = Pi^T y` in f32. Faster than the f64 version for
    /// batch decode of many small vectors, where the f32→f64 roundtrip is
    /// the bottleneck. Result is mathematically equivalent for the
    /// `as f32` final-cast case; intermediate precision loss is below the
    /// codebook quantization noise floor.
    pub fn apply_inverse_f32(&self, input: &[f32]) -> Result<Vec<f32>> {
        self.check_dim(input.len())?;
        let matrix_f32 = self.matrix_f32();
        self.apply_inverse_f32_with_matrix(input, &matrix_f32)
    }

    /// Apply inverse `x = Pi^T y` using a pre-converted f32 matrix. This
    /// avoids the f64→f32 conversion on every call, which is the bottleneck
    /// in `decode_batch_fast` when decoding thousands of codes that share
    /// the same rotation.
    pub fn apply_inverse_f32_with_matrix(
        &self,
        input: &[f32],
        matrix_f32: &[f32],
    ) -> Result<Vec<f32>> {
        self.check_dim(input.len())?;
        if matrix_f32.len() != self.dim * self.dim {
            return Err(FibQuantError::CorruptPayload(format!(
                "f32 matrix len {} != dim^2 {}",
                matrix_f32.len(),
                self.dim * self.dim
            )));
        }
        let dim = self.dim;
        let mut out = vec![0.0f32; dim];
        for col in 0..dim {
            let mut sum = 0.0f32;
            for row in 0..dim {
                sum += matrix_f32[row * dim + col] * input[row];
            }
            out[col] = sum;
        }
        Ok(out)
    }

    /// Apply inverse `x = Pi^T y` to a batch of inputs in one call. The
    /// matrix is converted to f32 once and reused across the batch.
    pub fn apply_inverse_batch_f32(&self, inputs: &[&[f32]]) -> Result<Vec<Vec<f32>>> {
        self.check_dim(inputs.first().map(|v| v.len()).unwrap_or(0))?;
        let dim = self.dim;
        // Cache the f32 matrix across all calls.
        let matrix_f32: Vec<f32> = self.matrix.iter().map(|&v| v as f32).collect();
        let mut out = Vec::with_capacity(inputs.len());
        for input in inputs {
            if input.len() != dim {
                return Err(FibQuantError::CorruptPayload(format!(
                    "input dim {} != rotation dim {}",
                    input.len(),
                    dim
                )));
            }
            let mut row = vec![0.0f32; dim];
            for col in 0..dim {
                let mut sum = 0.0f32;
                for r in 0..dim {
                    sum += matrix_f32[r * dim + col] * input[r];
                }
                row[col] = sum;
            }
            out.push(row);
        }
        Ok(out)
    }

    fn check_dim(&self, got: usize) -> Result<()> {
        if got != self.dim {
            return Err(FibQuantError::CorruptPayload(format!(
                "rotation expected dimension {}, got {got}",
                self.dim
            )));
        }
        Ok(())
    }
}
