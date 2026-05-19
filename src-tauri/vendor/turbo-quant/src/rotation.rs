//! Random rotation matrices for whitening high-dimensional vectors before quantization.
//!
//! Rotating the data before quantization simplifies the geometry, making uniform
//! scalar quantizers easier to use as deterministic baselines.
//!
//! Two implementations are provided:
//! - [`StoredRotation`]: full d×d orthogonal matrix via QR decomposition. Correct
//!   for any dimension but uses O(d²) memory (~9MB at d=1536).
//! - [`FastHadamardRotation`]: deterministic sign rotation followed by a
//!   normalized Hadamard transform for power-of-two dimensions.

use crate::error::{Result, TurboQuantError};
use nalgebra::DMatrix;
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;
use rand_distr::{Distribution, StandardNormal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Rotation selection policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum RotationKind {
    /// Use FastHadamard for supported dimensions, otherwise Stored QR.
    Auto,
    /// Use deterministic Hadamard/SRHT-style rotation. Requires power-of-two dimensions.
    FastHadamard,
    /// Use dense QR reference rotation.
    StoredQr,
}

impl RotationKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::FastHadamard => "fast_hadamard",
            Self::StoredQr => "stored_qr_reference",
        }
    }
}

/// Concrete rotation backend used by quantizers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RotationBackend {
    FastHadamard(FastHadamardRotation),
    StoredQr(StoredRotation),
}

impl RotationBackend {
    pub fn new(dim: usize, seed: u64, kind: RotationKind) -> Result<Self> {
        match kind {
            RotationKind::Auto if dim.is_power_of_two() => {
                FastHadamardRotation::new(dim, seed).map(Self::FastHadamard)
            }
            RotationKind::Auto => StoredRotation::new(dim, seed).map(Self::StoredQr),
            RotationKind::FastHadamard => {
                FastHadamardRotation::new(dim, seed).map(Self::FastHadamard)
            }
            RotationKind::StoredQr => StoredRotation::new(dim, seed).map(Self::StoredQr),
        }
    }

    pub fn kind(&self) -> RotationKind {
        match self {
            Self::FastHadamard(_) => RotationKind::FastHadamard,
            Self::StoredQr(_) => RotationKind::StoredQr,
        }
    }

    pub fn kind_label(&self) -> &'static str {
        self.kind().label()
    }

    pub fn seed(&self) -> u64 {
        match self {
            Self::FastHadamard(rotation) => rotation.seed(),
            Self::StoredQr(rotation) => rotation.seed(),
        }
    }
}

impl Rotation for RotationBackend {
    fn dim(&self) -> usize {
        match self {
            Self::FastHadamard(rotation) => rotation.dim(),
            Self::StoredQr(rotation) => rotation.dim(),
        }
    }

    fn apply(&self, input: &[f32], output: &mut [f32]) -> Result<()> {
        match self {
            Self::FastHadamard(rotation) => rotation.apply(input, output),
            Self::StoredQr(rotation) => rotation.apply(input, output),
        }
    }

    fn apply_inverse(&self, input: &[f32], output: &mut [f32]) -> Result<()> {
        match self {
            Self::FastHadamard(rotation) => rotation.apply_inverse(input, output),
            Self::StoredQr(rotation) => rotation.apply_inverse(input, output),
        }
    }
}

/// A rotation that can be applied to and inverted on vectors of a fixed dimension.
pub trait Rotation: Send + Sync {
    /// The dimension this rotation operates on.
    fn dim(&self) -> usize;

    /// Apply the rotation: y = R · x.
    ///
    /// `input` and `output` must both have length `dim()`.
    fn apply(&self, input: &[f32], output: &mut [f32]) -> Result<()>;

    /// Apply the inverse (transpose) rotation: x = Rᵀ · y.
    ///
    /// For orthogonal matrices, the inverse equals the transpose.
    fn apply_inverse(&self, input: &[f32], output: &mut [f32]) -> Result<()>;
}

/// Deterministic Hadamard/SRHT-style rotation for power-of-two dimensions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FastHadamardRotation {
    dim: usize,
    seed: u64,
    signs: Vec<f32>,
}

