//! Approximate inner product scoring without full decode.
//!
//! The FibQuant codebook supports estimating the inner product of a query
//! vector against a stored `FibCodeV1` without running the full decode
//! pipeline (rotation inverse + norm scaling). The method:
//!
//! 1. Normalize the query, apply the same rotation → rotated query blocks.
//! 2. For each block, find the nearest codeword index (same as encode).
//! 3. Use a precomputed codebook Gram table `G[i,j] = <cw_i, cw_j>` to
//!    estimate `<rotated_query_block, rotated_stored_block>` as
//!    `G[query_idx, stored_idx]`.
//! 4. Sum block-level estimates and scale by the stored norm.
//!
//! This avoids decoding the stored vector entirely — you only need the
//! packed indices and the stored norm, both of which are in `FibCodeV1`.
//!
//! The estimate is approximate because the stored code uses the quantized
//! codeword, not the original rotated block. The error is bounded by the
//! codebook quantization noise and decreases as codebook size grows.

use half::f16;

use crate::{
    bitpack::unpack_indices, codec::FibCodeV1, profile::FibQuantProfileV1, FibQuantError,
    FibQuantizer, Result,
};

/// Precomputed codebook Gram table for approximate inner product scoring.
///
/// `G[i, j] = <codeword_i, codeword_j>` stored as a flat `N × N` f32 matrix
/// in row-major order. For a codebook of size N=32, this is 1024 f32 values
/// (4 KB). For N=256, it's 64K values (256 KB). The table is symmetric, so
/// only the lower triangle is computed, but the full matrix is stored for
/// O(1) lookup without index arithmetic.
///
/// The table is lazily computed and cached inside `FibScorer`.
pub struct GramTable {
    /// Flat `N × N` f32 matrix, row-major. `G[i * N + j] = <cw_i, cw_j>`.
    values: Vec<f32>,
    n: usize,
}

impl GramTable {
    /// Build the Gram table from a codebook's codewords.
    ///
    /// `codewords` is the row-major `N × k` f32 array from `FibCodebookV1`.
    /// `k` is the block dimension.
    pub fn build(codewords: &[f32], n: usize, k: usize) -> Result<Self> {
        if codewords.len() != n * k {
            return Err(FibQuantError::CorruptPayload(format!(
                "codewords has {} values, expected {} (n={} k={})",
                codewords.len(),
                n * k,
                n,
                k
            )));
        }
        let mut values = vec![0.0f32; n * n];
        for i in 0..n {
            // Diagonal: <cw_i, cw_i> = ||cw_i||^2
            let mut dot_ii = 0.0f32;
            for d in 0..k {
                let vi = codewords[i * k + d];
                dot_ii += vi * vi;
            }
            values[i * n + i] = dot_ii;
            // Off-diagonal: exploit symmetry
            for j in (i + 1)..n {
                let mut dot = 0.0f32;
                for d in 0..k {
                    dot += codewords[i * k + d] * codewords[j * k + d];
                }
                values[i * n + j] = dot;
                values[j * n + i] = dot;
            }
        }
        Ok(Self { values, n })
    }

    /// Lookup `<codeword_i, codeword_j>`.
    #[inline]
    pub fn get(&self, i: usize, j: usize) -> f32 {
        debug_assert!(i < self.n && j < self.n);
        self.values[i * self.n + j]
    }

    /// Size N of the Gram table.
    pub fn n(&self) -> usize {
        self.n
    }

    /// Raw values (for serialization/digest).
    pub fn values(&self) -> &[f32] {
        &self.values
    }
}

/// Approximate scorer for FibQuant codes.
///
/// Wraps a `FibQuantizer` with a precomputed Gram table. The scorer can
/// estimate inner products between a raw query vector and a stored
/// `FibCodeV1` without decoding the stored code.
///
/// The scorer is cheap to construct (~1ms for N=32, k=4) and the Gram
/// table is cached for the lifetime of the scorer.
pub struct FibScorer {
    quantizer: FibQuantizer,
    gram: GramTable,
}

