//! PolarQuant: high-efficiency vector compression via polar coordinate encoding.
//!
//! PolarQuant converts Cartesian pairs into polar form (radius, angle), then
//! uniformly quantizes angles. Because the rotation stage whitens the data,
//! angles are uniformly distributed on [−π, π], making uniform quantization
//! a profile-defined alternative to trained-codebook calibration.
//!
//! # Algorithm
//!
//! Given a d-dimensional (even d) vector x after rotation y = R·x:
//!
//! 1. Group into d/2 pairs: (y₀, y₁), (y₂, y₃), …
//! 2. Convert each pair to polar: rᵢ = √(y₂ᵢ² + y₂ᵢ₊₁²), θᵢ = atan2(y₂ᵢ₊₁, y₂ᵢ)
//! 3. Quantize each θᵢ to `bits` levels uniformly on [−π, π]
//! 4. Store radii as f32 and bitpack quantized angle indices
//!
//! For approximate nearest-neighbor search, exact reconstruction is not required.
//! The inner product estimator operates directly on polar codes, avoiding
//! the decode round-trip entirely.

use std::f32::consts::PI;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    bitpack,
    error::{Result, TurboQuantError},
    rotation::{Rotation, RotationBackend, RotationKind},
};

/// A compressed representation of a single vector in polar form.
///
/// The `radii` array has length d/2, and `packed_angle_indices` stores d/2
/// logical angle indices using exactly `bits` bits per index. Angle indices
/// are in [0, 2^bits).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PolarCode {
    /// Original vector dimension (must be even).
    pub dim: usize,
    /// Number of bits used to quantize each angle.
    pub bits: u8,
    /// Per-pair radii (f32, lossless).
    pub radii: Vec<f32>,
    /// Bitpacked quantized angle indices in [0, 2^bits).
    pub packed_angle_indices: Vec<u8>,
}

impl PolarCode {
    /// Number of pairs (= dim / 2).
    pub fn pair_count(&self) -> usize {
        self.dim / 2
    }

    /// Build a packed code from logical angle indices.
    pub fn from_parts(
        dim: usize,
        bits: u8,
        radii: Vec<f32>,
        angle_indices: &[u16],
    ) -> Result<Self> {
        let packed_angle_indices = bitpack::pack_indices(angle_indices, bits)?;
        let code = Self {
            dim,
            bits,
            radii,
            packed_angle_indices,
        };
        code.validate_for(dim, bits)?;
        Ok(code)
    }

    /// Return the logical angle index for pair `i`.
    pub fn angle_index(&self, i: usize) -> Result<u16> {
        if i >= self.pair_count() {
            return Err(TurboQuantError::MalformedCode {
                reason: format!(
                    "angle index {i} is outside pair count {}",
                    self.pair_count()
                ),
            });
        }
        Ok(bitpack::unpack_indices(&self.packed_angle_indices, self.pair_count(), self.bits)?[i])
    }

    /// Unpack all logical angle indices.
    pub fn angle_indices(&self) -> Result<Vec<u16>> {
        bitpack::unpack_indices(&self.packed_angle_indices, self.pair_count(), self.bits)
    }

    /// Reconstruct the dequantized angle for pair `i` in radians ∈ [−π, π).
    pub fn dequantize_angle(&self, i: usize) -> Result<f32> {
        let levels = 1u32 << self.bits;
        let idx = self.angle_index(i)? as f32;
        Ok((idx / levels as f32) * (2.0 * PI) - PI)
    }

    /// Serialized payload bytes used by this code.
    pub fn encoded_bytes(&self) -> usize {
        self.radii.len() * std::mem::size_of::<f32>() + self.packed_angle_indices.len()
    }

    /// Validate this code against an expected profile.
    pub fn validate_for(&self, dim: usize, bits: u8) -> Result<()> {
        if self.dim != dim {
            return Err(TurboQuantError::DimensionMismatch {
                expected: dim,
                got: self.dim,
            });
        }
        if self.bits != bits {
            return Err(TurboQuantError::MalformedCode {
                reason: format!("code has bits={}, expected {bits}", self.bits),
            });
        }
        if dim == 0 || dim % 2 != 0 {
            return Err(TurboQuantError::MalformedCode {
                reason: format!("code dimension must be positive and even, got {dim}"),
            });
        }
        let pairs = dim / 2;
        if self.radii.len() != pairs {
            return Err(TurboQuantError::MalformedCode {
                reason: format!("code has {} radii, expected {pairs}", self.radii.len()),
            });
        }
        for (index, radius) in self.radii.iter().enumerate() {
            if !radius.is_finite() || *radius < 0.0 {
                return Err(TurboQuantError::MalformedCode {
                    reason: format!("radius {index} is not finite and non-negative"),
                });
            }
        }
        bitpack::unpack_indices(&self.packed_angle_indices, pairs, bits)?;
        Ok(())
    }
}

