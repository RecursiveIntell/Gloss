//! fib-quant adapter — Gram-table compressed scoring
//!
//! Uses fib-quant's FibScorer to estimate inner products via precomputed
//! codebook Gram table lookups. O(1) per scored vector.

use crate::error::{ScorerError, ScorerResult};
use crate::trait_def::{CompressedScorer, PreparedQuery};

#[cfg(feature = "no_std")]
use alloc::{format, vec::Vec};

use fib_quant::{FibCodeV1, FibPreparedQuery, FibQuantizer, FibScorer};

/// Adapter wrapping fib-quant's FibScorer.
pub struct FibScorerAdapter {
    scorer: FibScorer,
    dim: usize,
}

/// Wrapper to make FibPreparedQuery implement our PreparedQuery trait
pub struct FibPreparedWrapper {
    inner: FibPreparedQuery,
    dim: usize,
}

impl PreparedQuery for FibPreparedWrapper {
    fn dim(&self) -> usize {
        self.dim
    }
}

impl FibScorerAdapter {
    /// Create a new adapter from a FibQuantizer.
    pub fn new(quantizer: FibQuantizer) -> ScorerResult<Self> {
        let dim = quantizer.profile().ambient_dim as usize;
        let scorer = FibScorer::new(quantizer)
            .map_err(|e| ScorerError::ScoringFailed(format!("FibScorer construction: {e}")))?;
        Ok(Self { scorer, dim })
    }

    /// Create from profile parameters (convenience constructor).
    pub fn from_params(
        ambient_dim: usize,
        block_dim: usize,
        codebook_size: usize,
        _bits: u64,
        seed: u64,
    ) -> ScorerResult<Self> {
        use fib_quant::FibQuantProfileV1;
        let profile = FibQuantProfileV1::paper_default(ambient_dim, block_dim, codebook_size, seed)
            .map_err(|e| ScorerError::ScoringFailed(format!("profile: {e}")))?;
        let quantizer = FibQuantizer::new(profile)
            .map_err(|e| ScorerError::ScoringFailed(format!("quantizer: {e}")))?;
        Self::new(quantizer)
    }

    /// Access the inner FibScorer
    pub fn inner(&self) -> &FibScorer {
        &self.scorer
    }

    /// Encode a raw f32 vector into a compressed FibCodeV1.
    pub fn encode(&self, vector: &[f32]) -> ScorerResult<FibCodeV1> {
        self.scorer
            .quantizer()
            .encode(vector)
            .map_err(|e| ScorerError::CorruptPayload(format!("encode: {e}")))
    }

    /// Encode a batch of vectors.
    pub fn encode_batch(&self, vectors: &[&[f32]]) -> ScorerResult<Vec<FibCodeV1>> {
        vectors.iter().map(|v| self.encode(v)).collect()
    }
}

impl CompressedScorer for FibScorerAdapter {
    type Prepared = FibPreparedWrapper;
    type Compressed = FibCodeV1;

    fn prepare_query(&self, query: &[f32]) -> ScorerResult<Self::Prepared> {
        if query.len() != self.dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: query.len(),
            });
        }
        let inner = self
            .scorer
            .prepare_query(query)
            .map_err(|e| ScorerError::ScoringFailed(format!("prepare_query: {e}")))?;
        Ok(FibPreparedWrapper {
            inner,
            dim: self.dim,
        })
    }

    fn score_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        self.scorer
            .score_prepared(&prepared.inner, compressed)
            .map_err(|e| ScorerError::ScoringFailed(format!("score: {e}")))
    }

    fn decode(&self, compressed: &Self::Compressed) -> ScorerResult<Vec<f32>> {
        self.scorer
            .quantizer()
            .decode(compressed)
            .map_err(|e| ScorerError::CorruptPayload(format!("decode: {e}")))
    }

    fn cosine_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        self.scorer
            .cosine_prepared(&prepared.inner, compressed)
            .map_err(|e| ScorerError::ScoringFailed(format!("cosine: {e}")))
    }

    fn l2_distance_sq_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        self.scorer
            .l2_distance_sq_prepared(&prepared.inner, compressed)
            .map_err(|e| ScorerError::ScoringFailed(format!("l2: {e}")))
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn codec_name(&self) -> &'static str {
        "fib_quant"
    }

    fn internal_bytes(&self) -> usize {
        let n = self.scorer.gram_table().n();
        n * n * 4
    }
}
