//! turbo-quant adapter — PolarQuant + QJL compressed scoring
//!
//! Uses turbo-quant's TurboQuantizer to estimate inner products via
//! polar-coordinate quantization after seeded rotation.

use crate::error::{ScorerError, ScorerResult};
use crate::trait_def::{CompressedScorer, PreparedQuery};

#[cfg(feature = "no_std")]
use alloc::{format, vec::Vec};

use turbo_quant::{TurboCode, TurboProjectedQuery, TurboQuantizer};

/// Adapter wrapping turbo-quant's TurboQuantizer.
pub struct TurboScorerAdapter {
    quantizer: TurboQuantizer,
    dim: usize,
}

/// Wrapper for turbo-quant's prepared query
pub struct TurboPreparedWrapper {
    inner: TurboProjectedQuery,
    dim: usize,
}

impl PreparedQuery for TurboPreparedWrapper {
    fn dim(&self) -> usize {
        self.dim
    }
}

impl TurboScorerAdapter {
    /// Create a new adapter.
    ///
    /// - `dim`: vector dimensionality
    /// - `bits`: bits per scalar for the polar quantizer (typically 4-8)
    /// - `projections`: number of projection vectors (typically dim/4 to dim/8)
    /// - `seed`: random seed for deterministic rotation
    pub fn new(dim: usize, bits: u8, projections: usize, seed: u64) -> ScorerResult<Self> {
        let quantizer = TurboQuantizer::new(dim, bits, projections, seed)
            .map_err(|e| ScorerError::ScoringFailed(format!("TurboQuantizer: {e}")))?;
        Ok(Self { quantizer, dim })
    }

    /// Access the inner TurboQuantizer
    pub fn inner(&self) -> &TurboQuantizer {
        &self.quantizer
    }

    /// Encode a raw f32 vector into a compressed TurboCode
    pub fn encode(&self, vector: &[f32]) -> ScorerResult<TurboCode> {
        self.quantizer
            .encode(vector)
            .map_err(|e| ScorerError::CorruptPayload(format!("encode: {e}")))
    }

    /// Encode a batch of vectors
    pub fn encode_batch(&self, vectors: &[&[f32]]) -> ScorerResult<Vec<TurboCode>> {
        vectors.iter().map(|v| self.encode(v)).collect()
    }
}

impl CompressedScorer for TurboScorerAdapter {
    type Prepared = TurboPreparedWrapper;
    type Compressed = TurboCode;

    fn prepare_query(&self, query: &[f32]) -> ScorerResult<Self::Prepared> {
        if query.len() != self.dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.dim,
                got: query.len(),
            });
        }
        let inner = self
            .quantizer
            .prepare_query(query)
            .map_err(|e| ScorerError::ScoringFailed(format!("prepare_query: {e}")))?;
        Ok(TurboPreparedWrapper {
            inner,
            dim: self.dim,
        })
    }

    fn score_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        // Note: turbo-quant's API takes (code, query) not (query, code)
        self.quantizer
            .inner_product_estimate_prepared(compressed, &prepared.inner)
            .map_err(|e| ScorerError::ScoringFailed(format!("score: {e}")))
    }

    fn decode(&self, compressed: &Self::Compressed) -> ScorerResult<Vec<f32>> {
        // turbo-quant has decode_approximate for approximate reconstruction
        self.quantizer
            .decode_approximate(compressed)
            .map_err(|e| ScorerError::CorruptPayload(format!("decode: {e}")))
    }

    fn cosine_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        // For turbo-quant, inner_product_estimate IS the cosine estimate
        // when vectors are normalized
        self.score_prepared(prepared, compressed)
    }

    fn l2_distance_sq_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        let ip = self.score_prepared(prepared, compressed)?;
        Ok(2.0 - 2.0 * ip)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn codec_name(&self) -> &'static str {
        "turbo_quant"
    }

    fn internal_bytes(&self) -> usize {
        // TurboQuantizer state: reconstructable from (dim, bits, projections, seed)
        16
    }
}
