//! Scalar quantization (SQ8) accuracy tests.

#[allow(unused_imports)]
use semantic_memory::quantize::{pack_quantized, unpack_quantized, QuantizedVector, Quantizer};
use semantic_memory::MemoryError;

/// Compute cosine similarity between two f32 vectors.
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    dot / (norm_a * norm_b)
}

/// Generate a deterministic pseudo-random normalized vector.
fn random_normalized_vector(dims: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    if state == 0 {
        state = 1;
    }
    let mut values = Vec::with_capacity(dims);
    for _ in 0..dims {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let val = ((state as f64) / (u64::MAX as f64)) * 2.0 - 1.0;
        values.push(val as f32);
    }
    let magnitude: f32 = values.iter().map(|v| v * v).sum::<f32>().sqrt();
    if magnitude > 0.0 {
        for v in &mut values {
            *v /= magnitude;
        }
    }
    values
}

/// Perturb a vector slightly.
fn perturb(v: &[f32], amount: f32, seed: u64) -> Vec<f32> {
    let noise = random_normalized_vector(v.len(), seed);
    let mut result: Vec<f32> = v
        .iter()
        .zip(noise.iter())
        .map(|(&a, &n)| a + n * amount)
        .collect();
    // Re-normalize
    let mag: f32 = result.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag > 0.0 {
        for x in &mut result {
            *x /= mag;
        }
    }
    result
}

#[test]
fn test_sq8_round_trip_accuracy() {
    let q = Quantizer::new(768);
    let original: Vec<f32> = (0..768).map(|i| (i as f32 / 768.0) * 2.0 - 1.0).collect();
    let quantized = q.quantize(&original).unwrap();
    let reconstructed = q.dequantize(&quantized);

    let max_error = original
        .iter()
        .zip(reconstructed.iter())
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);

    assert!(max_error < 0.01, "Max quantization error: {}", max_error);
}

#[test]
fn test_sq8_cosine_similarity_preservation() {
    let q = Quantizer::new(768);

    let v1 = random_normalized_vector(768, 42);
    let v2 = perturb(&v1, 0.05, 123); // small perturbation -> should be similar
    let v3 = random_normalized_vector(768, 999); // unrelated -> should be dissimilar

    let exact_sim_12 = cosine_similarity(&v1, &v2);
    let _exact_sim_13 = cosine_similarity(&v1, &v3);

    // Quantize and compute approximate similarity
    let q1 = q.quantize(&v1).unwrap();
    let q2 = q.quantize(&v2).unwrap();
    let q3 = q.quantize(&v3).unwrap();
    let approx_sim_12 = cosine_similarity(&q.dequantize(&q1), &q.dequantize(&q2));
    let approx_sim_13 = cosine_similarity(&q.dequantize(&q1), &q.dequantize(&q3));

    // Ranking should be preserved: similar vectors stay more similar than dissimilar ones
    assert!(
        approx_sim_12 > approx_sim_13,
        "Ranking not preserved: approx_sim_12={}, approx_sim_13={}",
        approx_sim_12,
        approx_sim_13
    );

    // Absolute error should be small for normalized vectors
    let error_12 = (exact_sim_12 - approx_sim_12).abs();
    assert!(
        error_12 < 0.02,
        "Cosine similarity error too large: exact={}, approx={}, error={}",
        exact_sim_12,
        approx_sim_12,
        error_12
    );
}

#[test]
fn test_quantize_constant_vector() {
    // All dimensions same value -- edge case
    let q = Quantizer::new(4);
    let v = vec![0.5, 0.5, 0.5, 0.5];
    let result = q.quantize(&v).unwrap();
    // Should not panic or produce NaN
    assert_eq!(result.data.len(), 4);
    assert!(result.scale.is_finite());
}

#[test]
fn test_quantize_extreme_values() {
    let q = Quantizer::new(4);
    let v = vec![-1000.0, 0.0, 0.001, 1000.0];
    let result = q.quantize(&v).unwrap();
    assert_eq!(result.data.len(), 4);
    assert!(result.scale.is_finite());
    assert!(!result.scale.is_nan());
}

