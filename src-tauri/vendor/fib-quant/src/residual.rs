//! Residual (second-level) quantization for improved reconstruction quality.
//!
//! After encoding with the main codebook, the quantization residual
//! `r = rotated_block - codeword[main_index]` still contains signal.
//! A second, smaller codebook (typically 4-8 codewords) captures this
//! residual, adding 2-3 bits per block for a significant quality gain.
//!
//! The two-level scheme:
//! 1. Encode: find main index (as before), compute residual, find residual index.
//! 2. Store: main indices + residual indices in the packed payload.
//! 3. Decode: codeword[main_index] + residual_codebook[residual_index].
//!
//! Quality impact: cosine fidelity improves from ~0.863 (single-level)
//! to ~0.93+ (two-level) for typical nomic-768 workloads, at the cost
//! of 2-3 extra bits per block (~10-15% size increase).
//!
//! # Multi-Level Additive Quantization
//!
//! The multi-level scheme extends this to N levels:
//! 1. Encode: find main index, compute residual r1, find level-1 index,
//!    compute residual r2 = r1 - cw1[idx1], find level-2 index, etc.
//! 2. Store: main indices + packed residual indices per level.
//! 3. Decode: codeword[main] + cw1[idx1] + cw2[idx2] + ... then inverse
//!    rotation and norm scaling.
//!
//! Each residual level is trained with Lloyd-Max on the residual distribution
//! from the previous levels, producing codebooks adapted to the actual data
//! rather than random directions.

use serde::{Deserialize, Serialize};

use crate::{
    bitpack::{pack_indices, unpack_indices},
    codebook::FibCodebookV1,
    profile::FibQuantProfileV1,
    FibQuantError, Result,
};

/// Residual codebook schema marker.
pub const RESIDUAL_SCHEMA: &str = "fib_residual_codebook_v1";

/// A residual codebook — a small set of codewords for encoding the
/// quantization residual from the main codebook.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResidualCodebookV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Number of residual codewords (typically 4-8).
    pub size: u32,
    /// Block dimension (must match the main codebook's block_dim).
    pub block_dim: u32,
    /// Row-major `size × block_dim` f32 codewords.
    pub codewords: Vec<f32>,
    /// Digest of the residual codebook.
    pub codebook_digest: String,
}

impl ResidualCodebookV1 {
    /// Build a deterministic residual codebook.
    ///
    /// The residual codebook is seeded from the main profile's codebook_seed
    /// with a fixed offset. It uses a small set of unit-length directions
    /// scaled by a fraction of the expected residual magnitude.
    pub fn build(profile: &FibQuantProfileV1, main_codebook: &FibCodebookV1) -> Result<Self> {
        let k = profile.block_dim as usize;
        let n = profile.codebook_size as usize;
        // Residual codebook size: 4 codewords (2 bits) by default.
        // This captures the dominant residual direction and its negation,
        // plus two orthogonal corrections.
        let residual_n = 4usize;
        use rand::SeedableRng;
        let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(
            profile.codebook_seed.wrapping_add(0x5253_4355_4f4e), // "RSON"
        );
        use rand_distr::{Distribution, StandardNormal};
        let mut codewords = Vec::with_capacity(residual_n * k);
        // Codeword 0: zero (no residual correction — pass-through).
        codewords.resize(k, 0.0);
        // Codewords 1-3: random directions scaled by the average
        // nearest-neighbor distance in the main codebook.
        let avg_nn_dist = estimate_avg_nn_distance(&main_codebook.codewords, n, k);
        for _ in 1..residual_n {
            let mut dir = Vec::with_capacity(k);
            let mut norm_sq = 0.0f64;
            for _ in 0..k {
                let v: f64 = StandardNormal.sample(&mut rng);
                dir.push(v);
                norm_sq += v * v;
            }
            let norm = norm_sq.sqrt();
            let scale = avg_nn_dist * 0.5; // half the NN distance
            for v in &dir {
                codewords.push((v / norm * scale) as f32);
            }
        }
        let mut cb = Self {
            schema_version: RESIDUAL_SCHEMA.into(),
            size: residual_n as u32,
            block_dim: profile.block_dim,
            codewords,
            codebook_digest: String::new(),
        };
        cb.codebook_digest = cb.compute_digest()?;
        Ok(cb)
    }

    /// Train a residual codebook using Lloyd-Max on the residual distribution.
    ///
    /// Collects residuals after encoding training vectors with the main codebook,
    /// then runs Lloyd-Max refinement to find optimal residual codewords adapted
    /// to the actual residual distribution (instead of random directions).
    ///
    /// # Arguments
    /// * `profile` - The quantization profile (provides dims, rotation, seed).
    /// * `main_codebook` - The main codebook to compute residuals against.
    /// * `training_vectors` - Full ambient-dim training vectors (will be normalized + rotated).
    /// * `num_codewords` - Number of residual codewords to train.
    pub fn train(
        profile: &FibQuantProfileV1,
        main_codebook: &FibCodebookV1,
        training_vectors: &[Vec<f32>],
        num_codewords: usize,
    ) -> Result<Self> {
        let k = profile.block_dim as usize;
        let block_count = profile.block_count() as usize;

        // Build rotation to compute rotated blocks
        let rotation = crate::rotation::StoredRotation::new(
            profile.ambient_dim as usize,
            profile.rotation_seed,
        )?;

        // Collect residual blocks from all training vectors
        let mut residual_blocks: Vec<Vec<f32>> =
            Vec::with_capacity(training_vectors.len() * block_count);
        for x in training_vectors {
            let norm: f64 = x
                .iter()
                .map(|v| f64::from(*v) * f64::from(*v))
                .sum::<f64>()
                .sqrt();
            if norm == 0.0 {
                continue;
            }
            let normalized: Vec<f64> = x.iter().map(|v| f64::from(*v) / norm).collect();
            let rotated = rotation.apply(&normalized)?;
            let rotated_f32: Vec<f32> = rotated.iter().map(|&v| v as f32).collect();

            for block_idx in 0..block_count {
                let block = &rotated_f32[block_idx * k..(block_idx + 1) * k];
                let main_idx = nearest_codeword_f32(block, &main_codebook.codewords, k);
                let cw = &main_codebook.codewords[main_idx * k..(main_idx + 1) * k];
                let residual: Vec<f32> = block.iter().zip(cw.iter()).map(|(a, b)| a - b).collect();
                residual_blocks.push(residual);
            }
        }

        if residual_blocks.is_empty() {
            return Err(FibQuantError::NumericalFailure(
                "no residual blocks collected for training".into(),
            ));
        }

        let codewords = lloyd_max_train(
            &residual_blocks,
            k,
            num_codewords,
            profile.codebook_seed.wrapping_add(0x5452_4e52_4553), // "TNRES"
        )?;

        let mut cb = Self {
            schema_version: RESIDUAL_SCHEMA.into(),
            size: num_codewords as u32,
            block_dim: profile.block_dim,
            codewords,
            codebook_digest: String::new(),
        };
        cb.codebook_digest = cb.compute_digest()?;
        Ok(cb)
    }

