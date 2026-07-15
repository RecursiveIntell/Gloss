//! Phase 8: Simplified compression governor.
//!
//! Per-vector importance scoring that determines the quantization level for
//! each embedding. This is a standalone, dependency-light module: it does NOT
//! depend on `turbo-quant` or `quant-governor`. The importance score combines
//! access frequency, information entropy, and structuring score, then maps to
//! a [`QuantizationLevel`](crate::compression_governor::QuantizationLevel) via fixed thresholds.
//!
//! The module is gated behind the `compression-governor` feature flag.

use serde::{Deserialize, Serialize};

// ─── Config ───────────────────────────────────────────────────────────────

/// Configuration for importance scoring.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportanceConfig {
    /// Weight for access frequency (default 0.40).
    pub access_weight: f64,
    /// Weight for information entropy (default 0.30).
    pub entropy_weight: f64,
    /// Weight for structuring score (default 0.30).
    pub structuring_weight: f64,
    /// Half-life (days) for access frequency decay (default 30.0).
    pub access_half_life_days: f64,
}

impl Default for ImportanceConfig {
    fn default() -> Self {
        Self {
            access_weight: 0.40,
            entropy_weight: 0.30,
            structuring_weight: 0.30,
            access_half_life_days: 30.0,
        }
    }
}

// ─── Quantization level ─────────────────────────────────────────────────

/// The quantization level chosen for an embedding vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum QuantizationLevel {
    /// Full-precision 32-bit float (highest fidelity).
    F32,
    /// Scalar-quantized 8-bit.
    SQ8,
    /// Scalar-quantized 4-bit.
    SQ4,
    /// Scalar-quantized 4-bit with a marked/dead flag (lowest importance).
    SQ4Marked,
}

// ─── Importance score ───────────────────────────────────────────────────

/// The computed importance score and chosen quantization level for one
/// embedding.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ImportanceScore {
    /// The embedding id.
    pub embedding_id: String,
    /// The importance score in [0, 1] (caller may scale inputs).
    pub score: f64,
    /// The quantization level chosen for this embedding.
    pub level: QuantizationLevel,
    /// ISO-8601 timestamp of when the score was computed.
    pub last_computed: String,
}

// ─── Requantization receipt ────────────────────────────────────────────

/// A receipt recording that an embedding was re-quantized to a new level.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RequantizationReceipt {
    /// The embedding that was re-quantized.
    pub embedding_id: String,
    /// The previous quantization level.
    pub old_level: QuantizationLevel,
    /// The new quantization level.
    pub new_level: QuantizationLevel,
    /// ISO-8601 timestamp of the re-quantization.
    pub timestamp: String,
}

// ─── Importance computation ─────────────────────────────────────────────

/// Compute the importance score for a single embedding.
///
/// `score = access_weight * access_frequency + entropy_weight * information_entropy + structuring_weight * structuring_score`
///
/// Callers are expected to normalize each input to [0, 1] before calling.
pub fn compute_importance(
    access_frequency: f64,
    information_entropy: f64,
    structuring_score: f64,
    config: &ImportanceConfig,
) -> f64 {
    config.access_weight * access_frequency
        + config.entropy_weight * information_entropy
        + config.structuring_weight * structuring_score
}

// ─── Quantization decision ──────────────────────────────────────────────

/// Decide the quantization level for a given importance score.
///
/// - `> 0.8` → `F32`
/// - `> 0.5` → `SQ8`
/// - `> 0.2` → `SQ4`
/// - `<= 0.2` → `SQ4Marked`
pub fn decide_quantization(importance: f64) -> QuantizationLevel {
    if importance > 0.8 {
        QuantizationLevel::F32
    } else if importance > 0.5 {
        QuantizationLevel::SQ8
    } else if importance > 0.2 {
        QuantizationLevel::SQ4
    } else {
        QuantizationLevel::SQ4Marked
    }
}

// ─── TurboQuant integration ─────────────────────────────────────────────

/// Map a QuantizationLevel to TurboQuant codec parameters.
///
/// F32 → no compression (raw f32, no TurboQuant needed).
/// SQ8 → TurboQuant with 8 bits.
/// SQ4 / SQ4Marked → TurboQuant with 4 bits.
#[cfg(feature = "turbo-quant-codec")]
pub fn level_to_turbo_quant_bits(level: QuantizationLevel) -> Option<u8> {
    match level {
        QuantizationLevel::F32 => None, // no compression needed
        QuantizationLevel::SQ8 => Some(8),
        QuantizationLevel::SQ4 => Some(4),
        QuantizationLevel::SQ4Marked => Some(4),
    }
}

/// Compress an embedding using TurboQuant based on its importance score.
///
/// Returns `None` when the importance level is F32 (no compression needed).
/// Returns `Some(artifact)` when TurboQuant compression is applied.
#[cfg(feature = "turbo-quant-codec")]
pub fn compress_with_turbo_quant(
    embedding: &[f32],
    importance: f64,
    projections: usize,
    seed: u64,
) -> Result<Option<crate::vector_codec::VectorArtifactV1>, crate::MemoryError> {
    let level = decide_quantization(importance);
    let bits = match level_to_turbo_quant_bits(level) {
        Some(b) => b,
        None => return Ok(None), // F32 — no compression
    };

    let codec =
        crate::vector_codec::TurboQuantCodec::new(embedding.len(), bits, projections, seed)?;
    let artifact = crate::vector_codec::VectorCodec::encode(&codec, embedding)?;
    Ok(Some(artifact))
}

