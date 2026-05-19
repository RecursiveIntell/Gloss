//! KV cache compression for transformer attention.
//!
//! In a decoder-only transformer, the KV cache stores one key/value pair per
//! token per layer. With long context windows this dominates GPU memory.
//!
//! [`KvCacheCompressor`] provides an experimental online shadow-mode interface:
//! call [`KvCacheCompressor::compress_token`] as each token is generated, then
//! [`KvCacheCompressor::attention_scores`] to compute attention logits from the
//! selected key policy.
//!
//! Exact shadows can be retained for fallback and comparison. This module does
//! not validate deployed attention quality.
//!
//! # Usage
//!
//! ```rust
//! use turbo_quant::kv::{KvCacheCompressor, KvCacheConfig, KvQuantPolicy};
//!
//! let config = KvCacheConfig {
//!     head_dim: 64,        // per-attention-head dimension
//!     key_policy: KvQuantPolicy::quantized(8, 16),
//!     value_policy: KvQuantPolicy::Exact,
//!     seed: 42,
//!     keep_exact_shadow: true,
//! };
//!
//! let mut cache = KvCacheCompressor::new(config).unwrap();
//!
//! // At each generation step, compress the new key and value.
//! let key = vec![0.1f32; 64];
//! let value = vec![0.2f32; 64];
//! cache.compress_token(&key, &value).unwrap();
//!
//! // At attention time: compute logits against all compressed keys.
//! let query = vec![0.15f32; 64];
//! let scores = cache.attention_scores(&query).unwrap();
//! assert_eq!(scores.len(), 1); // one score per stored token
//!
//! // Retrieve approximate reconstructed values for the top-k tokens.
//! let values = cache.decode_values(&[0]).unwrap();
//! assert_eq!(values.len(), 1);
//! ```

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
    error::{Result, TurboQuantError},
    rotation::RotationKind,
    turbo::{TurboCode, TurboMode, TurboQuantizer},
};

/// Quantization policy for one KV stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum KvQuantPolicy {
    /// Keep exact f32 vectors and do not create a compressed sidecar.
    Exact,
    /// Create TurboQuant sidecars. Callers should benchmark before promoting.
    Quantized {
        bits: u8,
        projections: usize,
        mode: TurboMode,
        rotation_kind: RotationKind,
    },
}

impl KvQuantPolicy {
    pub fn quantized(bits: u8, projections: usize) -> Self {
        Self::Quantized {
            bits,
            projections,
            mode: TurboMode::PolarWithQjl,
            rotation_kind: RotationKind::Auto,
        }
    }

    pub fn quantized_with_stored_rotation(bits: u8, projections: usize) -> Self {
        Self::Quantized {
            bits,
            projections,
            mode: TurboMode::PolarWithQjl,
            rotation_kind: RotationKind::StoredQr,
        }
    }
}

/// Configuration for a single attention head's KV cache compressor.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KvCacheConfig {
    /// Dimension of each attention head's key/value vectors.
    pub head_dim: usize,
    /// Key compression policy.
    pub key_policy: KvQuantPolicy,
    /// Value compression policy. Values may need a different policy from keys.
    pub value_policy: KvQuantPolicy,
    /// Deterministic seed for all random matrices.
    pub seed: u64,
    /// Keep exact keys/values so callers can run fallback and shadow comparison.
    pub keep_exact_shadow: bool,
}

/// A compressed KV cache entry for one token.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CompressedToken {
    pub compressed_key: Option<TurboCode>,
    pub compressed_value: Option<TurboCode>,
    pub exact_key: Option<Vec<f32>>,
    pub exact_value: Option<Vec<f32>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct KvShadowScore {
    pub exact: f32,
    pub compressed: f32,
    pub abs_error: f32,
}

/// Online KV cache compressor for one attention head.
///
/// Tokens are appended in generation order. Attention scores are computed
/// across all stored compressed keys without decompression.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KvCacheCompressor {
    config: KvCacheConfig,
    key_quantizer: Option<TurboQuantizer>,
    value_quantizer: Option<TurboQuantizer>,
    tokens: Vec<CompressedToken>,
}

impl KvCacheCompressor {
    /// Create a new compressor for one attention head.
    pub fn new(config: KvCacheConfig) -> Result<Self> {
        let key_quantizer = quantizer_for_policy(config.head_dim, &config.key_policy, config.seed)?;
        // Value quantizer uses an independent seed offset so key and value
        // rotation matrices are uncorrelated.
        let value_quantizer = quantizer_for_policy(
            config.head_dim,
            &config.value_policy,
            config.seed.wrapping_add(0x1234_5678_ABCD_EF00),
        )?;
        Ok(Self {
            config,
            key_quantizer,
            value_quantizer,
            tokens: Vec::new(),
        })
    }