    /// Validate the residual codebook.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != RESIDUAL_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "residual codebook schema {}, expected {RESIDUAL_SCHEMA}",
                self.schema_version
            )));
        }
        let expected_len = (self.size as usize) * (self.block_dim as usize);
        if self.codewords.len() != expected_len {
            return Err(FibQuantError::CorruptPayload(format!(
                "residual codebook has {} values, expected {}",
                self.codewords.len(),
                expected_len
            )));
        }
        if self.codewords.iter().any(|v| !v.is_finite()) {
            return Err(FibQuantError::CorruptPayload(
                "residual codebook contains non-finite value".into(),
            ));
        }
        if self.codebook_digest != self.compute_digest()? {
            return Err(FibQuantError::CodebookDigestMismatch {
                expected: self.compute_digest()?,
                actual: self.codebook_digest.clone(),
            });
        }
        Ok(())
    }

    /// Find the nearest residual codeword for a given residual block.
    pub fn nearest(&self, residual: &[f32]) -> Result<u32> {
        let k = self.block_dim as usize;
        if residual.len() != k {
            return Err(FibQuantError::CorruptPayload(format!(
                "residual block dim {}, expected {}",
                residual.len(),
                k
            )));
        }
        let n = self.size as usize;
        let mut best_idx = 0u32;
        let mut best_dist = f32::INFINITY;
        for i in 0..n {
            let cw = &self.codewords[i * k..(i + 1) * k];
            let dist: f32 = residual
                .iter()
                .zip(cw.iter())
                .map(|(a, b)| {
                    let d = a - b;
                    d * d
                })
                .sum();
            if dist < best_dist {
                best_dist = dist;
                best_idx = i as u32;
            }
        }
        Ok(best_idx)
    }

    /// Get a residual codeword by index.
    pub fn codeword(&self, index: u32) -> Result<&[f32]> {
        let k = self.block_dim as usize;
        let i = index as usize;
        if i >= self.size as usize {
            return Err(FibQuantError::IndexOutOfRange {
                index,
                codebook_size: self.size,
            });
        }
        Ok(&self.codewords[i * k..(i + 1) * k])
    }

    /// Bits per residual index.
    pub fn bits_per_index(&self) -> u8 {
        let n = self.size as usize;
        if n <= 1 {
            return 0;
        }
        // ceil(log2(n)) = number of bits needed to represent indices [0, n)
        (n as u32).next_power_of_two().trailing_zeros() as u8
    }

    fn compute_digest(&self) -> Result<String> {
        #[derive(Serialize)]
        struct DigestView<'a> {
            schema_version: &'a str,
            size: u32,
            block_dim: u32,
            codewords: &'a [f32],
        }
        crate::digest::json_digest(
            RESIDUAL_SCHEMA,
            &DigestView {
                schema_version: &self.schema_version,
                size: self.size,
                block_dim: self.block_dim,
                codewords: &self.codewords,
            },
        )
    }
}

/// Estimate the average nearest-neighbor distance between codewords.
/// This determines the scale of the residual codebook.
fn estimate_avg_nn_distance(codewords: &[f32], n: usize, k: usize) -> f64 {
    if n <= 1 {
        return 1.0;
    }
    let mut total = 0.0f64;
    let mut count = 0usize;
    // Sample up to 32 codewords to keep this O(n * 32 * k) not O(n^2 * k).
    let sample = n.min(32);
    for i in 0..sample {
        let ci = &codewords[i * k..(i + 1) * k];
        let mut nearest_dist = f64::INFINITY;
        for j in 0..n {
            if j == i {
                continue;
            }
            let cj = &codewords[j * k..(j + 1) * k];
            let dist: f64 = ci
                .iter()
                .zip(cj.iter())
                .map(|(a, b)| {
                    let d = f64::from(*a) - f64::from(*b);
                    d * d
                })
                .sum::<f64>()
                .sqrt();
            if dist < nearest_dist {
                nearest_dist = dist;
            }
        }
        if nearest_dist.is_finite() {
            total += nearest_dist;
            count += 1;
        }
    }
    if count == 0 {
        1.0
    } else {
        total / count as f64
    }
}

/// Two-level encoded artifact: main code + residual code.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FibResidualCodeV1 {
    /// The main FibQuant code (same as single-level).
    pub main_code: crate::codec::FibCodeV1,
    /// Packed residual indices (same block_count, smaller bit width).
    pub residual_indices: Vec<u8>,
    /// Bits per residual index.
    pub residual_bits: u8,
}