/// Encodes and decodes vectors using PolarQuant.
///
/// The quantizer owns a selected rotation backend that is applied before
/// encoding. The rotation is seeded deterministically, so the profile can
/// record `(dim, seed, bits, rotation_kind)`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PolarQuantizer {
    dim: usize,
    bits: u8,
    rotation: RotationBackend,
}

/// Query state prepared once for scoring multiple PolarQuant codes.
#[derive(Debug, Clone, PartialEq)]
pub struct PolarProjectedQuery {
    rotated_query: Vec<f32>,
}

impl PolarQuantizer {
    /// Create a new quantizer for vectors of dimension `dim`.
    ///
    /// - `bits`: angle quantization levels (1–16). Higher values generally
    ///   reduce angle quantization error and increase storage.
    /// - `seed`: controls the random rotation. Identical `(dim, bits, seed)`
    ///   always produces an identical quantizer.
    pub fn new(dim: usize, bits: u8, seed: u64) -> Result<Self> {
        if dim == 0 {
            return Err(TurboQuantError::ZeroDimension);
        }
        if dim % 2 != 0 {
            return Err(TurboQuantError::OddDimension { got: dim });
        }
        if bits == 0 || bits > 16 {
            return Err(TurboQuantError::InvalidBitWidth { got: bits });
        }
        Self::new_with_rotation(dim, bits, seed, RotationKind::Auto)
    }

    /// Create a quantizer with an explicit rotation policy.
    pub fn new_with_rotation(
        dim: usize,
        bits: u8,
        seed: u64,
        rotation_kind: RotationKind,
    ) -> Result<Self> {
        if dim == 0 {
            return Err(TurboQuantError::ZeroDimension);
        }
        if dim % 2 != 0 {
            return Err(TurboQuantError::OddDimension { got: dim });
        }
        if bits == 0 || bits > 16 {
            return Err(TurboQuantError::InvalidBitWidth { got: bits });
        }
        let rotation = RotationBackend::new(dim, seed, rotation_kind)?;
        Ok(Self {
            dim,
            bits,
            rotation,
        })
    }

    /// Create a quantizer using dense QR reference rotation.
    pub fn new_with_stored_rotation(dim: usize, bits: u8, seed: u64) -> Result<Self> {
        Self::new_with_rotation(dim, bits, seed, RotationKind::StoredQr)
    }

    /// The vector dimension this quantizer operates on.
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Angle quantization bit width.
    pub fn bits(&self) -> u8 {
        self.bits
    }

    /// Resolved rotation backend.
    pub fn rotation_kind(&self) -> RotationKind {
        self.rotation.kind()
    }