    /// The number of tokens currently stored in this cache.
    pub fn len(&self) -> usize {
        self.tokens.len()
    }

    /// True if the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.tokens.is_empty()
    }

    /// Compress and store one (key, value) token pair.
    ///
    /// Called once per generation step. O(d) time.
    pub fn compress_token(&mut self, key: &[f32], value: &[f32]) -> Result<()> {
        if key.len() != self.config.head_dim {
            return Err(TurboQuantError::DimensionMismatch {
                expected: self.config.head_dim,
                got: key.len(),
            });
        }
        if value.len() != self.config.head_dim {
            return Err(TurboQuantError::DimensionMismatch {
                expected: self.config.head_dim,
                got: value.len(),
            });
        }

        let compressed_key = self
            .key_quantizer
            .as_ref()
            .map(|quantizer| quantizer.encode(key))
            .transpose()?;
        let compressed_value = self
            .value_quantizer
            .as_ref()
            .map(|quantizer| quantizer.encode(value))
            .transpose()?;
        self.tokens.push(CompressedToken {
            compressed_key,
            compressed_value,
            exact_key: self.config.keep_exact_shadow.then(|| key.to_vec()),
            exact_value: self.config.keep_exact_shadow.then(|| value.to_vec()),
        });
        Ok(())
    }

    /// Compute attention logits q·kᵢ for all stored tokens i.
    ///
    /// Returns a vector of length `self.len()`. Does not decompress keys.
    /// Suitable for softmax → attention weight computation.
    ///
    /// O(n·d) time where n is the number of stored tokens.
    pub fn attention_scores(&self, query: &[f32]) -> Result<Vec<f32>> {
        if query.len() != self.config.head_dim {
            return Err(TurboQuantError::DimensionMismatch {
                expected: self.config.head_dim,
                got: query.len(),
            });
        }

        match &self.key_quantizer {
            Some(quantizer) => self
                .tokens
                .iter()
                .map(|t| {
                    let code = t.compressed_key.as_ref().ok_or_else(|| {
                        TurboQuantError::MalformedCode {
                            reason: "token is missing compressed key".into(),
                        }
                    })?;
                    quantizer.inner_product_estimate(code, query)
                })
                .collect(),
            None => self.exact_attention_scores(query),
        }
    }

    /// Exact f32 attention logits from the retained shadow vectors.
    pub fn exact_attention_scores(&self, query: &[f32]) -> Result<Vec<f32>> {
        if query.len() != self.config.head_dim {
            return Err(TurboQuantError::DimensionMismatch {
                expected: self.config.head_dim,
                got: query.len(),
            });
        }
        self.tokens
            .iter()
            .map(|token| {
                let key =
                    token
                        .exact_key
                        .as_ref()
                        .ok_or_else(|| TurboQuantError::MalformedCode {
                            reason: "exact key fallback unavailable; enable keep_exact_shadow"
                                .into(),
                        })?;
                Ok(key.iter().zip(query.iter()).map(|(k, q)| k * q).sum())
            })
            .collect()
    }

    /// Compare exact and compressed key scores when exact shadows are retained.
    pub fn shadow_attention_scores(&self, query: &[f32]) -> Result<Vec<KvShadowScore>> {
        let exact = self.exact_attention_scores(query)?;
        let compressed = match &self.key_quantizer {
            Some(_) => self.attention_scores(query)?,
            None => exact.clone(),
        };
        Ok(exact
            .into_iter()
            .zip(compressed)
            .map(|(exact, compressed)| KvShadowScore {
                exact,
                compressed,
                abs_error: (exact - compressed).abs(),
            })
            .collect())
    }

    /// Softmax-weighted sum of approximate value reconstructions.
    ///
    /// This is the full compressed attention output for one head:
    /// `Σᵢ softmax(scores)ᵢ · decode(vᵢ)`
    ///
    /// Note: values are decoded (approximate reconstruction) rather than
    /// stored in compressed form, since weighted sums cannot be computed
    /// in compressed space. This is consistent with how KV cache compression
    /// is used in practice (compress keys for score computation; decode values
    /// for the weighted sum).
    pub fn attend(&self, query: &[f32]) -> Result<Vec<f32>> {
        if self.tokens.is_empty() {
            return Ok(vec![0.0f32; self.config.head_dim]);
        }

        let scores = self.attention_scores(query)?;

        // Numerically stable softmax.
        let max_score = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let exps: Vec<f32> = scores.iter().map(|s| (s - max_score).exp()).collect();
        let sum_exp: f32 = exps.iter().sum();
        let weights: Vec<f32> = exps.iter().map(|e| e / sum_exp).collect();

        // Weighted sum of decoded values.
        let mut output = vec![0.0f32; self.config.head_dim];
        for (token, &weight) in self.tokens.iter().zip(weights.iter()) {
            let decoded = match (
                &self.value_quantizer,
                &token.compressed_value,
                &token.exact_value,
            ) {
                (Some(quantizer), Some(code), _) => quantizer.decode_approximate(code)?,
                (None, _, Some(exact)) => exact.clone(),
                _ => {
                    return Err(TurboQuantError::MalformedCode {
                        reason: "value fallback unavailable".into(),
                    });
                }
            };
            for (out, val) in output.iter_mut().zip(decoded.iter()) {
                *out += weight * val;
            }
        }

        Ok(output)
    }

    /// Decode the value vector for specific token indices (for top-k attention).
    pub fn decode_values(&self, indices: &[usize]) -> Result<Vec<Vec<f32>>> {
        indices
            .iter()
            .map(|&i| {
                if i >= self.tokens.len() {
                    return Err(TurboQuantError::DimensionMismatch {
                        expected: self.tokens.len(),
                        got: i + 1,
                    });
                }
                match (
                    &self.value_quantizer,
                    &self.tokens[i].compressed_value,
                    &self.tokens[i].exact_value,
                ) {
                    (Some(quantizer), Some(code), _) => quantizer.decode_approximate(code),
                    (None, _, Some(exact)) => Ok(exact.clone()),
                    _ => Err(TurboQuantError::MalformedCode {
                        reason: "value fallback unavailable".into(),
                    }),
                }
            })
            .collect()
    }

    /// Approximate total bytes used by all compressed tokens.
    pub fn compressed_bytes(&self) -> usize {
        self.tokens
            .iter()
            .map(|t| {
                t.compressed_key
                    .as_ref()
                    .map_or(0, TurboCode::encoded_bytes)
                    + t.compressed_value
                        .as_ref()
                        .map_or(0, TurboCode::encoded_bytes)
            })
            .sum()
    }

    /// Bytes that would be used if keys/values were stored uncompressed as f32.
    pub fn uncompressed_bytes(&self) -> usize {
        self.tokens.len() * 2 * self.config.head_dim * std::mem::size_of::<f32>()
    }

    /// Compression ratio: uncompressed / compressed.
    pub fn compression_ratio(&self) -> f32 {
        let compressed = self.compressed_bytes();
        if compressed == 0 {
            return 0.0;
        }
        self.uncompressed_bytes() as f32 / compressed as f32
    }
}