/// Batch compress embeddings using TurboQuant based on importance scores.
///
/// Returns a vector of (embedding_id, Option<artifact>) pairs.
/// Embeddings with F32 importance are returned as None.
#[cfg(feature = "turbo-quant-codec")]
pub fn batch_compress_with_turbo_quant(
    embeddings: &[(String, Vec<f32>, f64)], // (id, vector, importance)
    projections: usize,
    seed: u64,
) -> Vec<(String, Option<crate::vector_codec::VectorArtifactV1>)> {
    embeddings
        .iter()
        .filter_map(|(id, vec, importance)| {
            match compress_with_turbo_quant(vec, *importance, projections, seed) {
                Ok(artifact) => Some((id.clone(), artifact)),
                Err(e) => {
                    tracing::warn!("TurboQuant compression failed for {}: {}", id, e);
                    None
                }
            }
        })
        .collect()
}

// ─── Re-quantization ────────────────────────────────────────────────────

/// Re-quantize a set of embeddings based on their current importance scores.
///
/// For each [`ImportanceScore`], compute the new target level from the score
/// and emit a [`RequantizationReceipt`] only when the new level differs from
/// the current `level` recorded in the score.
pub fn re_quantize(
    scores: &[ImportanceScore],
    _config: &ImportanceConfig,
) -> Vec<RequantizationReceipt> {
    let timestamp = "1970-01-01T00:00:00Z".to_string(); // deterministic for tests
    scores
        .iter()
        .filter_map(|s| {
            let new_level = decide_quantization(s.score);
            if new_level != s.level {
                Some(RequantizationReceipt {
                    embedding_id: s.embedding_id.clone(),
                    old_level: s.level,
                    new_level,
                    timestamp: timestamp.clone(),
                })
            } else {
                None
            }
        })
        .collect()
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn importance_computation_uses_weights() {
        let cfg = ImportanceConfig::default();
        // access_weight=0.40, entropy_weight=0.30, structuring_weight=0.30.
        let score = compute_importance(1.0, 1.0, 1.0, &cfg);
        assert!((score - 1.0).abs() < f64::EPSILON);

        let score = compute_importance(0.5, 0.0, 0.0, &cfg);
        assert!((score - 0.20).abs() < f64::EPSILON);

        let score = compute_importance(0.0, 0.5, 0.0, &cfg);
        assert!((score - 0.15).abs() < f64::EPSILON);

        let score = compute_importance(0.0, 0.0, 0.5, &cfg);
        assert!((score - 0.15).abs() < f64::EPSILON);
    }

    #[test]
    fn quantization_decision_boundaries() {
        assert_eq!(decide_quantization(0.81), QuantizationLevel::F32);
        assert_eq!(decide_quantization(0.8), QuantizationLevel::SQ8); // 0.8 not > 0.8
        assert_eq!(decide_quantization(0.51), QuantizationLevel::SQ8);
        assert_eq!(decide_quantization(0.5), QuantizationLevel::SQ4); // 0.5 not > 0.5
        assert_eq!(decide_quantization(0.21), QuantizationLevel::SQ4);
        assert_eq!(decide_quantization(0.2), QuantizationLevel::SQ4Marked); // 0.2 not > 0.2
        assert_eq!(decide_quantization(0.1), QuantizationLevel::SQ4Marked);
        assert_eq!(decide_quantization(0.0), QuantizationLevel::SQ4Marked);
    }

    #[test]
    fn re_quantize_only_on_change() {
        let cfg = ImportanceConfig::default();
        let scores = vec![
            ImportanceScore {
                embedding_id: "e1".to_string(),
                score: 0.9,                    // should be F32
                level: QuantizationLevel::SQ8, // wrong currently -> receipt
                last_computed: "t".to_string(),
            },
            ImportanceScore {
                embedding_id: "e2".to_string(),
                score: 0.4,                    // should be SQ4
                level: QuantizationLevel::SQ4, // already correct -> no receipt
                last_computed: "t".to_string(),
            },
            ImportanceScore {
                embedding_id: "e3".to_string(),
                score: 0.1,                    // should be SQ4Marked
                level: QuantizationLevel::F32, // wrong -> receipt
                last_computed: "t".to_string(),
            },
        ];
        let receipts = re_quantize(&scores, &cfg);
        assert_eq!(receipts.len(), 2);
        assert_eq!(receipts[0].embedding_id, "e1");
        assert_eq!(receipts[0].old_level, QuantizationLevel::SQ8);
        assert_eq!(receipts[0].new_level, QuantizationLevel::F32);
        assert_eq!(receipts[1].embedding_id, "e3");
        assert_eq!(receipts[1].old_level, QuantizationLevel::F32);
        assert_eq!(receipts[1].new_level, QuantizationLevel::SQ4Marked);
    }

    #[test]
    fn re_quantize_empty_inputs() {
        let cfg = ImportanceConfig::default();
        let receipts = re_quantize(&[], &cfg);
        assert!(receipts.is_empty());
    }

    #[test]
    fn config_defaults() {
        let cfg = ImportanceConfig::default();
        assert!((cfg.access_weight - 0.40).abs() < f64::EPSILON);
        assert!((cfg.entropy_weight - 0.30).abs() < f64::EPSILON);
        assert!((cfg.structuring_weight - 0.30).abs() < f64::EPSILON);
        assert!((cfg.access_half_life_days - 30.0).abs() < f64::EPSILON);
    }
}