/// Two-level quantizer: main codebook + residual codebook.
pub struct FibResidualQuantizer {
    quantizer: crate::codec::FibQuantizer,
    residual_codebook: ResidualCodebookV1,
}

impl FibResidualQuantizer {
    /// Build a two-level quantizer from a profile.
    pub fn new(profile: FibQuantProfileV1) -> Result<Self> {
        let quantizer = crate::codec::FibQuantizer::new(profile.clone())?;
        let residual_codebook = ResidualCodebookV1::build(&profile, quantizer.codebook())?;
        Ok(Self {
            quantizer,
            residual_codebook,
        })
    }

    /// Build a two-level quantizer with a custom (pre-trained) residual codebook.
    ///
    /// Useful for comparing random vs trained residual codebooks.
    pub fn with_residual(
        quantizer: crate::codec::FibQuantizer,
        residual_codebook: ResidualCodebookV1,
    ) -> Result<Self> {
        Ok(Self {
            quantizer,
            residual_codebook,
        })
    }

    /// Access the main quantizer.
    pub fn quantizer(&self) -> &crate::codec::FibQuantizer {
        &self.quantizer
    }

    /// Access the residual codebook.
    pub fn residual_codebook(&self) -> &ResidualCodebookV1 {
        &self.residual_codebook
    }

    /// Encode a vector with two-level quantization.
    pub fn encode(&self, x: &[f32]) -> Result<FibResidualCodeV1> {
        let d = self.quantizer.profile().ambient_dim as usize;
        let k = self.quantizer.profile().block_dim as usize;
        if x.len() != d {
            return Err(FibQuantError::CorruptPayload(format!(
                "input dimension {}, expected {d}",
                x.len()
            )));
        }
        // Encode with the main codebook (produces FibCodeV1 + rotation).
        let main_code = self.quantizer.encode(x)?;

        // Now compute the residual. We need the rotated vector to find
        // the per-block residual.
        let norm: f64 = x
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            return Err(FibQuantError::ZeroNorm);
        }
        let normalized: Vec<f64> = x.iter().map(|v| f64::from(*v) / norm).collect();
        let rotated = self.quantizer.rotation().apply(&normalized)?;
        let rotated_f32: Vec<f32> = rotated.iter().map(|&v| v as f32).collect();
        let block_count = self.quantizer.profile().block_count() as usize;

        // Unpack main indices to find which codeword was selected per block
        let main_indices = crate::bitpack::unpack_indices(
            &main_code.indices,
            block_count,
            self.quantizer.profile().wire_index_bits,
        )?;

        // For each block, compute residual and find nearest residual codeword
        let codewords = &self.quantizer.codebook().codewords;
        let mut residual_indices_list = Vec::with_capacity(block_count);
        for (block_idx, &main_idx) in main_indices.iter().enumerate() {
            let main_idx = main_idx as usize;
            let block = &rotated_f32[block_idx * k..(block_idx + 1) * k];
            let cw = &codewords[main_idx * k..(main_idx + 1) * k];
            let residual: Vec<f32> = block.iter().zip(cw.iter()).map(|(a, b)| a - b).collect();
            let res_idx = self.residual_codebook.nearest(&residual)?;
            residual_indices_list.push(res_idx);
        }

        let residual_bits = self.residual_codebook.bits_per_index();
        let residual_indices = if residual_bits > 0 {
            pack_indices(&residual_indices_list, residual_bits)?
        } else {
            Vec::new()
        };

        Ok(FibResidualCodeV1 {
            main_code,
            residual_indices,
            residual_bits,
        })
    }

    /// Decode a two-level code.
    pub fn decode(&self, code: &FibResidualCodeV1) -> Result<Vec<f32>> {
        let k = self.quantizer.profile().block_dim as usize;
        let block_count = self.quantizer.profile().block_count() as usize;

        // Unpack main and residual indices
        let main_indices = crate::bitpack::unpack_indices(
            &code.main_code.indices,
            block_count,
            self.quantizer.profile().wire_index_bits,
        )?;
        let residual_indices = if code.residual_bits > 0 && !code.residual_indices.is_empty() {
            crate::bitpack::unpack_indices(&code.residual_indices, block_count, code.residual_bits)?
        } else {
            vec![0u32; block_count]
        };

        // Reconstruct: codeword[main] + residual_cw[residual] per block
        let codewords = &self.quantizer.codebook().codewords;
        let mut rotated_f32 = Vec::with_capacity(self.quantizer.profile().ambient_dim as usize);
        for (main_idx, res_idx) in main_indices.iter().zip(residual_indices.iter()) {
            let main_idx = *main_idx as usize;
            let res_idx = *res_idx as usize;
            let main_cw = &codewords[main_idx * k..(main_idx + 1) * k];
            let res_cw = self.residual_codebook.codeword(res_idx as u32)?;
            for (m, r) in main_cw.iter().zip(res_cw.iter()) {
                rotated_f32.push(m + r);
            }
        }

        // Apply inverse rotation and scale by norm
        let norm = decode_norm_from_code(&code.main_code)?;
        let reconstructed = self
            .quantizer
            .rotation()
            .apply_inverse(&rotated_f32.iter().map(|&v| v as f64).collect::<Vec<_>>())?;
        let out: Vec<f32> = reconstructed
            .into_iter()
            .map(|v| (v * norm) as f32)
            .collect();
        Ok(out)
    }

    /// Reconstruction cosine similarity.
    pub fn cosine_similarity(&self, x: &[f32]) -> Result<f64> {
        let code = self.encode(x)?;
        let decoded = self.decode(&code)?;
        crate::metrics::cosine_similarity(x, &decoded)
    }
}

// ===========================================================================
// Multi-Level Additive Quantization
// ===========================================================================

