//! Core trait: CompressedScorer
//!
//! A codec-agnostic interface for scoring (estimating similarity) against
//! compressed vector representations without decompressing them.
//!
//! Implementations:
//! - [FibScorerAdapter](crate::fib_impl::FibScorerAdapter) — Gram-table lookup
//! - [TurboScorerAdapter](crate::turbo_impl::TurboScorerAdapter) — Polar/QJL estimates

#[cfg(feature = "no_std")]
use alloc::vec::Vec;

use crate::error::ScorerResult;

/// Stage used by progressive compressed-domain scoring.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScoreStage {
    /// Fastest approximate score.
    Coarse,
    /// Refined approximate score after margin-band selection.
    Refined,
    /// Authoritative/exact fallback score.
    ExactFallback,
}

/// A score with an optional absolute error bound.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScoreWithUncertainty {
    pub score: f32,
    pub error_bound: Option<f32>,
    pub stage: ScoreStage,
}

impl ScoreWithUncertainty {
    pub fn coarse(score: f32) -> Self {
        Self {
            score,
            error_bound: None,
            stage: ScoreStage::Coarse,
        }
    }

    pub fn with_bound(score: f32, error_bound: f32, stage: ScoreStage) -> Self {
        Self {
            score,
            error_bound: Some(error_bound),
            stage,
        }
    }

    pub fn lower_bound(&self) -> f32 {
        self.score - self.error_bound.unwrap_or(0.0)
    }

    pub fn upper_bound(&self) -> f32 {
        self.score + self.error_bound.unwrap_or(0.0)
    }
}

/// An opaque prepared query — pre-rotated and pre-quantized for batch scoring.
///
/// The concrete type depends on the codec implementation. Callers obtain one
/// via [`CompressedScorer::prepare_query`] and pass it to
/// [`CompressedScorer::score_prepared`] for each candidate.
pub trait PreparedQuery: Send + Sync {
    /// Dimensionality of the query vector
    fn dim(&self) -> usize;
}

/// A codec-agnostic compressed-domain scorer.
///
/// Implementations wrap a specific codec (fib-quant, turbo-quant) and provide:
/// 1. Query preparation (once per search)
/// 2. O(1) scoring per compressed vector (no decompression)
/// 3. Optional decode for top-K verification
///
/// The key optimization: scoring N vectors costs O(N) cheap lookups, not
/// O(N * dim) decompressions + dot products.
pub trait CompressedScorer: Send + Sync {
    /// The prepared query type for this codec
    type Prepared: PreparedQuery;

    /// The compressed representation type (codec-specific)
    type Compressed: Send + Sync;

    /// Prepare a query for batch scoring.
    ///
    /// This performs the codec-specific query transformation (rotation,
    /// quantization, projection) once. The result is reused for all
    /// `score_prepared` calls in this search round.
    fn prepare_query(&self, query: &[f32]) -> ScorerResult<Self::Prepared>;

    /// Score a single compressed vector against a prepared query.
    ///
    /// This is the hot-path call — it should be O(1) or O(blocks) per
    /// vector, NOT O(dim). No decompression occurs.
    fn score_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32>;

    /// Score all compressed vectors against a prepared query.
    ///
    /// Default implementation iterates, but codecs may override with
    /// batch-optimized paths (SIMD, parallel, etc.).
    fn score_batch_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &[Self::Compressed],
    ) -> ScorerResult<Vec<f32>> {
        compressed
            .iter()
            .map(|c| self.score_prepared(prepared, c))
            .collect()
    }

    /// Decode a compressed vector back to f32.
    ///
    /// Used for exact verification of top-K candidates after approximate
    /// scoring. This is the ONLY path that decompresses.
    fn decode(&self, compressed: &Self::Compressed) -> ScorerResult<Vec<f32>>;

    /// Cosine similarity estimate (for normalized vectors, same as inner product).
    ///
    /// Default delegates to score_prepared, but codecs with explicit cosine
    /// implementations can override.
    fn cosine_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        self.score_prepared(prepared, compressed)
    }

    /// L2 distance estimate (squared, not sqrt).
    ///
    /// Default: `||q||^2 + ||x||^2 - 2*<q,x>` where `<q,x>` is the inner
    /// product estimate from `score_prepared`.
    fn l2_distance_sq_prepared(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<f32> {
        // For compressed codecs, ||x||^2 is often stored alongside the code.
        // The default implementation uses the inner product estimate and
        // assumes normalized vectors (||x||^2 ≈ 1, ||q||^2 ≈ 1).
        let ip = self.score_prepared(prepared, compressed)?;
        // L2^2 ≈ 2 - 2*<q,x> for unit vectors
        Ok(2.0 - 2.0 * ip)
    }

    /// Vector dimensionality
    fn dim(&self) -> usize;

    /// Codec name (e.g. "fib_quant", "turbo_quant", "polar", "qjl")
    fn codec_name(&self) -> &'static str;

    /// Memory used by the scorer's internal state (Gram table, etc.)
    fn internal_bytes(&self) -> usize;
}

/// A scored candidate from compressed-domain search.
#[derive(Debug, Clone)]
pub struct ScoredCandidate {
    /// Index in the original input slice
    pub idx: usize,
    /// Approximate score (inner product, cosine, or distance depending on codec)
    pub score: f32,
}

impl ScoredCandidate {
    pub fn new(idx: usize, score: f32) -> Self {
        Self { idx, score }
    }
}

impl PartialEq for ScoredCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.idx == other.idx
    }
}

impl Eq for ScoredCandidate {}
/// Optional extension for staged coarse-to-fine compressed scoring.
pub trait ProgressiveCompressedScorer: CompressedScorer {
    /// Coarse score used for full-corpus/page scans.
    fn score_coarse(
        &self,
        prepared: &Self::Prepared,
        compressed: &Self::Compressed,
    ) -> ScorerResult<ScoreWithUncertainty> {
        Ok(ScoreWithUncertainty::coarse(
            self.score_prepared(prepared, compressed)?,
        ))
    }

    /// Refine a margin-band candidate set. Default promotes coarse scores to refined.
    fn refine_candidates(
        &self,
        prepared: &Self::Prepared,
        candidates: &mut [ProgressiveScoredCandidate],
        compressed: &[Self::Compressed],
    ) -> ScorerResult<()> {
        for candidate in candidates.iter_mut() {
            if let Some(code) = compressed.get(candidate.idx) {
                candidate.score = ScoreWithUncertainty {
                    score: self.score_prepared(prepared, code)?,
                    error_bound: candidate.score.error_bound,
                    stage: ScoreStage::Refined,
                };
            }
        }
        Ok(())
    }
}

impl<T: CompressedScorer> ProgressiveCompressedScorer for T {}

/// Candidate carrying progressive-stage metadata.
#[derive(Debug, Clone, PartialEq)]
pub struct ProgressiveScoredCandidate {
    pub idx: usize,
    pub score: ScoreWithUncertainty,
}

impl ProgressiveScoredCandidate {
    pub fn new(idx: usize, score: ScoreWithUncertainty) -> Self {
        Self { idx, score }
    }
}