/// A scored candidate from approximate search.
#[derive(Debug, Clone)]
pub struct ScoredItem {
    /// Index in the original input slice.
    pub idx: usize,
    /// Approximate inner product estimate.
    pub score: f32,
}

/// Pre-rotated, pre-quantized query for batch scoring.
///
/// Produced by [`FibScorer::prepare_query`]. Contains everything needed to
/// score against any number of `FibCodeV1` codes without recomputing the
/// rotation or nearest-codeword search per code.
///
/// - `rotated_query`: normalized + rotated query in f32 (block-major)
/// - `query_norm`: L2 norm of the original (un-normalized) query
/// - `query_indices`: nearest codeword index per block (precomputed argmin)
#[derive(Debug, Clone)]
pub struct FibPreparedQuery {
    /// Normalized, rotated query in f32, length = `ambient_dim`.
    pub rotated_query: Vec<f32>,
    /// L2 norm of the original query vector.
    pub query_norm: f64,
    /// Nearest codeword index for each block of the rotated query.
    pub query_indices: Vec<u32>,
}

impl FibScorer {
    /// Build a scorer from a quantizer. Computes the Gram table once.
    pub fn new(quantizer: FibQuantizer) -> Result<Self> {
        let n = quantizer.profile().codebook_size as usize;
        let k = quantizer.profile().block_dim as usize;
        let gram = GramTable::build(&quantizer.codebook().codewords, n, k)?;
        Ok(Self { quantizer, gram })
    }

    /// Get a reference to the underlying quantizer.
    pub fn quantizer(&self) -> &FibQuantizer {
        &self.quantizer
    }

    /// Get a reference to the Gram table.
    pub fn gram_table(&self) -> &GramTable {
        &self.gram
    }

    /// Estimate the inner product `<query, stored_vector>` where
    /// `stored_vector` is represented by `code`.
    ///
    /// This does NOT decode the stored vector. It:
    /// 1. Normalizes the query, applies the rotation.
    /// 2. For each block, finds the nearest codeword index for the query.
    /// 3. Looks up `G[query_idx, stored_idx]` from the Gram table.
    /// 4. Sums block-level estimates and scales by the stored norm.
    ///
    /// Returns the approximate inner product.
    pub fn inner_product_estimate(&self, query: &[f32], code: &FibCodeV1) -> Result<f32> {
        let d = self.quantizer.profile().ambient_dim as usize;
        let k = self.quantizer.profile().block_dim as usize;
        if query.len() != d {
            return Err(FibQuantError::CorruptPayload(format!(
                "query dimension {}, expected {}",
                query.len(),
                d
            )));
        }
        // Check query is finite
        if query.iter().any(|v| !v.is_finite()) {
            return Err(FibQuantError::NonFiniteInput(0));
        }
        let query_norm: f64 = query
            .iter()
            .map(|v| (*v as f64) * (*v as f64))
            .sum::<f64>()
            .sqrt();
        if query_norm == 0.0 {
            return Ok(0.0);
        }
        // Normalize and rotate the query
        let normalized: Vec<f64> = query.iter().map(|v| f64::from(*v) / query_norm).collect();
        let rotated = self.quantizer.profile();
        let _ = rotated; // suppress unused warning
        let rotated_query = self.quantizer_codebook_rotation_apply(&normalized)?;

        // Decode the stored norm
        let stored_norm = decode_stored_norm(code, self.quantizer.profile())?;

        // Unpack stored indices
        let block_count = self.quantizer.profile().block_count() as usize;
        let stored_indices = unpack_indices(
            &code.indices,
            block_count,
            self.quantizer.profile().wire_index_bits,
        )?;

        // For each block, find the nearest codeword for the query block
        let rotated_query_f32: Vec<f32> = rotated_query.iter().map(|&v| v as f32).collect();
        let codewords = &self.quantizer.codebook().codewords;
        let n = self.quantizer.profile().codebook_size as usize;

        let mut total = 0.0f32;
        for (block_idx, stored_idx) in stored_indices.iter().enumerate() {
            let stored_idx = *stored_idx as usize;
            if stored_idx >= n {
                return Err(FibQuantError::IndexOutOfRange {
                    index: stored_idx as u32,
                    codebook_size: n as u32,
                });
            }
            let query_block = &rotated_query_f32[block_idx * k..(block_idx + 1) * k];
            let query_idx =
                crate::ffi::c_encode_vector_block(query_block, codewords, n, k)[0] as usize;
            // Gram table lookup: <cw_query, cw_stored>
            total += self.gram.get(query_idx, stored_idx);
        }

        // Scale by query norm and stored norm
        Ok(total * (query_norm as f32) * (stored_norm as f32))
    }

