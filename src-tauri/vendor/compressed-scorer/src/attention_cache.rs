//! Generic compressed-domain attention cache built on [`CompressedScorer`].
//!
//! This module is intentionally codec-agnostic and `no_std`-friendly (with
//! `alloc`). It is the shared embedded path for ESP32-S3 style attention: score
//! compressed keys directly, then decode only selected values.

#[cfg(feature = "no_std")]
use alloc::{vec, vec::Vec};

#[cfg(not(feature = "no_std"))]
use std::vec::Vec;

use crate::error::{ScorerError, ScorerResult};
use crate::trait_def::CompressedScorer;

/// Output of compressed-domain attention.
#[derive(Debug, Clone, PartialEq)]
pub struct AttentionOutput {
    /// Pre-softmax compressed-domain logits, one per cached token.
    pub logits: Vec<f32>,
    /// Stable softmax probabilities over `logits`.
    pub probabilities: Vec<f32>,
    /// Weighted aggregate over decoded top-k values.
    pub output: Vec<f32>,
    /// Indices decoded for the output aggregation, sorted by probability desc.
    pub top_k_indices: Vec<usize>,
    /// Number of compressed values decoded. This is bounded by top_k, not cache size.
    pub decompression_count: usize,
}

/// One-head compressed attention cache.
///
/// Keys and values use the scorer's compressed representation. Attention logits
/// are computed with `score_prepared()` against compressed keys only; value
/// vectors are decoded only for the top-k probability positions.
pub struct AttentionCache<S: CompressedScorer> {
    scorer: S,
    keys: Vec<S::Compressed>,
    values: Vec<S::Compressed>,
    head_dim: usize,
}

impl<S: CompressedScorer> AttentionCache<S> {
    /// Build an empty cache for one attention head.
    pub fn new(scorer: S) -> Self {
        let head_dim = scorer.dim();
        Self {
            scorer,
            keys: Vec::new(),
            values: Vec::new(),
            head_dim,
        }
    }

    /// Number of cached token positions.
    pub fn len(&self) -> usize {
        self.keys.len()
    }

    /// True when no positions are cached.
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }

    /// Borrow the underlying scorer for codec-specific inspection.
    pub fn scorer(&self) -> &S {
        &self.scorer
    }

    /// Append one compressed key/value token pair.
    pub fn push_compressed(&mut self, key: S::Compressed, value: S::Compressed) {
        self.keys.push(key);
        self.values.push(value);
    }

    /// Compute scaled attention logits without decompressing keys.
    pub fn logits(&self, query: &[f32]) -> ScorerResult<Vec<f32>> {
        if query.is_empty() || self.keys.is_empty() {
            return Ok(Vec::new());
        }
        if query.len() != self.head_dim {
            return Err(ScorerError::DimensionMismatch {
                expected: self.head_dim,
                got: query.len(),
            });
        }
        ensure_finite(query)?;
        let prepared = self.scorer.prepare_query(query)?;
        let scale = sqrt_f32(self.head_dim as f32);
        let mut logits = Vec::with_capacity(self.keys.len());
        for key in &self.keys {
            let score = self.scorer.score_prepared(&prepared, key)?;
            if !score.is_finite() {
                return Err(ScorerError::ScoringFailed(
                    "attention logit was non-finite".into(),
                ));
            }
            logits.push(score / scale);
        }
        Ok(logits)
    }

    /// Run compressed-domain attention and decode only top-k values.
    pub fn attention_topk(&self, query: &[f32], top_k: usize) -> ScorerResult<AttentionOutput> {
        if self.keys.len() != self.values.len() {
            return Err(ScorerError::CorruptPayload(
                "attention cache has mismatched key/value lengths".into(),
            ));
        }
        if self.keys.is_empty() || top_k == 0 {
            return Ok(AttentionOutput {
                logits: Vec::new(),
                probabilities: Vec::new(),
                output: vec![0.0; self.head_dim],
                top_k_indices: Vec::new(),
                decompression_count: 0,
            });
        }
        let logits = self.logits(query)?;
        let probabilities = softmax(&logits)?;
        let k = top_k.min(probabilities.len());
        let top_k_indices = topk_indices_by_probability(&probabilities, k);
        let mut output = vec![0.0f64; self.head_dim];
        let mut decompression_count = 0usize;
        for &idx in &top_k_indices {
            let decoded = self.scorer.decode(&self.values[idx])?;
            if decoded.len() != self.head_dim {
                return Err(ScorerError::DimensionMismatch {
                    expected: self.head_dim,
                    got: decoded.len(),
                });
            }
            ensure_finite(&decoded)?;
            let prob = f64::from(probabilities[idx]);
            for (channel, acc) in decoded.iter().zip(output.iter_mut()) {
                *acc += f64::from(*channel) * prob;
            }
            decompression_count += 1;
        }
        Ok(AttentionOutput {
            logits,
            probabilities,
            output: output.into_iter().map(|v| v as f32).collect(),
            top_k_indices,
            decompression_count,
        })
    }
}