impl FastHadamardRotation {
    pub fn new(dim: usize, seed: u64) -> Result<Self> {
        if dim == 0 {
            return Err(TurboQuantError::ZeroDimension);
        }
        if !dim.is_power_of_two() {
            return Err(TurboQuantError::RotationFailed {
                reason: format!("Hadamard rotation requires a power-of-two dimension, got {dim}"),
            });
        }
        let mut rng = ChaCha8Rng::seed_from_u64(seed.wrapping_add(0xA11C_E55E_D5A5_EED5));
        let signs = (0..dim)
            .map(|_| if rng.gen::<bool>() { 1.0 } else { -1.0 })
            .collect();
        Ok(Self { dim, seed, signs })
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }
}

impl Rotation for FastHadamardRotation {
    fn dim(&self) -> usize {
        self.dim
    }

    fn apply(&self, input: &[f32], output: &mut [f32]) -> Result<()> {
        check_dim(input.len(), self.dim)?;
        check_dim(output.len(), self.dim)?;
        for ((out, value), sign) in output.iter_mut().zip(input.iter()).zip(self.signs.iter()) {
            *out = value * sign;
        }
        fwht_normalized(output);
        Ok(())
    }

    fn apply_inverse(&self, input: &[f32], output: &mut [f32]) -> Result<()> {
        check_dim(input.len(), self.dim)?;
        check_dim(output.len(), self.dim)?;
        output.copy_from_slice(input);
        fwht_normalized(output);
        for (out, sign) in output.iter_mut().zip(self.signs.iter()) {
            *out *= sign;
        }
        Ok(())
    }
}

/// A full d×d orthogonal rotation matrix generated via QR decomposition of a
/// random Gaussian matrix.
///
/// Seeded deterministically so that quantizer state can be serialized and
/// reconstructed without storing the matrix itself — only the seed and dimension
/// need to be persisted.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredRotation {
    dim: usize,
    seed: u64,
    /// Row-major flat storage of the d×d orthogonal matrix.
    #[serde(with = "matrix_serde")]
    matrix: DMatrix<f32>,
}

impl StoredRotation {
    /// Generate a new rotation for vectors of dimension `dim` using `seed`.
    ///
    /// The same `(dim, seed)` pair always produces the same rotation, making
    /// this suitable for deterministic, reproducible compression pipelines.
    pub fn new(dim: usize, seed: u64) -> Result<Self> {
        if dim == 0 {
            return Err(TurboQuantError::ZeroDimension);
        }

        let matrix = generate_orthogonal(dim, seed)?;
        Ok(Self { dim, seed, matrix })
    }

    /// The seed used to generate this rotation.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Approximate memory used by the stored matrix in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.dim * self.dim * std::mem::size_of::<f32>()
    }
}

impl Rotation for StoredRotation {
    fn dim(&self) -> usize {
        self.dim
    }

    fn apply(&self, input: &[f32], output: &mut [f32]) -> Result<()> {
        check_dim(input.len(), self.dim)?;
        check_dim(output.len(), self.dim)?;

        // y = R · x  (matrix is stored column-major in nalgebra)
        for (i, out) in output.iter_mut().enumerate() {
            *out = self
                .matrix
                .row(i)
                .iter()
                .zip(input)
                .map(|(r, x)| r * x)
                .sum();
        }
        Ok(())
    }

    fn apply_inverse(&self, input: &[f32], output: &mut [f32]) -> Result<()> {
        check_dim(input.len(), self.dim)?;
        check_dim(output.len(), self.dim)?;

        // x = Rᵀ · y  — for orthogonal R, R⁻¹ = Rᵀ
        for (i, out) in output.iter_mut().enumerate() {
            *out = self
                .matrix
                .column(i)
                .iter()
                .zip(input)
                .map(|(r, y)| r * y)
                .sum();
        }
        Ok(())
    }
}

/// Generate a d×d orthogonal matrix via QR decomposition of a random Gaussian
/// matrix. The resulting Q is Haar-distributed (uniformly random orthogonal).
fn generate_orthogonal(dim: usize, seed: u64) -> Result<DMatrix<f32>> {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);
    let dist = StandardNormal;

    // Sample a d×d matrix with i.i.d. N(0,1) entries.
    let data: Vec<f32> = (0..dim * dim).map(|_| dist.sample(&mut rng)).collect();

    // nalgebra is column-major; DMatrix::from_vec(rows, cols, data) fills column by column.
    let m = DMatrix::from_vec(dim, dim, data);

    let qr = m.qr();
    let q = qr.q();

    // QR decomposition can return Q with det = -1. Fix the sign to ensure det = +1
    // (a proper rotation rather than an improper one with reflection).
    let r = qr.r();
    let signs: Vec<f32> = (0..dim)
        .map(|i| if r[(i, i)] >= 0.0 { 1.0 } else { -1.0 })
        .collect();

    let mut corrected = q;
    for (j, &s) in signs.iter().enumerate() {
        if s < 0.0 {
            for i in 0..dim {
                corrected[(i, j)] *= -1.0;
            }
        }
    }

    Ok(corrected)
}