    /// Score a batch of stored codes against a single query.
    ///
    /// Returns `Vec<(idx, score)>` sorted by descending score.
    pub fn score_batch(&self, query: &[f32], codes: &[FibCodeV1]) -> Result<Vec<ScoredItem>> {
        let mut results = Vec::with_capacity(codes.len());
        for (idx, code) in codes.iter().enumerate() {
            let score = self.inner_product_estimate(query, code)?;
            results.push(ScoredItem { idx, score });
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }

    /// Search the top-K closest codes to a query, with optional oversampling.
    ///
    /// Returns the top-K `ScoredItem`s by approximate inner product.
    /// `oversample > 1` returns more candidates than `top_k` for downstream
    /// exact reranking.
    pub fn search(
        &self,
        query: &[f32],
        codes: &[FibCodeV1],
        top_k: usize,
        oversample: usize,
    ) -> Result<Vec<ScoredItem>> {
        let limit = top_k.saturating_mul(oversample.max(1)).min(codes.len());
        let scored = self.score_batch(query, codes)?;
        Ok(scored.into_iter().take(limit).collect())
    }

    // ──────────────────────────────────────────────────────────────────
    //  Prepared-query path — rotate/argmin ONCE, score many codes
    // ──────────────────────────────────────────────────────────────────

    /// Prepare a query for batch scoring.
    ///
    /// Normalizes the query, applies the rotation, converts to f32, and
    /// precomputes the nearest-codeword index for each block. The resulting
    /// [`FibPreparedQuery`] can be passed to [`score_prepared`](Self::score_prepared),
    /// [`score_batch_prepared`](Self::score_batch_prepared), or
    /// [`search_prepared`](Self::search_prepared) to avoid recomputing the
    /// rotation and argmin for every code in a batch.
    pub fn prepare_query(&self, query: &[f32]) -> Result<FibPreparedQuery> {
        let d = self.quantizer.profile().ambient_dim as usize;
        let k = self.quantizer.profile().block_dim as usize;
        if query.len() != d {
            return Err(FibQuantError::CorruptPayload(format!(
                "query dimension {}, expected {}",
                query.len(),
                d
            )));
        }
        if query.iter().any(|v| !v.is_finite()) {
            return Err(FibQuantError::NonFiniteInput(0));
        }

        let query_norm: f64 = query
            .iter()
            .map(|v| (*v as f64) * (*v as f64))
            .sum::<f64>()
            .sqrt();
        if query_norm == 0.0 {
            // Return a zero-filled prepared query — all scores will be 0.
            let block_count = self.quantizer.profile().block_count() as usize;
            return Ok(FibPreparedQuery {
                rotated_query: vec![0.0f32; d],
                query_norm: 0.0,
                query_indices: vec![0u32; block_count],
            });
        }

        // Normalize and rotate
        let normalized: Vec<f64> = query.iter().map(|v| f64::from(*v) / query_norm).collect();
        let rotated_query = self.quantizer_codebook_rotation_apply(&normalized)?;
        let rotated_query_f32: Vec<f32> = rotated_query.iter().map(|&v| v as f32).collect();

        // Precompute nearest codeword index per block
        let _block_count = self.quantizer.profile().block_count() as usize;
        let codewords = &self.quantizer.codebook().codewords;
        let n = self.quantizer.profile().codebook_size as usize;
        let c_indices = crate::ffi::c_encode_vector_block(&rotated_query_f32, codewords, n, k);
        let query_indices: Vec<u32> = c_indices.iter().map(|&i| i as u32).collect();

        Ok(FibPreparedQuery {
            rotated_query: rotated_query_f32,
            query_norm,
            query_indices,
        })
    }

    /// Score a single code against a prepared query.
    ///
    /// This skips the rotation and argmin steps (already done in
    /// [`prepare_query`](Self::prepare_query)) and only performs:
    /// 1. Unpack the stored code's indices.
    /// 2. For each block: Gram table lookup `G[query_idx, stored_idx]`.
    /// 3. Sum and scale by `query_norm * stored_norm`.
    pub fn score_prepared(&self, prepared: &FibPreparedQuery, code: &FibCodeV1) -> Result<f32> {
        if prepared.query_norm == 0.0 {
            return Ok(0.0);
        }

        let block_count = self.quantizer.profile().block_count() as usize;
        let stored_indices = unpack_indices(
            &code.indices,
            block_count,
            self.quantizer.profile().wire_index_bits,
        )?;

        let stored_norm = decode_stored_norm(code, self.quantizer.profile())?;
        let n = self.quantizer.profile().codebook_size as usize;

        let mut total = 0.0f32;
        for (block_idx, stored_idx) in stored_indices.iter().enumerate() {
            let stored_idx = *stored_idx as usize;
            if stored_idx >= n {
                return Err(FibQuantError::IndexOutOfRange {
                    index: stored_idx as u32,
                    codebook_size: n as u32,
                });
            }
            let query_idx = prepared.query_indices[block_idx] as usize;
            total += self.gram.get(query_idx, stored_idx);
        }

        Ok(total * (prepared.query_norm as f32) * (stored_norm as f32))
    }

    /// Score a batch of codes against a prepared query.
    ///
    /// Like [`score_batch`](Self::score_batch) but uses the pre-rotated query,
    /// avoiding redundant rotation + argmin per code.
    /// Returns `Vec<ScoredItem>` sorted by descending score.
    pub fn score_batch_prepared(
        &self,
        prepared: &FibPreparedQuery,
        codes: &[FibCodeV1],
    ) -> Result<Vec<ScoredItem>> {
        let mut results = Vec::with_capacity(codes.len());
        for (idx, code) in codes.iter().enumerate() {
            let score = self.score_prepared(prepared, code)?;
            results.push(ScoredItem { idx, score });
        }
        results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(results)
    }

    /// Search top-K closest codes to a prepared query, with optional oversampling.
    ///
    /// Like [`search`](Self::search) but uses the pre-rotated query.
    pub fn search_prepared(
        &self,
        prepared: &FibPreparedQuery,
        codes: &[FibCodeV1],
        top_k: usize,
        oversample: usize,
    ) -> Result<Vec<ScoredItem>> {
        let limit = top_k.saturating_mul(oversample.max(1)).min(codes.len());
        let scored = self.score_batch_prepared(prepared, codes)?;
        Ok(scored.into_iter().take(limit).collect())
    }

    /// Apply the rotation to a query vector (internal helper).
    fn quantizer_codebook_rotation_apply(&self, x: &[f64]) -> Result<Vec<f64>> {
        // Use the exposed rotation from the quantizer — no reconstruction.
        self.quantizer.rotation().apply(x)
    }

    /// Estimate L2 distance ||query - stored|| without decoding the stored vector.
    ///
    /// Uses the identity: ||q - v||^2 = ||q||^2 + ||v||^2 - 2<q, v>
    /// where <q, v> is the approximate inner product from the Gram table.
    /// Returns the estimated squared L2 distance (avoiding a sqrt for
    /// comparison purposes — callers can sqrt if needed).
    pub fn l2_distance_sq_estimate(&self, query: &[f32], code: &FibCodeV1) -> Result<f32> {
        let ip = self.inner_product_estimate(query, code)?;
        let q_norm_sq: f32 = query.iter().map(|v| v * v).sum();
        let stored_norm = decode_stored_norm(code, self.quantizer.profile())? as f32;
        let v_norm_sq = stored_norm * stored_norm;
        Ok((q_norm_sq + v_norm_sq - 2.0 * ip).max(0.0))
    }

    /// Estimate cosine similarity <query, stored> / (||query|| * ||stored||)
    /// without decoding the stored vector.
    pub fn cosine_estimate(&self, query: &[f32], code: &FibCodeV1) -> Result<f32> {
        let ip = self.inner_product_estimate(query, code)?;
        let q_norm: f32 = query.iter().map(|v| v * v).sum::<f32>().sqrt();
        let stored_norm = decode_stored_norm(code, self.quantizer.profile())? as f32;
        if q_norm == 0.0 || stored_norm == 0.0 {
            return Ok(0.0);
        }
        Ok(ip / (q_norm * stored_norm))
    }

    /// L2 distance using a prepared query — avoids recomputing query rotation.
    pub fn l2_distance_sq_prepared(
        &self,
        prepared: &FibPreparedQuery,
        code: &FibCodeV1,
    ) -> Result<f32> {
        let ip = self.score_prepared(prepared, code)?;
        let q_norm_sq = (prepared.query_norm * prepared.query_norm) as f32;
        let stored_norm = decode_stored_norm(code, self.quantizer.profile())? as f32;
        let v_norm_sq = stored_norm * stored_norm;
        Ok((q_norm_sq + v_norm_sq - 2.0 * ip).max(0.0))
    }

    /// Cosine similarity using a prepared query.
    pub fn cosine_prepared(&self, prepared: &FibPreparedQuery, code: &FibCodeV1) -> Result<f32> {
        let ip = self.score_prepared(prepared, code)?;
        let q_norm = prepared.query_norm as f32;
        let stored_norm = decode_stored_norm(code, self.quantizer.profile())? as f32;
        if q_norm == 0.0 || stored_norm == 0.0 {
            return Ok(0.0);
        }
        Ok(ip / (q_norm * stored_norm))
    }
}

pub fn decode_stored_norm(code: &FibCodeV1, _profile: &FibQuantProfileV1) -> Result<f64> {
    // Decode the norm from the code's norm_payload. We handle both
    // Fp16Paper and F32Reference formats here to avoid depending on
    // the private decode_norm function in codec.rs.
    use crate::profile::NormFormat;
    match code.norm_format {
        NormFormat::Fp16Paper => {
            let bytes: [u8; 2] =
                code.norm_payload.as_slice().try_into().map_err(|_| {
                    FibQuantError::CorruptPayload("fp16 norm payload length".into())
                })?;
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
                .map_err(|_| FibQuantError::CorruptPayload("f32 norm payload length".into()))?;
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

    fn build_test_scorer() -> Result<FibScorer> {
        let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
        profile.training_samples = 128;
        profile.lloyd_restarts = 1;
        profile.lloyd_iterations = 2;
        let quantizer = FibQuantizer::new(profile)?;
        FibScorer::new(quantizer)
    }

    #[test]
    fn gram_table_diagonal_matches_codeword_norms() -> Result<()> {
        let scorer = build_test_scorer()?;
        let k = scorer.quantizer.profile().block_dim as usize;
        let n = scorer.quantizer.profile().codebook_size as usize;
        let codewords = &scorer.quantizer.codebook().codewords;
        for i in 0..n {
            let mut norm_sq = 0.0f32;
            for d in 0..k {
                let v = codewords[i * k + d];
                norm_sq += v * v;
            }
            let gram_diag = scorer.gram.get(i, i);
            assert!(
                (norm_sq - gram_diag).abs() < 1e-5,
                "gram diagonal mismatch at {}: ||cw||^2 = {}, gram = {}",
                i,
                norm_sq,
                gram_diag
            );
        }
        Ok(())
    }

    #[test]
    fn gram_table_symmetric() -> Result<()> {
        let scorer = build_test_scorer()?;
        let n = scorer.gram.n();
        for i in 0..n {
            for j in 0..n {
                assert!(
                    (scorer.gram.get(i, j) - scorer.gram.get(j, i)).abs() < 1e-6,
                    "gram not symmetric at ({}, {})",
                    i,
                    j
                );
            }
        }
        Ok(())
    }

    #[test]
    fn inner_product_estimate_positive_for_self() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer.profile().ambient_dim as usize;
        let input: Vec<f32> = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        assert_eq!(input.len(), d);
        let code = scorer.quantizer.encode(&input)?;
        let est = scorer.inner_product_estimate(&input, &code)?;
        // <x, x> > 0 for a non-zero vector
        assert!(
            est > 0.0,
            "inner product estimate of self should be positive, got {}",
            est
        );
        // It should be in the ballpark of ||x||^2
        let true_ip: f32 = input.iter().map(|v| v * v).sum();
        let ratio = est / true_ip;
        assert!(
            ratio > 0.5 && ratio < 2.0,
            "estimate {} vs true {} — ratio {} out of [0.5, 2.0]",
            est,
            true_ip,
            ratio
        );
        Ok(())
    }

    #[test]
    fn search_returns_sorted_descending() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer.profile().ambient_dim as usize;
        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        assert_eq!(query.len(), d);

        // Encode several vectors
        let vectors: Vec<Vec<f32>> = (0..16)
            .map(|seed| {
                (0..d)
                    .map(|i| (seed as f32 * 0.1 + i as f32 * 0.05 - 0.3).sin())
                    .collect()
            })
            .collect();
        let codes: Vec<FibCodeV1> = vectors
            .iter()
            .map(|v| scorer.quantizer.encode(v).unwrap())
            .collect();

        let results = scorer.search(&query, &codes, 5, 2)?;
        // top_k=5, oversample=2 → returns min(5*2, 16) = 10 candidates
        assert_eq!(results.len(), 10);
        for w in results.windows(2) {
            assert!(
                w[0].score >= w[1].score,
                "results not sorted: {} before {}",
                w[0].score,
                w[1].score
            );
        }
        Ok(())
    }

    #[test]
    fn score_batch_handles_empty() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer.profile().ambient_dim as usize;
        let query = vec![0.0f32; d];
        let results = scorer.score_batch(&query, &[])?;
        assert!(results.is_empty());
        Ok(())
    }