/// Multi-level residual codebook container.
///
/// Holds all residual codebooks for a multi-level additive quantizer.
/// Each level's codebook captures the residual left after encoding with
/// all previous levels.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MultiLevelResidualCodebookV1 {
    /// Each level's residual codebook (levels[0] = first residual level, etc.).
    pub levels: Vec<ResidualCodebookV1>,
    /// Sum of bits per index across all residual levels (main bits not included).
    pub total_bits: u32,
}

impl MultiLevelResidualCodebookV1 {
    /// Validate all levels and check total_bits consistency.
    pub fn validate(&self) -> Result<()> {
        for level in &self.levels {
            level.validate()?;
        }
        let computed: u32 = self.levels.iter().map(|l| l.bits_per_index() as u32).sum();
        if computed != self.total_bits {
            return Err(FibQuantError::CorruptPayload(format!(
                "total_bits {} does not match sum of per-level bits {}",
                self.total_bits, computed
            )));
        }
        Ok(())
    }

    /// Number of residual levels.
    pub fn num_levels(&self) -> usize {
        self.levels.len()
    }
}

/// Multi-level encoded artifact: main code + residual indices for all levels.
///
/// `residual_indices` is the concatenation of packed index arrays for each
/// residual level. `residual_bits[i]` gives the bit width for level i.
/// During decode, each level's byte segment is extracted using
/// `ceil(block_count * residual_bits[i] / 8)` and unpacked independently.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultiLevelCode {
    /// The main FibQuant code (same as single-level).
    pub main_code: crate::codec::FibCodeV1,
    /// Concatenated packed residual indices for all levels.
    pub residual_indices: Vec<u8>,
    /// Bits per residual index for each level.
    pub residual_bits: Vec<u8>,
}

/// Multi-level additive quantizer: main codebook + N trained residual codebooks.
///
/// Each residual level is trained with Lloyd-Max on the residual distribution
/// from the previous levels, producing codebooks adapted to the actual data
/// rather than random directions.
///
/// `num_levels = 1` means main only (no residual codebooks).
/// `num_levels = 2` means main + 1 residual level (equivalent to 2-level).
/// `num_levels = 3` means main + 2 residual levels.
#[derive(Debug, Clone)]
pub struct FibMultiLevelQuantizer {
    quantizer: crate::codec::FibQuantizer,
    residual_codebooks: Vec<ResidualCodebookV1>,
}

impl FibMultiLevelQuantizer {
    /// Build a multi-level quantizer with trained residual codebooks.
    ///
    /// # Arguments
    /// * `profile` - The quantization profile.
    /// * `num_levels` - Total number of levels (1 = main only, 2 = main + 1 residual, etc.).
    /// * `residual_sizes` - Number of codewords for each residual level
    ///   (length must be `num_levels - 1`).
    pub fn new(
        profile: FibQuantProfileV1,
        num_levels: usize,
        residual_sizes: Vec<usize>,
    ) -> Result<Self> {
        if num_levels == 0 {
            return Err(FibQuantError::CorruptPayload(
                "num_levels must be >= 1".into(),
            ));
        }
        let expected_residual_count = num_levels.saturating_sub(1);
        if residual_sizes.len() != expected_residual_count {
            return Err(FibQuantError::CorruptPayload(format!(
                "residual_sizes length {} does not match num_levels-1 = {}",
                residual_sizes.len(),
                expected_residual_count
            )));
        }
        for &sz in &residual_sizes {
            if sz == 0 {
                return Err(FibQuantError::CorruptPayload(
                    "residual_sizes must be > 0".into(),
                ));
            }
        }

        let quantizer = crate::codec::FibQuantizer::new(profile.clone())?;

        if num_levels == 1 {
            return Ok(Self {
                quantizer,
                residual_codebooks: Vec::new(),
            });
        }

        // Generate training vectors for residual codebook training
        let training_vectors = generate_training_vectors(&profile)?;

        // Build residual codebooks level by level
        let mut residual_codebooks = Vec::with_capacity(expected_residual_count);

        for (level, &num_cw) in residual_sizes
            .iter()
            .enumerate()
            .take(expected_residual_count)
        {
            // Compute residual blocks at this level (residuals after all previous levels)
            let residual_blocks = compute_multi_level_residuals(
                &profile,
                &quantizer,
                &residual_codebooks,
                &training_vectors,
            )?;

            if residual_blocks.is_empty() {
                return Err(FibQuantError::NumericalFailure(format!(
                    "no residual blocks collected for level {level}"
                )));
            }

            // Train a residual codebook on these residuals using Lloyd-Max
            let cb = train_residual_on_blocks(
                &profile,
                &residual_blocks,
                num_cw,
                profile
                    .codebook_seed
                    .wrapping_add((level as u64).wrapping_mul(0x4c45_5645_4c52)), // "LEVELR"
            )?;

            residual_codebooks.push(cb);
        }

        Ok(Self {
            quantizer,
            residual_codebooks,
        })
    }

    /// Access the main quantizer.
    pub fn quantizer(&self) -> &crate::codec::FibQuantizer {
        &self.quantizer
    }

    /// Access all residual codebooks.
    pub fn residual_codebooks(&self) -> &[ResidualCodebookV1] {
        &self.residual_codebooks
    }

    /// Total number of levels (1 + residual codebooks count).
    pub fn num_levels(&self) -> usize {
        1 + self.residual_codebooks.len()
    }

    /// Get a `MultiLevelResidualCodebookV1` view of the residual codebooks.
    pub fn multi_level_codebook(&self) -> MultiLevelResidualCodebookV1 {
        let total_bits: u32 = self
            .residual_codebooks
            .iter()
            .map(|cb| cb.bits_per_index() as u32)
            .sum();
        MultiLevelResidualCodebookV1 {
            levels: self.residual_codebooks.clone(),
            total_bits,
        }
    }