fn check_dim(got: usize, expected: usize) -> Result<()> {
    if got != expected {
        return Err(TurboQuantError::DimensionMismatch { expected, got });
    }
    Ok(())
}

fn fwht_normalized(values: &mut [f32]) {
    let n = values.len();
    let mut step = 1;
    while step < n {
        let block = step * 2;
        for start in (0..n).step_by(block) {
            for offset in 0..step {
                let a = values[start + offset];
                let b = values[start + offset + step];
                values[start + offset] = a + b;
                values[start + offset + step] = a - b;
            }
        }
        step = block;
    }
    let scale = (n as f32).sqrt().recip();
    for value in values {
        *value *= scale;
    }
}

mod matrix_serde {
    use nalgebra::DMatrix;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    struct MatrixProxy {
        rows: usize,
        cols: usize,
        data: Vec<f32>,
    }

    pub fn serialize<S: Serializer>(
        m: &DMatrix<f32>,
        s: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        MatrixProxy {
            rows: m.nrows(),
            cols: m.ncols(),
            data: m.as_slice().to_vec(),
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        d: D,
    ) -> std::result::Result<DMatrix<f32>, D::Error> {
        let p = MatrixProxy::deserialize(d)?;
        Ok(DMatrix::from_vec(p.rows, p.cols, p.data))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rotation_is_deterministic_for_same_seed() {
        let r1 = StoredRotation::new(8, 42).unwrap();
        let r2 = StoredRotation::new(8, 42).unwrap();
        assert_eq!(r1.matrix.as_slice(), r2.matrix.as_slice());
    }

    #[test]
    fn rotation_differs_across_seeds() {
        let r1 = StoredRotation::new(8, 1).unwrap();
        let r2 = StoredRotation::new(8, 2).unwrap();
        assert_ne!(r1.matrix.as_slice(), r2.matrix.as_slice());
    }

    #[test]
    fn rotation_is_orthogonal_rrt_equals_identity() {
        let r = StoredRotation::new(16, 7).unwrap();
        let m = &r.matrix;
        let product = m.transpose() * m;
        for i in 0..16 {
            for j in 0..16 {
                let expected = if i == j { 1.0f32 } else { 0.0f32 };
                let got = product[(i, j)];
                assert!(
                    (got - expected).abs() < 1e-5,
                    "RᵀR[{i},{j}] = {got}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn apply_inverse_recovers_input() {
        let r = StoredRotation::new(8, 99).unwrap();
        let x = vec![1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mut y = vec![0.0f32; 8];
        let mut recovered = vec![0.0f32; 8];

        r.apply(&x, &mut y).unwrap();
        r.apply_inverse(&y, &mut recovered).unwrap();

        for (orig, rec) in x.iter().zip(recovered.iter()) {
            assert!((orig - rec).abs() < 1e-5, "orig={orig}, recovered={rec}");
        }
    }

    #[test]
    fn rotation_preserves_inner_products() {
        // For orthogonal R: <Rx, Ry> = <x, y>
        let r = StoredRotation::new(8, 13).unwrap();
        let x = vec![1.0f32, 0.5, -1.0, 2.0, 0.1, -0.3, 1.5, 0.8];
        let y = vec![0.2f32, -1.0, 0.5, 1.0, -0.5, 0.3, 0.9, -0.7];
        let mut rx = vec![0.0f32; 8];
        let mut ry = vec![0.0f32; 8];

        r.apply(&x, &mut rx).unwrap();
        r.apply(&y, &mut ry).unwrap();

        let ip_original: f32 = x.iter().zip(y.iter()).map(|(a, b)| a * b).sum();
        let ip_rotated: f32 = rx.iter().zip(ry.iter()).map(|(a, b)| a * b).sum();

        assert!((ip_original - ip_rotated).abs() < 1e-4);
    }

    #[test]
    fn zero_dimension_is_rejected() {
        assert!(StoredRotation::new(0, 0).is_err());
    }

    #[test]
    fn serialization_roundtrip() {
        let r = StoredRotation::new(8, 55).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let restored: StoredRotation = serde_json::from_str(&json).unwrap();
        assert_eq!(r.matrix.as_slice(), restored.matrix.as_slice());
    }
}