fn quantizer_for_policy(
    head_dim: usize,
    policy: &KvQuantPolicy,
    seed: u64,
) -> Result<Option<TurboQuantizer>> {
    match policy {
        KvQuantPolicy::Exact => Ok(None),
        KvQuantPolicy::Quantized {
            bits,
            projections,
            mode,
            rotation_kind,
        } => TurboQuantizer::new_with_mode_and_rotation(
            head_dim,
            *bits,
            *projections,
            seed,
            *mode,
            *rotation_kind,
        )
        .map(Some),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn random_vec(dim: usize, seed: u64) -> Vec<f32> {
        use rand::SeedableRng;
        use rand_chacha::ChaCha8Rng;
        use rand_distr::{Distribution, StandardNormal};
        let mut rng = ChaCha8Rng::seed_from_u64(seed);
        (0..dim).map(|_| StandardNormal.sample(&mut rng)).collect()
    }

    fn make_cache(dim: usize) -> KvCacheCompressor {
        KvCacheCompressor::new(KvCacheConfig {
            head_dim: dim,
            key_policy: KvQuantPolicy::quantized(8, dim / 4),
            value_policy: KvQuantPolicy::quantized(8, dim / 4),
            seed: 42,
            keep_exact_shadow: true,
        })
        .unwrap()
    }

    #[test]
    fn empty_cache_returns_empty_scores() {
        let cache = make_cache(16);
        let scores = cache.attention_scores(&random_vec(16, 1)).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn token_count_increments_correctly() {
        let mut cache = make_cache(16);
        for i in 0..5 {
            cache
                .compress_token(&random_vec(16, i), &random_vec(16, i + 100))
                .unwrap();
        }
        assert_eq!(cache.len(), 5);
    }

    #[test]
    fn attention_scores_length_matches_token_count() {
        let mut cache = make_cache(16);
        for i in 0..10 {
            cache
                .compress_token(&random_vec(16, i), &random_vec(16, i + 100))
                .unwrap();
        }
        let scores = cache.attention_scores(&random_vec(16, 999)).unwrap();
        assert_eq!(scores.len(), 10);
    }

    #[test]
    fn highest_score_is_for_query_similar_key() {
        let dim = 16;
        let mut cache = make_cache(dim);

        // Add 9 random keys.
        for i in 0..9u64 {
            cache
                .compress_token(&random_vec(dim, i * 10), &random_vec(dim, i * 10 + 1))
                .unwrap();
        }

        // Add one key that is very close to the query.
        let query = random_vec(dim, 999);
        let similar_key: Vec<f32> = query.iter().map(|x| x + 0.001).collect();
        cache
            .compress_token(&similar_key, &random_vec(dim, 9000))
            .unwrap();

        let scores = cache.attention_scores(&query).unwrap();
        let best_idx = scores
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap();

        assert_eq!(
            best_idx, 9,
            "similar key should have highest attention score"
        );
    }

    #[test]
    fn attend_output_has_correct_dimension() {
        let dim = 16;
        let mut cache = make_cache(dim);
        for i in 0..5u64 {
            cache
                .compress_token(&random_vec(dim, i), &random_vec(dim, i + 50))
                .unwrap();
        }
        let output = cache.attend(&random_vec(dim, 1)).unwrap();
        assert_eq!(output.len(), dim);
    }

    #[test]
    fn attend_weights_sum_to_one_implicitly() {
        // If all values are the same vector v, attend() should return ≈ v.
        let dim = 16;
        let mut cache = make_cache(dim);
        let v = random_vec(dim, 77);

        for i in 0..8u64 {
            cache.compress_token(&random_vec(dim, i), &v).unwrap();
        }

        let output = cache.attend(&random_vec(dim, 1)).unwrap();

        // Output should be close to v since all values are the same.
        let error: f32 = output
            .iter()
            .zip(v.iter())
            .map(|(o, vi)| (o - vi).powi(2))
            .sum::<f32>()
            .sqrt();
        let v_norm: f32 = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(
            error / v_norm < 0.15,
            "attend output too far from value: relative_error={:.4}",
            error / v_norm
        );
    }

    #[test]
    fn compression_ratio_is_above_one() {
        let mut cache = make_cache(64);
        for i in 0..20u64 {
            cache
                .compress_token(&random_vec(64, i), &random_vec(64, i + 1000))
                .unwrap();
        }
        let ratio = cache.compression_ratio();
        assert!(
            ratio > 1.0,
            "compression ratio should be > 1, got {ratio:.2}"
        );
        println!("compression ratio at 8 bits, d=64: {ratio:.2}x");
    }

    #[test]
    fn wrong_key_dimension_is_rejected() {
        let mut cache = make_cache(16);
        let result = cache.compress_token(&random_vec(8, 0), &random_vec(16, 1));
        assert!(result.is_err());
    }

    #[test]
    fn wrong_query_dimension_is_rejected() {
        let mut cache = make_cache(16);
        cache
            .compress_token(&random_vec(16, 0), &random_vec(16, 1))
            .unwrap();
        let result = cache.attention_scores(&random_vec(8, 0));
        assert!(result.is_err());
    }

    #[test]
    fn key_and_value_quantizers_are_independent() {
        // Encode the same vector as both key and value.
        // The codes should differ because the quantizers use different seeds.
        let mut cache = make_cache(16);
        let v = random_vec(16, 42);
        cache.compress_token(&v, &v).unwrap();

        let token = &cache.tokens[0];
        // Different rotation seeds → different angle indices with overwhelming probability.
        assert_ne!(
            token
                .compressed_key
                .as_ref()
                .unwrap()
                .polar_code
                .packed_angle_indices,
            token
                .compressed_value
                .as_ref()
                .unwrap()
                .polar_code
                .packed_angle_indices,
            "key and value quantizers should use independent rotations"
        );
    }

    #[test]
    fn exact_shadow_scores_are_available() {
        let mut cache = make_cache(16);
        cache
            .compress_token(&random_vec(16, 1), &random_vec(16, 2))
            .unwrap();
        let shadow = cache.shadow_attention_scores(&random_vec(16, 3)).unwrap();
        assert_eq!(shadow.len(), 1);
        assert!(shadow[0].abs_error.is_finite());
    }

    #[test]
    fn asymmetric_exact_value_policy_decodes_exact_values() {
        let dim = 16;
        let mut cache = KvCacheCompressor::new(KvCacheConfig {
            head_dim: dim,
            key_policy: KvQuantPolicy::quantized(8, dim / 4),
            value_policy: KvQuantPolicy::Exact,
            seed: 42,
            keep_exact_shadow: true,
        })
        .unwrap();
        let value = random_vec(dim, 99);
        cache.compress_token(&random_vec(dim, 1), &value).unwrap();
        assert_eq!(cache.decode_values(&[0]).unwrap()[0], value);
    }
}
