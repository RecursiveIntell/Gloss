//! Per-dimension uniform quantizer scorer.
//!
//! Each dimension uses a shared (per-codec) min/max learned from a
//! calibration batch, and each compressed vector stores only the code.
//! The score is the inner product of the per-dim linear reconstructions.
//!
//! This matches the Python `per-dim` scorer in
//! `poly-kv/scripts/compressed_attention_forward_ppl.py`, which computes
//! per-dimension min/max over the full layer of keys.

#[cfg(feature = "no_std")]
extern crate alloc;
#[cfg(feature = "no_std")]
use alloc::vec::Vec;
#[cfg(not(feature = "no_std"))]
use std::vec::Vec;

use crate::error::{ScorerError, ScorerResult};
use crate::trait_def::{CompressedScorer, PreparedQuery};

#[cfg(feature = "c-kernels")]
extern "C" {
    fn cs_per_dim_score(codes: *const u8, dim: usize, lut: *const f32, levels: usize) -> f32;
}

/// Compressed vector using per-dimension uniform quantization.
#[derive(Clone, Debug)]
pub struct PerDimCompressed {
    /// Per-dimension codes, length == dim.
    pub codes: Vec<u8>,
    /// Original L2 norm, used to reconstruct un-normalized inner product.
    pub norm: f32,
}

/// Prepared query: dimension-wise quantized query.
#[derive(Clone, Debug)]
pub struct PerDimPrepared {
    dim: usize,
    query_codes: Vec<u8>,
    levels: usize,
    contribution_lut: Vec<f32>,
}

impl PerDimPrepared {
    /// Number of quantization levels represented in the contribution table.
    pub fn levels(&self) -> usize {
        self.levels
    }

    /// Number of scalar entries in the query-prepared contribution table.
    pub fn lookup_table_len(&self) -> usize {
        self.contribution_lut.len()
    }

    /// Per-dimension query codes used to build the contribution table.
    pub fn query_codes(&self) -> &[u8] {
        &self.query_codes
    }
}

impl PreparedQuery for PerDimPrepared {
    fn dim(&self) -> usize {
        self.dim
    }
}

/// Per-dimension quantized scorer.
#[derive(Clone, Debug)]
pub struct PerDimScorer {
    dim: usize,
    #[allow(dead_code)]
    bits: u32,
    levels: u32,
    /// Per-dimension minimum (shared across all vectors).
    min: Vec<f32>,
    /// Per-dimension step size (shared across all vectors).
    step: Vec<f32>,
}

impl PerDimScorer {
    pub fn new(dim: usize, bits: u32) -> ScorerResult<Self> {
        if dim == 0 {
            return Err(ScorerError::DimensionMismatch {
                expected: 1,
                got: 0,
            });
        }
        if bits == 0 || bits > 8 {
            return Err(ScorerError::ScoringFailed(
                "PerDimScorer bits must be in 1..=8".into(),
            ));
        }
        let levels = 1u32 << bits;
        Ok(Self {
            dim,
            bits,
            levels,
            min: (0..dim).map(|_| 0.0f32).collect(),
            step: (0..dim).map(|_| 1.0f32).collect(),
        })
    }

    /// Fit shared per-dimension min/max from a calibration batch.
    /// Vectors are normalized by their L2 norm before computing stats.
    pub fn fit(&mut self, vectors: &[&[f32]]) -> ScorerResult<()> {
        if vectors.is_empty() {
            return Err(ScorerError::ScoringFailed("empty fit batch".into()));
        }
        let mut min = (0..self.dim).map(|_| f32::INFINITY).collect::<Vec<_>>();
        let mut max = (0..self.dim).map(|_| f32::NEG_INFINITY).collect::<Vec<_>>();
        for v in vectors {
            if v.len() != self.dim {
                return Err(ScorerError::DimensionMismatch {
                    expected: self.dim,
                    got: v.len(),
                });
            }
            let norm = libm::sqrtf(v.iter().map(|&x| x * x).sum::<f32>());
            if norm == 0.0 {
                continue;
            }
            for (i, &x) in v.iter().enumerate() {
                let xi = x / norm;
                min[i] = min[i].min(xi);
                max[i] = max[i].max(xi);
            }
        }
        for i in 0..self.dim {
            let range = (max[i] - min[i]).max(1e-12);
            self.min[i] = min[i];
            self.step[i] = range / ((self.levels - 1) as f32);
        }
        Ok(())
    }