#[cfg(feature = "no_std")]
fn sqrt_f32(value: f32) -> f32 {
    libm::sqrtf(value)
}

#[cfg(not(feature = "no_std"))]
fn sqrt_f32(value: f32) -> f32 {
    value.sqrt()
}

#[cfg(feature = "no_std")]
fn exp_f32(value: f32) -> f32 {
    libm::expf(value)
}

#[cfg(not(feature = "no_std"))]
fn exp_f32(value: f32) -> f32 {
    value.exp()
}

fn ensure_finite(values: &[f32]) -> ScorerResult<()> {
    if values.iter().any(|v| !v.is_finite()) {
        return Err(ScorerError::ScoringFailed(
            "attention input contained non-finite value".into(),
        ));
    }
    Ok(())
}

fn softmax(logits: &[f32]) -> ScorerResult<Vec<f32>> {
    if logits.is_empty() {
        return Ok(Vec::new());
    }
    ensure_finite(logits)?;
    let max = logits
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, |acc, value| acc.max(value));
    let mut sum = 0.0f64;
    let mut exps = Vec::with_capacity(logits.len());
    for &logit in logits {
        let exp = f64::from(exp_f32(logit - max));
        sum += exp;
        exps.push(exp);
    }
    if !sum.is_finite() || sum <= 0.0 {
        return Err(ScorerError::ScoringFailed(
            "attention softmax underflow".into(),
        ));
    }
    Ok(exps.into_iter().map(|v| (v / sum) as f32).collect())
}

fn topk_indices_by_probability(probabilities: &[f32], k: usize) -> Vec<usize> {
    let mut indexed: Vec<(usize, f32)> = probabilities.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    indexed.into_iter().take(k).map(|(idx, _)| idx).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CompressedScorer, PreparedQuery};
    #[cfg(feature = "no_std")]
    use alloc::vec;

    #[derive(Clone)]
    struct ToyPrepared {
        dim: usize,
        query: Vec<f32>,
    }
    impl PreparedQuery for ToyPrepared {
        fn dim(&self) -> usize {
            self.dim
        }
    }

    struct ToyScorer {
        dim: usize,
    }
    impl CompressedScorer for ToyScorer {
        type Prepared = ToyPrepared;
        type Compressed = Vec<f32>;
        fn prepare_query(&self, query: &[f32]) -> ScorerResult<Self::Prepared> {
            Ok(ToyPrepared {
                dim: self.dim,
                query: query.to_vec(),
            })
        }
        fn score_prepared(
            &self,
            prepared: &Self::Prepared,
            compressed: &Self::Compressed,
        ) -> ScorerResult<f32> {
            Ok(prepared
                .query
                .iter()
                .zip(compressed)
                .map(|(a, b)| a * b)
                .sum())
        }
        fn decode(&self, compressed: &Self::Compressed) -> ScorerResult<Vec<f32>> {
            Ok(compressed.clone())
        }
        fn dim(&self) -> usize {
            self.dim
        }
        fn codec_name(&self) -> &'static str {
            "toy"
        }
        fn internal_bytes(&self) -> usize {
            0
        }
    }

    #[test]
    fn attention_topk_decodes_only_selected_values() {
        let mut cache = AttentionCache::new(ToyScorer { dim: 2 });
        cache.push_compressed(vec![1.0, 0.0], vec![10.0, 0.0]);
        cache.push_compressed(vec![0.0, 1.0], vec![0.0, 20.0]);
        cache.push_compressed(vec![0.5, 0.5], vec![5.0, 5.0]);
        let out = cache.attention_topk(&[1.0, 0.0], 1).unwrap();
        assert_eq!(out.decompression_count, 1);
        assert_eq!(out.top_k_indices, vec![0]);
        assert!(out.output[0] > 0.0);
        assert_eq!(out.output[1], 0.0);
    }

    #[test]
    fn attention_logits_match_cache_len() {
        let mut cache = AttentionCache::new(ToyScorer { dim: 2 });
        cache.push_compressed(vec![1.0, 0.0], vec![1.0, 0.0]);
        cache.push_compressed(vec![0.0, 1.0], vec![0.0, 1.0]);
        let logits = cache.logits(&[1.0, 0.0]).unwrap();
        assert_eq!(logits.len(), 2);
        assert!(logits[0] > logits[1]);
    }
}