#[test]
fn test_quantize_dimension_mismatch() {
    let q = Quantizer::new(768);
    let v = vec![1.0, 2.0, 3.0]; // wrong dimensions
    let result = q.quantize(&v);
    assert!(result.is_err());
}

#[test]
fn test_dequantize_round_trip_preserves_length() {
    let q = Quantizer::new(768);
    let original = random_normalized_vector(768, 77);
    let quantized = q.quantize(&original).unwrap();
    let reconstructed = q.dequantize(&quantized);
    assert_eq!(reconstructed.len(), 768);
}

#[test]
fn test_quantize_zero_vector() {
    let q = Quantizer::new(4);
    let v = vec![0.0, 0.0, 0.0, 0.0];
    let result = q.quantize(&v).unwrap();
    assert_eq!(result.data.len(), 4);
    // Constant vector -> special case
    assert_eq!(result.scale, 1.0);
    assert_eq!(result.zero_point, 0);
}

// ---------------------------------------------------------------------------
// v0.3.0 new tests
// ---------------------------------------------------------------------------

#[test]
#[allow(unused_comparisons)]
fn round_trip_symmetric_range() {
    let q = Quantizer::new(768);
    let original = random_normalized_vector(768, 0xDEAD_BEEF);

    let quantized = q.quantize(&original).unwrap();

    // All quantized values must be in the full i8 range.
    for (i, &val) in quantized.data.iter().enumerate() {
        assert!(
            (-128..=127).contains(&val),
            "quantized value at dim {} is {} which is outside [-128, 127]",
            i,
            val
        );
    }

    // Zero point must also be in the full i8 range.
    assert!(
        (-128..=127).contains(&quantized.zero_point),
        "zero_point {} is outside [-128, 127]",
        quantized.zero_point
    );

    // Dequantize and check cosine similarity with original.
    let reconstructed = q.dequantize(&quantized);
    let sim = cosine_similarity(&original, &reconstructed);
    assert!(
        sim > 0.995,
        "cosine similarity between original and reconstructed is {} (expected > 0.995)",
        sim
    );
}

#[test]
fn round_trip_max_error() {
    let q = Quantizer::new(768);
    let original = random_normalized_vector(768, 0xCAFE_BABE);

    let quantized = q.quantize(&original).unwrap();
    let reconstructed = q.dequantize(&quantized);
    let scale = quantized.scale;

    for i in 0..768 {
        let err = (original[i] - reconstructed[i]).abs();
        assert!(
            err < scale,
            "dimension {}: |original ({}) - reconstructed ({})| = {} >= scale ({})",
            i,
            original[i],
            reconstructed[i],
            err,
            scale
        );
    }
}

#[test]
fn constant_vector_handling() {
    let q = Quantizer::new(768);
    let v = vec![0.5f32; 768];
    let result = q.quantize(&v).unwrap();

    // Constant vector: all data should be zero, scale = 1.0, zero_point = 0.
    assert_eq!(
        result.data,
        vec![0i8; 768],
        "data should be all zeros for a constant vector"
    );
    assert_eq!(
        result.scale, 1.0,
        "scale should be 1.0 for a constant vector"
    );
    assert_eq!(
        result.zero_point, 0,
        "zero_point should be 0 for a constant vector"
    );
}

#[test]
fn pack_unpack_round_trip() {
    let q = Quantizer::new(768);

    // Test with several different vectors.
    let seeds: &[u64] = &[1, 42, 100, 0xBEEF, 0xF00D_CAFE, 9999999];

    for &seed in seeds {
        let original = random_normalized_vector(768, seed);
        let quantized = q.quantize(&original).unwrap();

        let packed = pack_quantized(&quantized);
        let unpacked = unpack_quantized(&packed, 768).unwrap();

        assert_eq!(
            quantized.data, unpacked.data,
            "data mismatch for seed {}",
            seed
        );
        assert_eq!(
            quantized.scale, unpacked.scale,
            "scale mismatch for seed {}: {} vs {}",
            seed, quantized.scale, unpacked.scale
        );
        assert_eq!(
            quantized.zero_point, unpacked.zero_point,
            "zero_point mismatch for seed {}: {} vs {}",
            seed, quantized.zero_point, unpacked.zero_point
        );
    }
}