    /// Resolved rotation backend label for profiles and receipts.
    pub fn rotation_kind_label(&self) -> &'static str {
        self.rotation.kind_label()
    }

    /// Encode a vector into a [`PolarCode`].
    ///
    /// `vector` must have length `dim`.
    pub fn encode(&self, vector: &[f32]) -> Result<PolarCode> {
        self.check_input_dim(vector.len())?;
        check_finite_vector(vector)?;

        let mut rotated = vec![0.0f32; self.dim];
        self.rotation.apply(vector, &mut rotated)?;

        let pairs = self.dim / 2;
        let mut radii = Vec::with_capacity(pairs);
        let mut angle_indices = Vec::with_capacity(pairs);

        for i in 0..pairs {
            let a = rotated[2 * i];
            let b = rotated[2 * i + 1];
            let (r, idx) = encode_pair(a, b, self.bits);
            radii.push(r);
            angle_indices.push(idx);
        }

        PolarCode::from_parts(self.dim, self.bits, radii, &angle_indices)
    }

    /// Decode a [`PolarCode`] back to an approximate vector.
    ///
    /// The result is the nearest-neighbor reconstruction in the rotated space,
    /// then inverse-rotated back. Reconstruction error depends on `bits`.
    pub fn decode(&self, code: &PolarCode) -> Result<Vec<f32>> {
        self.validate_code(code)?;

        let mut rotated = vec![0.0f32; self.dim];
        let pairs = self.dim / 2;

        for i in 0..pairs {
            let theta = code.dequantize_angle(i)?;
            let r = code.radii[i];
            rotated[2 * i] = r * theta.cos();
            rotated[2 * i + 1] = r * theta.sin();
        }

        let mut output = vec![0.0f32; self.dim];
        self.rotation.apply_inverse(&rotated, &mut output)?;
        Ok(output)
    }

    /// Estimate the inner product ⟨query, encoded_vector⟩ without decoding.
    ///
    /// This is the core operation for approximate nearest-neighbor search.
    /// The query is rotated, then each rotated pair is compared to the stored
    /// polar code using the identity:
    ///
    /// ```text
    /// ⟨q_pair, k_pair⟩ ≈ r_k · (q_pair · [cos θ_k, sin θ_k])
    ///                   = r_k · (q_a cos θ_k + q_b sin θ_k)
    /// ```
    ///
    /// Summing over all pairs gives the full inner product estimate.
    pub fn inner_product_estimate(&self, code: &PolarCode, query: &[f32]) -> Result<f32> {
        let projected = self.project_query(query)?;
        self.inner_product_estimate_with_projected_query(code, &projected)
    }

    /// Rotate a query once so it can score multiple codes without repeated allocation.
    pub fn project_query(&self, query: &[f32]) -> Result<PolarProjectedQuery> {
        self.check_input_dim(query.len())?;
        check_finite_vector(query)?;
        let mut rotated_query = vec![0.0f32; self.dim];
        self.rotation.apply(query, &mut rotated_query)?;
        check_finite_vector(&rotated_query)?;
        Ok(PolarProjectedQuery { rotated_query })
    }

    /// Estimate inner product using a pre-rotated query.
    pub fn inner_product_estimate_with_projected_query(
        &self,
        code: &PolarCode,
        query: &PolarProjectedQuery,
    ) -> Result<f32> {
        self.validate_code(code)?;

        let pairs = self.dim / 2;
        let mut estimate = 0.0f32;

        for i in 0..pairs {
            let theta = code.dequantize_angle(i)?;
            let r = code.radii[i];
            let q_a = query.rotated_query[2 * i];
            let q_b = query.rotated_query[2 * i + 1];
            estimate += r * (q_a * theta.cos() + q_b * theta.sin());
        }

        if !estimate.is_finite() {
            return Err(TurboQuantError::MalformedCode {
                reason: "polar score is not finite".into(),
            });
        }
        Ok(estimate)
    }

    /// Compute the squared L2 distance estimate between query and encoded vector.
    ///
    /// Uses the identity: ||x - y||² = ||x||² + ||y||² - 2⟨x, y⟩.
    /// The query's squared norm is computed exactly; the encoded vector's norm
    /// is derived from the stored radii (lossless since radii are stored as f32).
    pub fn l2_distance_estimate(&self, code: &PolarCode, query: &[f32]) -> Result<f32> {
        let ip = self.inner_product_estimate(code, query)?;

        let query_norm_sq: f32 = query.iter().map(|x| x * x).sum();
        let code_norm_sq: f32 = code.radii.iter().map(|r| r * r).sum();

        Ok((query_norm_sq + code_norm_sq - 2.0 * ip).max(0.0))
    }

    fn check_input_dim(&self, got: usize) -> Result<()> {
        if got != self.dim {
            return Err(TurboQuantError::DimensionMismatch {
                expected: self.dim,
                got,
            });
        }
        Ok(())
    }

    fn validate_code(&self, code: &PolarCode) -> Result<()> {
        code.validate_for(self.dim, self.bits)
    }
}

fn check_finite_vector(vector: &[f32]) -> Result<()> {
    if let Some((index, _)) = vector
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite())
    {
        return Err(TurboQuantError::NonFiniteInput { index });
    }
    Ok(())
}