    #[test]
    fn prepared_query_matches_inner_product_estimate() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer.profile().ambient_dim as usize;
        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        assert_eq!(query.len(), d);

        // Encode several vectors
        let vectors: Vec<Vec<f32>> = (0..16)
            .map(|seed| {
                (0..d)
                    .map(|i| (seed as f32 * 0.1 + i as f32 * 0.05 - 0.3).sin())
                    .collect()
            })
            .collect();
        let codes: Vec<FibCodeV1> = vectors
            .iter()
            .map(|v| scorer.quantizer.encode(v).unwrap())
            .collect();

        let prepared = scorer.prepare_query(&query)?;
        for (i, code) in codes.iter().enumerate() {
            let direct = scorer.inner_product_estimate(&query, code)?;
            let prepared_score = scorer.score_prepared(&prepared, code)?;
            assert!(
                (direct - prepared_score).abs() < 1e-4,
                "mismatch at code {}: direct={}, prepared={}",
                i,
                direct,
                prepared_score
            );
        }
        Ok(())
    }

    #[test]
    fn prepared_batch_matches_score_batch() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer.profile().ambient_dim as usize;
        let query: Vec<f32> = vec![0.3, 0.7, -0.2, 0.9, -0.5, 0.1, -0.8, 0.4];
        assert_eq!(query.len(), d);

        let vectors: Vec<Vec<f32>> = (0..24)
            .map(|seed| {
                (0..d)
                    .map(|i| ((seed as f32 + i as f32) * 0.13).cos())
                    .collect()
            })
            .collect();
        let codes: Vec<FibCodeV1> = vectors
            .iter()
            .map(|v| scorer.quantizer.encode(v).unwrap())
            .collect();

        let batch = scorer.score_batch(&query, &codes)?;
        let prepared = scorer.prepare_query(&query)?;
        let batch_prepared = scorer.score_batch_prepared(&prepared, &codes)?;

        assert_eq!(batch.len(), batch_prepared.len());
        for (a, b) in batch.iter().zip(batch_prepared.iter()) {
            assert_eq!(a.idx, b.idx);
            assert!(
                (a.score - b.score).abs() < 1e-4,
                "score mismatch at idx {}: batch={}, prepared={}",
                a.idx,
                a.score,
                b.score
            );
        }
        Ok(())
    }

    #[test]
    fn prepared_search_matches_search() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer.profile().ambient_dim as usize;
        let query: Vec<f32> = vec![0.6, -0.1, 0.3, -0.7, 0.8, 0.2, -0.4, 0.5];
        assert_eq!(query.len(), d);

        let vectors: Vec<Vec<f32>> = (0..32)
            .map(|seed| {
                (0..d)
                    .map(|i| (seed as f32 * 0.17 + i as f32 * 0.03).sin())
                    .collect()
            })
            .collect();
        let codes: Vec<FibCodeV1> = vectors
            .iter()
            .map(|v| scorer.quantizer.encode(v).unwrap())
            .collect();

        let direct = scorer.search(&query, &codes, 5, 2)?;
        let prepared = scorer.prepare_query(&query)?;
        let prepared_results = scorer.search_prepared(&prepared, &codes, 5, 2)?;

        assert_eq!(direct.len(), prepared_results.len());
        for (a, b) in direct.iter().zip(prepared_results.iter()) {
            assert_eq!(a.idx, b.idx);
            assert!(
                (a.score - b.score).abs() < 1e-4,
                "search mismatch at idx {}: direct={}, prepared={}",
                a.idx,
                a.score,
                b.score
            );
        }
        Ok(())
    }

    #[test]
    fn prepared_query_zero_norm() -> Result<()> {
        let scorer = build_test_scorer()?;
        let d = scorer.quantizer.profile().ambient_dim as usize;
        let query = vec![0.0f32; d];
        let prepared = scorer.prepare_query(&query)?;
        assert_eq!(prepared.query_norm, 0.0);

        let input: Vec<f32> = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = scorer.quantizer.encode(&input)?;
        let score = scorer.score_prepared(&prepared, &code)?;
        assert!(score.abs() < 1e-6, "zero query should give zero score");
        Ok(())
    }

    #[test]
    fn l2_distance_is_non_negative() -> Result<()> {
        let scorer = build_test_scorer()?;
        let input: Vec<f32> = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = scorer.quantizer.encode(&input)?;
        let dist = scorer.l2_distance_sq_estimate(&input, &code)?;
        assert!(
            dist >= 0.0,
            "L2 distance squared should be non-negative, got {}",
            dist
        );
        Ok(())
    }

    #[test]
    fn cosine_estimate_in_valid_range() -> Result<()> {
        let scorer = build_test_scorer()?;
        let input: Vec<f32> = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = scorer.quantizer.encode(&input)?;
        let cos = scorer.cosine_estimate(&input, &code)?;
        assert!(
            (-1.5..=1.5).contains(&cos),
            "cosine should be in [-1.5, 1.5], got {}",
            cos
        );
        Ok(())
    }

    #[test]
    fn cosine_prepared_matches_cosine_estimate() -> Result<()> {
        let scorer = build_test_scorer()?;
        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let input: Vec<f32> = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = scorer.quantizer.encode(&input)?;
        let cos_direct = scorer.cosine_estimate(&query, &code)?;
        let prepared = scorer.prepare_query(&query)?;
        let cos_prepared = scorer.cosine_prepared(&prepared, &code)?;
        assert!(
            (cos_direct - cos_prepared).abs() < 1e-5,
            "prepared cosine {} should match direct {}",
            cos_prepared,
            cos_direct
        );
        Ok(())
    }

    #[test]
    fn l2_prepared_matches_l2_estimate() -> Result<()> {
        let scorer = build_test_scorer()?;
        let query: Vec<f32> = vec![0.5, -0.3, 0.8, -0.1, 0.2, -0.4, 0.7, -0.6];
        let input: Vec<f32> = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
        let code = scorer.quantizer.encode(&input)?;
        let dist_direct = scorer.l2_distance_sq_estimate(&query, &code)?;
        let prepared = scorer.prepare_query(&query)?;
        let dist_prepared = scorer.l2_distance_sq_prepared(&prepared, &code)?;
        assert!(
            (dist_direct - dist_prepared).abs() < 1e-5,
            "prepared L2 {} should match direct {}",
            dist_prepared,
            dist_direct
        );
        Ok(())
    }
}