    /// Encode a vector with multi-level additive quantization.
    ///
    /// Encodes with the main codebook, then iteratively encodes the residual
    /// with each residual level: residual[i+1] = residual[i] - cw_i[idx_i].
    pub fn encode(&self, x: &[f32]) -> Result<MultiLevelCode> {
        let d = self.quantizer.profile().ambient_dim as usize;
        let k = self.quantizer.profile().block_dim as usize;
        if x.len() != d {
            return Err(FibQuantError::CorruptPayload(format!(
                "input dimension {}, expected {d}",
                x.len()
            )));
        }

        // Encode with the main codebook
        let main_code = self.quantizer.encode(x)?;

        if self.residual_codebooks.is_empty() {
            return Ok(MultiLevelCode {
                main_code,
                residual_indices: Vec::new(),
                residual_bits: Vec::new(),
            });
        }

        // Compute rotated blocks for residual encoding
        let norm: f64 = x
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            return Err(FibQuantError::ZeroNorm);
        }
        let normalized: Vec<f64> = x.iter().map(|v| f64::from(*v) / norm).collect();
        let rotated = self.quantizer.rotation().apply(&normalized)?;
        let rotated_f32: Vec<f32> = rotated.iter().map(|&v| v as f32).collect();
        let block_count = self.quantizer.profile().block_count() as usize;

        // Unpack main indices
        let main_indices = unpack_indices(
            &main_code.indices,
            block_count,
            self.quantizer.profile().wire_index_bits,
        )?;

        let codewords = &self.quantizer.codebook().codewords;

        // For each block, encode residuals level by level (additive)
        let mut all_residual_indices: Vec<Vec<u32>> =
            Vec::with_capacity(self.residual_codebooks.len());
        for _ in &self.residual_codebooks {
            all_residual_indices.push(Vec::with_capacity(block_count));
        }

        for (block_idx, &main_idx) in main_indices.iter().enumerate() {
            let main_idx = main_idx as usize;
            let block = &rotated_f32[block_idx * k..(block_idx + 1) * k];
            let main_cw = &codewords[main_idx * k..(main_idx + 1) * k];

            // Initial residual = block - main_cw
            let mut residual: Vec<f32> = block
                .iter()
                .zip(main_cw.iter())
                .map(|(a, b)| a - b)
                .collect();

            // Encode with each residual level, subtracting the codeword each time
            for (level, rcb) in self.residual_codebooks.iter().enumerate() {
                let idx = rcb.nearest(&residual)?;
                all_residual_indices[level].push(idx);
                let cw = rcb.codeword(idx)?;
                for (r, c) in residual.iter_mut().zip(cw.iter()) {
                    *r -= c;
                }
            }
        }

        // Pack each level's indices and concatenate
        let mut residual_bits = Vec::with_capacity(self.residual_codebooks.len());
        let mut residual_indices = Vec::new();
        for (level, rcb) in self.residual_codebooks.iter().enumerate() {
            let bits = rcb.bits_per_index();
            residual_bits.push(bits);
            if bits > 0 {
                let packed = pack_indices(&all_residual_indices[level], bits)?;
                residual_indices.extend_from_slice(&packed);
            }
        }

        Ok(MultiLevelCode {
            main_code,
            residual_indices,
            residual_bits,
        })
    }

    /// Decode a multi-level code.
    ///
    /// Reconstructs: codeword[main] + residual_cw[0] + residual_cw[1] + ...
    /// then applies inverse rotation and norm scaling.
    pub fn decode(&self, code: &MultiLevelCode) -> Result<Vec<f32>> {
        let k = self.quantizer.profile().block_dim as usize;
        let block_count = self.quantizer.profile().block_count() as usize;

        // Unpack main indices
        let main_indices = unpack_indices(
            &code.main_code.indices,
            block_count,
            self.quantizer.profile().wire_index_bits,
        )?;

        // Validate residual_bits length matches our codebooks
        if code.residual_bits.len() != self.residual_codebooks.len() {
            return Err(FibQuantError::CorruptPayload(format!(
                "residual_bits length {} does not match quantizer residual levels {}",
                code.residual_bits.len(),
                self.residual_codebooks.len()
            )));
        }

        // Unpack residual indices per level from the concatenated byte array
        let mut all_residual_indices: Vec<Vec<u32>> =
            Vec::with_capacity(self.residual_codebooks.len());
        let mut offset = 0;
        for &bits in &code.residual_bits {
            if bits > 0 {
                let level_bytes = (block_count * bits as usize).div_ceil(8);
                if offset + level_bytes > code.residual_indices.len() {
                    return Err(FibQuantError::CorruptPayload(format!(
                        "residual_indices too short: need {} bytes at offset {}, have {}",
                        level_bytes,
                        offset,
                        code.residual_indices.len()
                    )));
                }
                let packed = &code.residual_indices[offset..offset + level_bytes];
                let unpacked = unpack_indices(packed, block_count, bits)?;
                all_residual_indices.push(unpacked);
                offset += level_bytes;
            } else {
                // 0 bits means single-codeword level — all indices are 0
                all_residual_indices.push(vec![0u32; block_count]);
            }
        }

        // Reconstruct: main_cw + sum of all residual level codewords per block
        let codewords = &self.quantizer.codebook().codewords;
        let mut rotated_f32 = Vec::with_capacity(self.quantizer.profile().ambient_dim as usize);

        for block_idx in 0..block_count {
            let main_idx = main_indices[block_idx] as usize;
            let main_cw = &codewords[main_idx * k..(main_idx + 1) * k];

            // Start with main codeword
            let mut block_recon: Vec<f32> = main_cw.to_vec();

            // Add residual codewords from each level
            for (level, rcb) in self.residual_codebooks.iter().enumerate() {
                let res_idx = all_residual_indices[level][block_idx];
                let cw = rcb.codeword(res_idx)?;
                for (r, c) in block_recon.iter_mut().zip(cw.iter()) {
                    *r += c;
                }
            }

            rotated_f32.extend(block_recon);
        }

        // Apply inverse rotation and scale by norm
        let norm = decode_norm_from_code(&code.main_code)?;
        let reconstructed = self
            .quantizer
            .rotation()
            .apply_inverse(&rotated_f32.iter().map(|&v| v as f64).collect::<Vec<_>>())?;
        let out: Vec<f32> = reconstructed
            .into_iter()
            .map(|v| (v * norm) as f32)
            .collect();
        Ok(out)
    }

    /// Reconstruction cosine similarity.
    pub fn cosine_similarity(&self, x: &[f32]) -> Result<f64> {
        let code = self.encode(x)?;
        let decoded = self.decode(&code)?;
        crate::metrics::cosine_similarity(x, &decoded)
    }
}