/// Encode a Cartesian pair (a, b) into (radius, quantized_angle_index).
fn encode_pair(a: f32, b: f32, bits: u8) -> (f32, u16) {
    let r = (a * a + b * b).sqrt();
    let theta = b.atan2(a); // ∈ [−π, π]
    let levels = 1u32 << bits;
    // Map [−π, π] → [0, 1) → [0, levels)
    let normalized = (theta + PI) / (2.0 * PI);
    let idx = (normalized * levels as f32).floor() as u32 % levels;
    (r, idx as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit_vector(dim: usize, i: usize) -> Vec<f32> {
        let mut v = vec![0.0f32; dim];
        v[i] = 1.0;
        v
    }

    fn random_vector(dim: usize, seed: u64) -> Vec<f32> {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        use rand_distr::{Distribution, StandardNormal};
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        (0..dim).map(|_| StandardNormal.sample(&mut rng)).collect()
    }

    #[test]
    fn encode_decode_roundtrip_high_bits() {
        let q = PolarQuantizer::new(8, 16, 42).unwrap();
        let x = vec![1.0f32, 2.0, -1.5, 0.5, 3.0, -2.0, 0.1, -0.8];

        let code = q.encode(&x).unwrap();
        let decoded = q.decode(&code).unwrap();

        for (orig, dec) in x.iter().zip(decoded.iter()) {
            assert!(
                (orig - dec).abs() < 0.01,
                "orig={orig:.4}, decoded={dec:.4}"
            );
        }
    }

    #[test]
    fn inner_product_estimate_is_close_at_high_bits() {
        let q = PolarQuantizer::new(16, 16, 7).unwrap();
        let x = random_vector(16, 1);
        let y = random_vector(16, 2);

        let code = q.encode(&x).unwrap();
        let estimated = q.inner_product_estimate(&code, &y).unwrap();
        let exact: f32 = x.iter().zip(y.iter()).map(|(a, b)| a * b).sum();

        let relative_error = (estimated - exact).abs() / (exact.abs() + 1e-6);
        assert!(
            relative_error < 0.02,
            "relative error {relative_error:.4} too large: estimated={estimated:.4}, exact={exact:.4}"
        );
    }

    #[test]
    fn encoding_is_deterministic() {
        let q = PolarQuantizer::new(8, 8, 0).unwrap();
        let x = vec![1.0f32; 8];

        let c1 = q.encode(&x).unwrap();
        let c2 = q.encode(&x).unwrap();
        assert_eq!(c1.packed_angle_indices, c2.packed_angle_indices);
        assert_eq!(c1.radii, c2.radii);
    }

    #[test]
    fn zero_vector_has_zero_radius() {
        let q = PolarQuantizer::new(8, 8, 1).unwrap();
        let x = vec![0.0f32; 8];
        let code = q.encode(&x).unwrap();
        for r in &code.radii {
            assert!(*r < 1e-7, "expected zero radius, got {r}");
        }
    }

    #[test]
    fn unit_vectors_preserve_norm() {
        let q = PolarQuantizer::new(8, 16, 3).unwrap();
        for i in 0..8 {
            let x = unit_vector(8, i);
            let code = q.encode(&x).unwrap();
            let norm_sq: f32 = code.radii.iter().map(|r| r * r).sum();
            assert!((norm_sq - 1.0).abs() < 1e-5, "norm_sq={norm_sq}");
        }
    }

    #[test]
    fn nearest_neighbor_ordering_preserved_at_8bits() {
        let q = PolarQuantizer::new(16, 8, 42).unwrap();
        let query = random_vector(16, 99);

        // Create three database vectors: one close, two far.
        let close = {
            let mut v = query.clone();
            v.iter_mut().for_each(|x| *x += 0.01);
            v
        };
        let far1 = random_vector(16, 200);
        let far2 = random_vector(16, 201);

        let code_close = q.encode(&close).unwrap();
        let code_far1 = q.encode(&far1).unwrap();
        let code_far2 = q.encode(&far2).unwrap();

        let ip_close = q.inner_product_estimate(&code_close, &query).unwrap();
        let ip_far1 = q.inner_product_estimate(&code_far1, &query).unwrap();
        let ip_far2 = q.inner_product_estimate(&code_far2, &query).unwrap();

        assert!(
            ip_close > ip_far1 && ip_close > ip_far2,
            "close={ip_close:.3}, far1={ip_far1:.3}, far2={ip_far2:.3}"
        );
    }

    #[test]
    fn dimension_mismatch_is_rejected() {
        let q = PolarQuantizer::new(8, 8, 0).unwrap();
        let result = q.encode(&[1.0f32; 10]);
        assert!(result.is_err());
    }

    #[test]
    fn odd_dimension_is_rejected() {
        assert!(PolarQuantizer::new(7, 8, 0).is_err());
    }

    #[test]
    fn zero_bits_rejected() {
        assert!(PolarQuantizer::new(8, 0, 0).is_err());
    }

    #[test]
    fn code_serialization_roundtrip() {
        let q = PolarQuantizer::new(8, 8, 42).unwrap();
        let x = vec![1.0f32, -2.0, 0.5, 1.5, -0.3, 0.8, -1.0, 2.0];
        let code = q.encode(&x).unwrap();
        let json = serde_json::to_string(&code).unwrap();
        let restored: PolarCode = serde_json::from_str(&json).unwrap();
        assert_eq!(code, restored);
    }
}