    fn encode(&self, vec: &[f32]) -> ScorerResult<Vec<u8>> {
        if vec.len() != self.dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: vec.len(),
            });
        }
        let mut codes = (0..self.dim).map(|_| 0u8).collect::<Vec<_>>();
        for i in 0..self.dim {
            let vi = vec[i] as f64;
            let min = self.min[i] as f64;
            let step = self.step[i] as f64;
            let code_f = libm::round(((vi - min) / step).clamp(0.0, (self.levels - 1) as f64));
            codes[i] = code_f as u8;
        }
        Ok(codes)
    }

    pub fn compress(&self, vector: &[f32]) -> ScorerResult<PerDimCompressed> {
        if vector.len() != self.dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        let norm = libm::sqrtf(vector.iter().map(|&x| x * x).sum::<f32>());
        if norm == 0.0 {
            return Err(ScorerError::ScoringFailed("zero-norm vector".into()));
        }
        let normalized: Vec<f32> = vector.iter().map(|&x| x / norm).collect();
        let codes = self.encode(&normalized)?;
        Ok(PerDimCompressed { codes, norm })
    }
}

impl CompressedScorer for PerDimScorer {
    type Compressed = PerDimCompressed;
    type Prepared = PerDimPrepared;

    fn prepare_query(&self, query: &[f32]) -> ScorerResult<Self::Prepared> {
        if query.len() != self.dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: query.len(),
            });
        }
        let norm = libm::sqrtf(query.iter().map(|&x| x * x).sum::<f32>());
        if norm == 0.0 {
            return Err(ScorerError::ScoringFailed("zero-norm query".into()));
        }
        let normalized: Vec<f32> = query.iter().map(|&x| x / norm).collect();
        let codes = self.encode(&normalized)?;
        let levels = self.levels as usize;
        let mut contribution_lut = Vec::with_capacity(self.dim * levels);
        for (i, query_code) in codes.iter().enumerate().take(self.dim) {
            let qry_val = self.min[i] + self.step[i] * f32::from(*query_code);
            for code in 0..levels {
                let key_val = self.min[i] + self.step[i] * code as f32;
                contribution_lut.push(qry_val * key_val);
            }
        }
        Ok(PerDimPrepared {
            dim: self.dim,
            query_codes: codes,
            levels,
            contribution_lut,
        })
    }

    fn score_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        if compressed.codes.len() != self.dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: compressed.codes.len(),
            });
        }
        if prepared.dim != self.dim || prepared.levels != self.levels as usize {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: prepared.dim,
            });
        }
        let expected_lut_len = self.dim * prepared.levels;
        if prepared.contribution_lut.len() != expected_lut_len {
            return Err(ScorerError::CorruptPayload(
                "per-dim prepared query lookup table has invalid length".into(),
            ));
        }
        #[cfg(feature = "c-kernels")]
        let score = unsafe {
            cs_per_dim_score(
                compressed.codes.as_ptr(),
                self.dim,
                prepared.contribution_lut.as_ptr(),
                prepared.levels,
            )
        };
        #[cfg(not(feature = "c-kernels"))]
        let score = compressed
            .codes
            .iter()
            .enumerate()
            .map(|(i, &code)| {
                f64::from(prepared.contribution_lut[i * prepared.levels + usize::from(code)])
            })
            .sum::<f64>() as f32;
        Ok(score)
    }

    fn decode(&self, compressed: &Self::Compressed) -> ScorerResult<Vec<f32>> {
        let mut out = Vec::with_capacity(self.dim);
        for i in 0..self.dim {
            let val = self.min[i] + self.step[i] * compressed.codes[i] as f32;
            out.push(val * compressed.norm);
        }
        Ok(out)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn codec_name(&self) -> &'static str {
        "per_dim_quant"
    }

    fn internal_bytes(&self) -> usize {
        self.min.len() * 4 + self.step.len() * 4
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "no_std")]
    use alloc::vec;

    fn sine_vec(dim: usize) -> Vec<f32> {
        (0..dim).map(|i| libm::sinf(i as f32)).collect()
    }

    #[test]
    fn per_dim_lookup_table_is_prepared_once_per_query() {
        let dim = 8;
        let mut scorer = PerDimScorer::new(dim, 4).unwrap();
        let v = sine_vec(dim);
        let v2: Vec<f32> = (0..dim).map(|i| libm::cosf(i as f32)).collect();
        scorer.fit(&[&v, &v2]).unwrap();

        let prepared = scorer.prepare_query(&v).unwrap();

        assert_eq!(prepared.levels(), 16);
        assert_eq!(prepared.lookup_table_len(), dim * 16);
    }

    #[test]
    fn per_dim_lookup_score_matches_direct_reconstruction_formula() {
        let dim = 8;
        let mut scorer = PerDimScorer::new(dim, 4).unwrap();
        let v = sine_vec(dim);
        let v2: Vec<f32> = (0..dim).map(|i| libm::cosf(i as f32)).collect();
        scorer.fit(&[&v, &v2]).unwrap();
        let compressed = scorer.compress(&v2).unwrap();
        let prepared = scorer.prepare_query(&v).unwrap();

        let lookup_score = scorer.score_prepared(&prepared, &compressed).unwrap();
        let mut direct_score = 0.0f64;
        for i in 0..dim {
            let key_val = scorer.min[i] + scorer.step[i] * compressed.codes[i] as f32;
            let query_val = scorer.min[i] + scorer.step[i] * prepared.query_codes()[i] as f32;
            direct_score += f64::from(key_val) * f64::from(query_val);
        }

        assert!((lookup_score - direct_score as f32).abs() < 1e-6);
    }

    #[test]
    fn roundtrip_identity() {
        let dim = 8;
        let mut scorer = PerDimScorer::new(dim, 4).unwrap();
        let v = sine_vec(dim);
        // Use several shifted vectors so per-dim range is non-degenerate.
        let v2: Vec<f32> = (0..dim).map(|i| libm::cosf(i as f32)).collect();
        let v3: Vec<f32> = (0..dim).map(|i| 0.5 * libm::sinf(i as f32)).collect();
        scorer.fit(&[&v, &v2, &v3]).unwrap();

        let compressed = scorer.compress(&v).unwrap();
        let prepared = scorer.prepare_query(&v).unwrap();
        let score = scorer.score_prepared(&prepared, &compressed).unwrap();
        let true_ip: f32 = v.iter().map(|&x| x * x).sum();
        // Scorer returns cosine (normalized inner product) since both query
        // and key are normalized before quantization.
        let norm = true_ip.sqrt();
        let expected = true_ip / (norm * norm);
        let rel_err = (score - expected).abs() / expected.abs().max(1e-6);
        assert!(
            rel_err < 0.25,
            "score={score} expected={expected} rel_err={rel_err}"
        );
    }

    #[test]
    fn higher_bits_finer_steps() {
        let dim = 16;
        let v = sine_vec(dim);
        let v2: Vec<f32> = (0..dim).map(|i| libm::cosf(i as f32)).collect();
        let batch: Vec<&[f32]> = vec![&v, &v2];

        let mut s4 = PerDimScorer::new(dim, 4).unwrap();
        s4.fit(&batch).unwrap();

        let mut s8 = PerDimScorer::new(dim, 8).unwrap();
        s8.fit(&batch).unwrap();

        let step_sum_4: f32 = s4.step.iter().sum();
        let step_sum_8: f32 = s8.step.iter().sum();
        assert!(
            step_sum_8 < step_sum_4,
            "8-bit per-dim steps should be finer than 4-bit"
        );
    }

    #[test]
    fn codec_name_and_dim() {
        let s = PerDimScorer::new(32, 4).unwrap();
        assert_eq!(s.dim(), 32);
        assert_eq!(s.codec_name(), "per_dim_quant");
    }
}