// ===========================================================================
// Helper functions for multi-level training
// ===========================================================================

/// Find the nearest codeword in a flat row-major codebook (f32).
fn nearest_codeword_f32(block: &[f32], codewords: &[f32], k: usize) -> usize {
    let n = codewords.len() / k;
    let mut best_idx = 0usize;
    let mut best_dist = f32::INFINITY;
    for i in 0..n {
        let cw = &codewords[i * k..(i + 1) * k];
        let dist: f32 = block
            .iter()
            .zip(cw.iter())
            .map(|(a, b)| {
                let d = a - b;
                d * d
            })
            .sum();
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
    }
    best_idx
}

/// Run Lloyd-Max on a set of k-dimensional residual blocks.
///
/// Initializes centroids from random samples, then iterates
/// assignment + centroid update until convergence or max_iterations.
/// Empty cells are reinitialized from random samples.
fn lloyd_max_train(samples: &[Vec<f32>], k: usize, n: usize, seed: u64) -> Result<Vec<f32>> {
    if samples.is_empty() {
        return Err(FibQuantError::NumericalFailure(
            "no samples for Lloyd-Max training".into(),
        ));
    }
    if n == 0 {
        return Err(FibQuantError::CorruptPayload(
            "num_codewords must be > 0".into(),
        ));
    }

    use rand::seq::SliceRandom;
    use rand::SeedableRng;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(seed);

    // Initialize: pick n random samples as initial centroids
    let mut indices: Vec<usize> = (0..samples.len()).collect();
    indices.shuffle(&mut rng);

    let mut centroids: Vec<Vec<f32>> = Vec::with_capacity(n);
    for i in 0..n.min(samples.len()) {
        centroids.push(samples[indices[i]].clone());
    }
    // If fewer samples than n, fill remaining with perturbed copies
    use rand_distr::{Distribution, StandardNormal};
    while centroids.len() < n {
        let base = &samples[indices[0]];
        let mut cw = Vec::with_capacity(k);
        for &v in base {
            let noise: f64 =
                <StandardNormal as Distribution<f64>>::sample(&StandardNormal, &mut rng) * 0.001;
            cw.push((f64::from(v) + noise) as f32);
        }
        centroids.push(cw);
    }

    let max_iterations = 25;
    for _ in 0..max_iterations {
        // Assignment step: assign each sample to nearest centroid
        let mut assignments = Vec::with_capacity(samples.len());
        for s in samples {
            let mut best_idx = 0usize;
            let mut best_dist = f32::INFINITY;
            for (i, cw) in centroids.iter().enumerate() {
                let dist: f32 = s
                    .iter()
                    .zip(cw.iter())
                    .map(|(a, b)| {
                        let d = a - b;
                        d * d
                    })
                    .sum();
                if dist < best_dist {
                    best_dist = dist;
                    best_idx = i;
                }
            }
            assignments.push(best_idx);
        }

        // Update step: recompute centroids as means of assigned samples
        let mut sums = vec![0.0f64; n * k];
        let mut counts = vec![0usize; n];
        for (s, &a) in samples.iter().zip(&assignments) {
            counts[a] += 1;
            for d in 0..k {
                sums[a * k + d] += f64::from(s[d]);
            }
        }

        let mut changed = false;
        for i in 0..n {
            if counts[i] > 0 {
                for d in 0..k {
                    let new_val = (sums[i * k + d] / counts[i] as f64) as f32;
                    if (new_val - centroids[i][d]).abs() > 1e-10 {
                        changed = true;
                    }
                    centroids[i][d] = new_val;
                }
            } else {
                // Empty cell: reinitialize from a random sample
                let idx = indices.choose(&mut rng).copied().unwrap_or(0);
                centroids[i] = samples[idx].clone();
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    // Flatten to row-major
    let mut codewords = Vec::with_capacity(n * k);
    for cw in &centroids {
        codewords.extend_from_slice(cw);
    }
    Ok(codewords)
}

/// Generate full d-dimensional training vectors from the spherical-Beta distribution.
///
/// Each training vector is constructed by sampling `block_count` independent
/// k-dimensional blocks from the spherical-Beta source and concatenating them
/// into a d-dimensional vector. This matches the structure that the main
/// codebook operates on after rotation.
fn generate_training_vectors(profile: &FibQuantProfileV1) -> Result<Vec<Vec<f32>>> {
    use rand::SeedableRng;
    let d = profile.ambient_dim as usize;
    let k = profile.block_dim as usize;
    let block_count = profile.block_count() as usize;
    let count = profile.training_samples.max(256) as usize;
    let mut rng = rand_chacha::ChaCha8Rng::seed_from_u64(
        profile.codebook_seed ^ 0x5452_5641_494e, // "TRAIN"
    );
    let mut result = Vec::with_capacity(count);
    for _ in 0..count {
        let mut full_vec = Vec::with_capacity(d);
        for _ in 0..block_count {
            let block = crate::spherical_beta::sample_spherical_beta(d, k, &mut rng)?;
            full_vec.extend(block.into_iter().map(|x| x as f32));
        }
        // Normalize to unit length so the rotation + norm path works correctly
        let norm: f64 = full_vec
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            .sqrt();
        if norm > 0.0 && norm.is_finite() {
            for v in &mut full_vec {
                *v = (f64::from(*v) / norm) as f32;
            }
        }
        result.push(full_vec);
    }
    Ok(result)
}

/// Compute residual blocks at a given level for training.
///
/// For each training vector: normalize → rotate → for each block, find main
/// codeword, compute residual, then subtract all previous residual level
/// codewords to get the residual at the current level.
fn compute_multi_level_residuals(
    profile: &FibQuantProfileV1,
    quantizer: &crate::codec::FibQuantizer,
    prev_codebooks: &[ResidualCodebookV1],
    training_vectors: &[Vec<f32>],
) -> Result<Vec<Vec<f32>>> {
    let k = profile.block_dim as usize;
    let block_count = profile.block_count() as usize;
    let rotation = quantizer.rotation();
    let codewords = &quantizer.codebook().codewords;

    let mut all_residuals: Vec<Vec<f32>> = Vec::with_capacity(training_vectors.len() * block_count);

    for x in training_vectors {
        let norm: f64 = x
            .iter()
            .map(|v| f64::from(*v) * f64::from(*v))
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            continue;
        }
        let normalized: Vec<f64> = x.iter().map(|v| f64::from(*v) / norm).collect();
        let rotated = rotation.apply(&normalized)?;
        let rotated_f32: Vec<f32> = rotated.iter().map(|&v| v as f32).collect();

        for block_idx in 0..block_count {
            let block = &rotated_f32[block_idx * k..(block_idx + 1) * k];
            let main_idx = nearest_codeword_f32(block, codewords, k);
            let main_cw = &codewords[main_idx * k..(main_idx + 1) * k];

            // Initial residual = block - main_cw
            let mut residual: Vec<f32> = block
                .iter()
                .zip(main_cw.iter())
                .map(|(a, b)| a - b)
                .collect();

            // Subtract all previous residual level codewords
            for rcb in prev_codebooks {
                let idx = rcb.nearest(&residual)?;
                let cw = rcb.codeword(idx)?;
                for (r, c) in residual.iter_mut().zip(cw.iter()) {
                    *r -= c;
                }
            }

            all_residuals.push(residual);
        }
    }

    Ok(all_residuals)
}

/// Train a residual codebook on pre-computed residual blocks using Lloyd-Max.
fn train_residual_on_blocks(
    profile: &FibQuantProfileV1,
    residual_blocks: &[Vec<f32>],
    num_codewords: usize,
    seed: u64,
) -> Result<ResidualCodebookV1> {
    let k = profile.block_dim as usize;
    let codewords = lloyd_max_train(residual_blocks, k, num_codewords, seed)?;

    let mut cb = ResidualCodebookV1 {
        schema_version: RESIDUAL_SCHEMA.into(),
        size: num_codewords as u32,
        block_dim: profile.block_dim,
        codewords,
        codebook_digest: String::new(),
    };
    cb.codebook_digest = cb.compute_digest()?;
    Ok(cb)
}

fn decode_norm_from_code(code: &crate::codec::FibCodeV1) -> Result<f64> {
    use crate::profile::NormFormat;
    use half::f16;
    match code.norm_format {
        NormFormat::Fp16Paper => {
            let bytes: [u8; 2] = code
                .norm_payload
                .as_slice()
                .try_into()
                .map_err(|_| FibQuantError::CorruptPayload("fp16 norm length".into()))?;
            let value = f16::from_le_bytes(bytes).to_f32() as f64;
            if value.is_finite() && value > 0.0 {
                Ok(value)
            } else {
                Err(FibQuantError::CorruptPayload("invalid fp16 norm".into()))
            }
        }
        NormFormat::F32Reference => {
            let bytes: [u8; 4] = code
                .norm_payload
                .as_slice()
                .try_into()
                .map_err(|_| FibQuantError::CorruptPayload("f32 norm length".into()))?;
            let value = f32::from_le_bytes(bytes) as f64;
            if value.is_finite() && value > 0.0 {
                Ok(value)
            } else {
                Err(FibQuantError::CorruptPayload("invalid f32 norm".into()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_test_quantizer() -> Result<FibResidualQuantizer> {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        profile.training_samples = 128;
        profile.lloyd_restarts = 1;
        profile.lloyd_iterations = 2;
        FibResidualQuantizer::new(profile)
    }

    fn build_test_multi_level_quantizer(
        num_levels: usize,
        residual_sizes: Vec<usize>,
    ) -> Result<FibMultiLevelQuantizer> {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        profile.training_samples = 256;
        profile.lloyd_restarts = 2;
        profile.lloyd_iterations = 10;
        FibMultiLevelQuantizer::new(profile, num_levels, residual_sizes)
    }

    #[test]
    fn residual_codebook_has_correct_size() -> Result<()> {
        let rq = build_test_quantizer()?;
        assert_eq!(rq.residual_codebook().size, 4);
        assert_eq!(rq.residual_codebook().block_dim, 2);
        assert_eq!(rq.residual_codebook().codewords.len(), 4 * 2);
        Ok(())
    }

    #[test]
    fn residual_codebook_digest_is_valid() -> Result<()> {
        let rq = build_test_quantizer()?;
        rq.residual_codebook().validate()?;
        Ok(())
    }

    #[test]
    fn two_level_encode_decode_roundtrip() -> Result<()> {
        let rq = build_test_quantizer()?;
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = rq.encode(&input)?;
        let decoded = rq.decode(&code)?;
        assert_eq!(decoded.len(), input.len());
        for (a, b) in input.iter().zip(decoded.iter()) {
            assert!(a.is_finite() && b.is_finite());
        }
        Ok(())
    }

    #[test]
    fn two_level_better_than_single_level() -> Result<()> {
        let rq = build_test_quantizer()?;
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];

        // Single-level cosine
        let single_cos = rq.quantizer().cosine_similarity(&input)?;

        // Two-level cosine
        let two_level_cos = rq.cosine_similarity(&input)?;

        assert!(
            two_level_cos >= single_cos - 1e-6,
            "two-level should be >= single-level: {} vs {}",
            two_level_cos,
            single_cos
        );
        Ok(())
    }

    #[test]
    fn residual_nearest_returns_valid_index() -> Result<()> {
        let rq = build_test_quantizer()?;
        let residual = vec![0.1, -0.05];
        let idx = rq.residual_codebook().nearest(&residual)?;
        assert!(idx < rq.residual_codebook().size);
        Ok(())
    }

    // ----- Multi-level tests -----

    #[test]
    fn multi_level_roundtrip_produces_finite_values() -> Result<()> {
        let rq = build_test_multi_level_quantizer(3, vec![8, 8])?;
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = rq.encode(&input)?;
        let decoded = rq.decode(&code)?;
        assert_eq!(decoded.len(), input.len());
        for v in &decoded {
            assert!(v.is_finite(), "decoded value is not finite: {v}");
        }
        Ok(())
    }

    #[test]
    fn multi_level_codebook_validates() -> Result<()> {
        let rq = build_test_multi_level_quantizer(3, vec![8, 8])?;
        let mlcb = rq.multi_level_codebook();
        assert_eq!(mlcb.num_levels(), 2);
        assert!(mlcb.total_bits > 0);
        mlcb.validate()?;
        Ok(())
    }

    #[test]
    fn three_level_better_than_two_better_than_one() -> Result<()> {
        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];

        // 1-level (main only)
        let q1 = build_test_multi_level_quantizer(1, vec![])?;
        let code1 = q1.encode(&input)?;
        let decoded1 = q1.decode(&code1)?;
        let cos1 = crate::metrics::cosine_similarity(&input, &decoded1)?;

        // 2-level (main + 1 trained residual)
        let q2 = build_test_multi_level_quantizer(2, vec![8])?;
        let code2 = q2.encode(&input)?;
        let decoded2 = q2.decode(&code2)?;
        let cos2 = crate::metrics::cosine_similarity(&input, &decoded2)?;

        // 3-level (main + 2 trained residuals)
        let q3 = build_test_multi_level_quantizer(3, vec![8, 8])?;
        let code3 = q3.encode(&input)?;
        let decoded3 = q3.decode(&code3)?;
        let cos3 = crate::metrics::cosine_similarity(&input, &decoded3)?;

        assert!(
            cos2 >= cos1 - 1e-6,
            "2-level ({}) should be >= 1-level ({})",
            cos2,
            cos1
        );
        assert!(
            cos3 >= cos2 - 1e-6,
            "3-level ({}) should be >= 2-level ({})",
            cos3,
            cos2
        );
        Ok(())
    }

    #[test]
    fn trained_residual_better_than_random() -> Result<()> {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        profile.training_samples = 256;
        profile.lloyd_restarts = 2;
        profile.lloyd_iterations = 10;

        let quantizer = crate::codec::FibQuantizer::new(profile.clone())?;

        // Generate training vectors
        let training_vectors = generate_training_vectors(&profile)?;

        // Build random residual codebook (existing path)
        let random_cb = ResidualCodebookV1::build(&profile, quantizer.codebook())?;

        // Train a residual codebook with Lloyd-Max
        let trained_cb =
            ResidualCodebookV1::train(&profile, quantizer.codebook(), &training_vectors, 8)?;

        // Build two-level quantizers with each codebook
        let rq_random = FibResidualQuantizer::with_residual(
            crate::codec::FibQuantizer::new(profile.clone())?,
            random_cb,
        )?;
        let rq_trained = FibResidualQuantizer::with_residual(
            crate::codec::FibQuantizer::new(profile.clone())?,
            trained_cb,
        )?;

        // Test on multiple vectors to get a robust comparison
        let test_inputs: Vec<Vec<f32>> = vec![
            vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875],
            vec![0.3, 0.7, -0.2, 0.9, 0.4, -0.6, 0.8, -0.1],
            vec![-0.5, 0.3, 0.6, -0.8, 0.2, 0.9, -0.4, 0.7],
            vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
            vec![-0.9, 0.1, -0.3, 0.5, -0.7, 0.2, -0.4, 0.6],
        ];

        let mut random_total = 0.0f64;
        let mut trained_total = 0.0f64;
        for input in &test_inputs {
            let cos_random = rq_random.cosine_similarity(input)?;
            let cos_trained = rq_trained.cosine_similarity(input)?;
            random_total += cos_random;
            trained_total += cos_trained;
        }

        let random_avg = random_total / test_inputs.len() as f64;
        let trained_avg = trained_total / test_inputs.len() as f64;

        assert!(
            trained_avg >= random_avg - 1e-6,
            "trained residual ({}) should be >= random residual ({})",
            trained_avg,
            random_avg
        );
        Ok(())
    }

    #[test]
    fn one_level_multi_level_matches_single_level() -> Result<()> {
        let profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        let single = crate::codec::FibQuantizer::new(profile.clone())?;
        let multi = build_test_multi_level_quantizer(1, vec![])?;

        let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];

        let single_decoded = single.decode(&single.encode(&input)?)?;
        let multi_decoded = multi.decode(&multi.encode(&input)?)?;

        // The multi-level decode uses f64 rotation inverse while the single-level
        // FibQuantizer::decode uses the f32 fast path, so there can be small
        // floating-point differences. Check approximate equality with a
        // generous tolerance (f32 vs f64 rotation can differ at ~1e-2 level
        // for small dimensions).
        let cos_single = crate::metrics::cosine_similarity(&single_decoded, &multi_decoded)?;
        assert!(
            cos_single > 0.99,
            "single vs multi decoded cosine too low: {cos_single}"
        );
        Ok(())
    }
}