#[test]
fn pack_unpack_wrong_dimensions() {
    let q = Quantizer::new(768);
    let v = random_normalized_vector(768, 12345);
    let quantized = q.quantize(&v).unwrap();
    let packed = pack_quantized(&quantized);

    // Attempt to unpack with wrong dimensions (384 instead of 768).
    let result = unpack_quantized(&packed, 384);
    assert!(result.is_err(), "unpack with wrong dimensions should fail");

    match result {
        Err(MemoryError::QuantizationError(msg)) => {
            assert!(
                msg.contains("384"),
                "error message should mention the expected dimensions: {}",
                msg
            );
        }
        Err(other) => {
            panic!("expected MemoryError::QuantizationError, got: {:?}", other);
        }
        Ok(_) => {
            panic!("expected error, got Ok");
        }
    }
}

#[test]
fn cosine_similarity_preserved() {
    let q = Quantizer::new(768);
    let n = 100;

    // Generate 100 random normalized vectors.
    let vectors: Vec<Vec<f32>> = (0..n)
        .map(|i| random_normalized_vector(768, (i as u64 + 1) * 7919))
        .collect();

    // Quantize all, then dequantize.
    let reconstructed: Vec<Vec<f32>> = vectors
        .iter()
        .map(|v| {
            let qv = q.quantize(v).unwrap();
            q.dequantize(&qv)
        })
        .collect();

    // Compute all pairwise cosine similarities (upper triangle only).
    let mut exact_sims = Vec::new();
    let mut approx_sims = Vec::new();

    for i in 0..n {
        for j in (i + 1)..n {
            exact_sims.push(cosine_similarity(&vectors[i], &vectors[j]));
            approx_sims.push(cosine_similarity(&reconstructed[i], &reconstructed[j]));
        }
    }

    // Compute Spearman rank correlation between exact and approximate similarity rankings.
    let spearman = spearman_correlation(&exact_sims, &approx_sims);
    assert!(
        spearman > 0.99,
        "Spearman rank correlation is {} (expected > 0.99)",
        spearman
    );
}

/// Compute Spearman rank correlation between two sequences.
fn spearman_correlation(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let n = a.len();

    let rank_a = ranks(a);
    let rank_b = ranks(b);

    // Pearson correlation of the ranks.
    let mean_a: f64 = rank_a.iter().sum::<f64>() / n as f64;
    let mean_b: f64 = rank_b.iter().sum::<f64>() / n as f64;

    let mut cov = 0.0f64;
    let mut var_a = 0.0f64;
    let mut var_b = 0.0f64;

    for i in 0..n {
        let da = rank_a[i] - mean_a;
        let db = rank_b[i] - mean_b;
        cov += da * db;
        var_a += da * da;
        var_b += db * db;
    }

    if var_a == 0.0 || var_b == 0.0 {
        return 0.0;
    }

    cov / (var_a.sqrt() * var_b.sqrt())
}

/// Assign fractional ranks to a slice of f32 values (average rank for ties).
fn ranks(values: &[f32]) -> Vec<f64> {
    let n = values.len();
    let mut indexed: Vec<(usize, f32)> = values.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut result = vec![0.0f64; n];
    let mut i = 0;
    while i < n {
        let mut j = i;
        // Find all elements with the same value (ties).
        while j < n && (indexed[j].1 - indexed[i].1).abs() < f32::EPSILON {
            j += 1;
        }
        // Average rank for this group of ties (1-based ranks).
        let avg_rank = (i + j + 1) as f64 / 2.0; // average of (i+1) .. j
        for k in i..j {
            result[indexed[k].0] = avg_rank;
        }
        i = j;
    }

    result
}
