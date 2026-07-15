//! Compressed-domain attention: approximate logits on compressed keys,
//! top-K value decode only.
//!
//! This module implements the core insight of compressed attention: you
//! don't need to decompress every key vector to compute attention logits.
//! The [`FibScorer`] can estimate `<query, key>` directly from the packed
//! codeword indices via the Gram table, avoiding full decompression of the
//! rotation-inverse + norm-scaling pipeline. Only the top-K value vectors
//! (selected by approximate probability) need to be decompressed.
//!
//! This trades a small amount of logit accuracy for a large reduction in
//! decode work: instead of `N` decompressions, only `top_k` are needed.

use crate::{
    codec::{FibCodeV1, FibQuantizer},
    scoring::FibScorer,
    FibQuantError, Result,
};

/// Output of compressed-domain attention with top-K value decode.
#[derive(Debug, Clone)]
pub struct CompressedAttentionOutput {
    /// Approximate attention logits (pre-softmax), one per key.
    pub logits: Vec<f32>,
    /// Softmax probabilities derived from the approximate logits.
    pub probabilities: Vec<f32>,
    /// Weighted-aggregated output vector (length = head_dim).
    pub output: Vec<f32>,
    /// Indices of the top-K positions selected by probability (descending).
    pub top_k_indices: Vec<usize>,
    /// Number of value vectors actually decompressed (should be ≤ top_k).
    pub decompression_count: usize,
}

/// Compute approximate attention logits on compressed keys WITHOUT full
/// decompression.
///
/// Uses [`FibScorer::prepare_query`] + [`FibScorer::score_prepared`] for
/// efficient batch scoring: the query is rotated and quantized once, then
/// each compressed key is scored via Gram-table lookup only — no
/// rotation-inverse or codeword reconstruction is needed.
///
/// The logits are scaled by `1/sqrt(head_dim)` as in standard scaled
/// dot-product attention, where `head_dim = query.len()`.
///
/// # Arguments
/// * `query` — Query vector, length = ambient_dim (= head_dim).
/// * `compressed_keys` — Compressed key codes (`FibCodeV1`).
/// * `scorer` — [`FibScorer`] wrapping the quantizer and Gram table.
///
/// # Errors
/// Returns [`FibQuantError::ZeroDimension`] if the query is empty,
/// [`FibQuantError::CorruptPayload`] if any input is non-finite.
pub fn compressed_attention_logits(
    query: &[f32],
    compressed_keys: &[FibCodeV1],
    scorer: &FibScorer,
) -> Result<Vec<f32>> {
    if query.is_empty() {
        return Err(FibQuantError::ZeroDimension);
    }
    if compressed_keys.is_empty() {
        return Ok(Vec::new());
    }
    check_finite(query)?;

    let head_dim = query.len();
    let scale = (head_dim as f64).sqrt() as f32;

    // Prepare the query once for batch scoring (rotation + argmin).
    let prepared = scorer.prepare_query(query)?;

    // Gather all key indices and norms for the C kernel.
    let block_count = scorer.quantizer().profile().block_count() as usize;
    let gram = scorer.gram_table();
    let gram_size = gram.n();
    let gram_values = gram.values();

    let n_keys = compressed_keys.len();
    let mut all_key_indices = Vec::with_capacity(n_keys * block_count);
    let mut key_norms = Vec::with_capacity(n_keys);
    for code in compressed_keys {
        let stored_indices = crate::bitpack::unpack_indices(
            &code.indices,
            block_count,
            scorer.quantizer().profile().wire_index_bits,
        )?;
        let stored_norm =
            crate::scoring::decode_stored_norm(code, scorer.quantizer().profile())? as f32;
        for &idx in &stored_indices {
            all_key_indices.push(idx as u16);
        }
        key_norms.push(stored_norm);
    }

    // Convert query indices to u16 for the C kernel.
    let query_indices_u16: Vec<u16> = prepared.query_indices.iter().map(|&i| i as u16).collect();

    let logits = crate::ffi::c_compressed_attention_logits(
        &all_key_indices,
        n_keys,
        &key_norms,
        gram_values,
        gram_size,
        &query_indices_u16,
        block_count,
        prepared.query_norm as f32,
        scale,
    );

    // Validate logits are finite.
    check_finite(&logits)?;
    Ok(logits)
}

/// Compute compressed-domain attention with top-K value decode.
///
/// Pipeline:
/// 1. Compute approximate logits on compressed keys (no decompression).
/// 2. Softmax the logits to get attention probabilities.
/// 3. Select top-K positions by probability (descending).
/// 4. Decode ONLY the top-K value vectors via [`FibQuantizer::decode`].
/// 5. Weighted-aggregate the top-K decoded values by their probabilities.
///
/// The `decompression_count` in the output will be `min(top_k, len)` — NOT
/// the total number of values. This is the key efficiency win: with `N`
/// stored positions and `top_k << N`, only `top_k` decode operations are
/// performed instead of `N`.
///
/// # Arguments
/// * `query` — Query vector, length = ambient_dim (= head_dim).
/// * `compressed_keys` — Compressed key codes.
/// * `compressed_values` — Compressed value codes (same length as keys).
/// * `scorer` — [`FibScorer`] for approximate inner product scoring.
/// * `quantizer` — [`FibQuantizer`] for decoding value vectors.
/// * `top_k` — Number of top-probability positions to decompress and aggregate.
///
/// # Errors
/// Returns [`FibQuantError::ZeroDimension`] if the query is empty,
/// [`FibQuantError::CorruptPayload`] if keys/values length mismatch or
/// any input is non-finite.
pub fn compressed_attention_topk(
    query: &[f32],
    compressed_keys: &[FibCodeV1],
    compressed_values: &[FibCodeV1],
    scorer: &FibScorer,
    quantizer: &FibQuantizer,
    top_k: usize,
) -> Result<CompressedAttentionOutput> {
    if query.is_empty() {
        return Err(FibQuantError::ZeroDimension);
    }
    if compressed_keys.is_empty() {
        return Err(FibQuantError::CorruptPayload(
            "compressed_attention_topk: empty keys".into(),
        ));
    }
    if compressed_keys.len() != compressed_values.len() {
        return Err(FibQuantError::CorruptPayload(format!(
            "compressed_attention_topk: {} keys but {} values",
            compressed_keys.len(),
            compressed_values.len()
        )));
    }
    check_finite(query)?;

    // 1. Compute approximate logits on compressed keys (no decompression).
    let logits = compressed_attention_logits(query, compressed_keys, scorer)?;

    // 2. Softmax → attention probabilities.
    let probabilities = softmax(&logits)?;

    // 3. Select top-K positions by descending probability.
    let n = compressed_keys.len();
    let k = top_k.min(n).max(1);
    let top_k_indices = topk_indices_by_probability(&probabilities, k);

    // 4. Decode ONLY the top-K value vectors and weighted-aggregate.
    let head_dim = query.len();
    let mut output = vec![0.0f64; head_dim];
    let mut decompression_count = 0usize;

    for &idx in &top_k_indices {
        let decoded = quantizer.decode(&compressed_values[idx])?;
        decompression_count += 1;
        let prob = f64::from(probabilities[idx]);
        for (channel, acc) in decoded.iter().zip(output.iter_mut()) {
            *acc += prob * f64::from(*channel);
        }
    }

    let output: Vec<f32> = output.into_iter().map(|v| v as f32).collect();

    Ok(CompressedAttentionOutput {
        logits,
        probabilities,
        output,
        top_k_indices,
        decompression_count,
    })
}

// ──────────────────────────────────────────────────────────────────────
//  Internal helpers
// ──────────────────────────────────────────────────────────────────────

/// Numerically stable softmax with max-subtraction (f64 accumulator).
/// Delegates the inner loop to the C kernel (`fq_softmax`).
fn softmax(logits: &[f32]) -> Result<Vec<f32>> {
    if logits.is_empty() {
        return Err(FibQuantError::ZeroDimension);
    }
    check_finite(logits)?;
    let mut logits_mut = logits.to_vec();
    crate::ffi::c_softmax(&mut logits_mut).map_err(|_| {
        FibQuantError::NumericalFailure("compressed attention softmax underflow".into())
    })?;
    Ok(logits_mut)
}

/// Select top-K indices by descending probability.
/// Ties are broken by ascending index for determinism.
fn topk_indices_by_probability(probabilities: &[f32], k: usize) -> Vec<usize> {
    let mut indexed: Vec<(usize, f32)> = probabilities.iter().copied().enumerate().collect();
    // Sort by descending probability, ties broken by ascending index.
    indexed.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    indexed.into_iter().take(k).map(|(idx, _)| idx).collect()
}

/// Check that all values are finite.
fn check_finite(values: &[f32]) -> Result<()> {
    if values.iter().any(|v| !v.is_finite()) {
        return Err(FibQuantError::CorruptPayload(
            "compressed attention input contains non-finite value".into(),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::FibQuantProfileV1;

    /// Build a test quantizer: ambient_dim=8, block_dim=2, codebook_size=32.
    fn build_test_quantizer() -> Result<FibQuantizer> {
        let profile = FibQuantProfileV1::paper_default(8, 2, 32, 7)?;
        FibQuantizer::new(profile)
    }

    /// Simple MSE between two slices (for test assertions only).
    fn mse(a: &[f32], b: &[f32]) -> f64 {
        assert_eq!(a.len(), b.len(), "mse length mismatch");
        if a.is_empty() {
            return 0.0;
        }
        let sum: f64 = a
            .iter()
            .zip(b)
            .map(|(x, y)| {
                let d = f64::from(*x) - f64::from(*y);
                d * d
            })
            .sum();
        sum / a.len() as f64
    }

    #[test]
    fn test_compressed_attention_vs_reference() -> Result<()> {
        let quantizer = build_test_quantizer()?;
        let scorer = FibScorer::new(quantizer.clone())?;
        let head_dim = 8usize;

        // Synthetic query
        let query: Vec<f32> = vec![0.1, -0.2, 0.3, 0.4, -0.5, 0.6, -0.7, 0.8];

        // 6 synthetic key/value positions
        let raw_keys: Vec<Vec<f32>> = vec![
            vec![0.8, -0.1, 0.2, 0.3, -0.4, 0.5, -0.6, 0.7],
            vec![-0.3, 0.4, -0.5, 0.6, 0.7, -0.8, 0.1, -0.2],
            vec![0.5, 0.5, -0.5, 0.1, 0.2, -0.3, 0.4, 0.5],
            vec![-0.2, 0.3, 0.4, -0.6, 0.5, -0.1, 0.2, -0.7],
            vec![0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1, 0.1],
            vec![0.6, -0.5, 0.4, -0.3, 0.2, -0.1, 0.8, -0.6],
        ];
        let raw_values: Vec<Vec<f32>> = vec![
            vec![0.2, 0.3, -0.1, 0.5, 0.4, -0.2, 0.6, 0.1],
            vec![-0.5, 0.4, 0.3, -0.2, 0.6, 0.1, -0.3, 0.5],
            vec![0.7, -0.3, 0.2, 0.4, -0.1, 0.5, 0.3, -0.4],
            vec![0.1, -0.6, 0.3, 0.2, -0.4, 0.7, -0.1, 0.3],
            vec![0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3, 0.3],
            vec![-0.2, 0.5, -0.4, 0.6, -0.3, 0.2, 0.7, -0.5],
        ];

        // Encode keys and values
        let compressed_keys: Vec<FibCodeV1> = raw_keys
            .iter()
            .map(|k| quantizer.encode(k))
            .collect::<Result<Vec<_>>>()?;
        let compressed_values: Vec<FibCodeV1> = raw_values
            .iter()
            .map(|v| quantizer.encode(v))
            .collect::<Result<Vec<_>>>()?;

        // --- Compressed attention (top-K = 4) ---
        let top_k = 4usize;
        let compressed_out = compressed_attention_topk(
            &query,
            &compressed_keys,
            &compressed_values,
            &scorer,
            &quantizer,
            top_k,
        )?;

        // Verify structural properties
        assert_eq!(
            compressed_out.decompression_count, top_k,
            "decompression_count should be {}, got {}",
            top_k, compressed_out.decompression_count
        );
        assert_eq!(compressed_out.top_k_indices.len(), top_k);
        assert_eq!(compressed_out.output.len(), head_dim);
        assert_eq!(compressed_out.logits.len(), raw_keys.len());
        assert_eq!(compressed_out.probabilities.len(), raw_keys.len());

        // Probabilities should sum to ~1.0
        let prob_sum: f64 = compressed_out
            .probabilities
            .iter()
            .map(|p| f64::from(*p))
            .sum();
        assert!(
            (prob_sum - 1.0).abs() < 1e-5,
            "probabilities should sum to 1.0, got {}",
            prob_sum
        );

        // --- Reference: decode ALL keys and values, compute exact attention ---
        let decoded_keys: Vec<Vec<f32>> = compressed_keys
            .iter()
            .map(|c| quantizer.decode(c))
            .collect::<Result<Vec<_>>>()?;
        let decoded_values: Vec<Vec<f32>> = compressed_values
            .iter()
            .map(|c| quantizer.decode(c))
            .collect::<Result<Vec<_>>>()?;

        let flat_decoded_keys: Vec<f32> = decoded_keys.iter().flatten().copied().collect();
        let flat_decoded_values: Vec<f32> = decoded_values.iter().flatten().copied().collect();

        let ref_logits = super::super::attention_ref::reference_attention_logits(
            &query,
            &flat_decoded_keys,
            head_dim,
        )?;
        let ref_probs = softmax_local(&ref_logits)?;
        let ref_output = super::super::attention_ref::reference_value_aggregation(
            &ref_probs,
            &flat_decoded_values,
            head_dim,
        )?;

        // Logits should be in the same ballpark (approximate scoring).
        let logit_mse = mse(&compressed_out.logits, &ref_logits);
        assert!(logit_mse < 2.0, "logit MSE too large: {}", logit_mse);

        // Output should be in the same ballpark.
        // The compressed path uses top-K (4 of 6) with approximate probabilities,
        // while the reference uses all 6 with exact probabilities on decoded keys.
        let output_mse = mse(&compressed_out.output, &ref_output);
        assert!(output_mse < 0.5, "output MSE too large: {}", output_mse);

        // Top-K indices should have meaningful overlap with reference top-K.
        let ref_topk = topk_indices_by_probability(&ref_probs, top_k);
        let overlap = compressed_out
            .top_k_indices
            .iter()
            .filter(|idx| ref_topk.contains(idx))
            .count();
        let agreement = overlap as f64 / top_k as f64;
        assert!(
            agreement >= 0.5,
            "top-K agreement too low: {}/{} (compressed={:?}, ref={:?})",
            overlap,
            top_k,
            compressed_out.top_k_indices,
            ref_topk
        );

        Ok(())
    }

    #[test]
    fn test_empty_keys_returns_empty_logits() -> Result<()> {
        let quantizer = build_test_quantizer()?;
        let scorer = FibScorer::new(quantizer.clone())?;
        let query: Vec<f32> = vec![0.1, -0.2, 0.3, 0.4, -0.5, 0.6, -0.7, 0.8];

        let logits = compressed_attention_logits(&query, &[], &scorer)?;
        assert!(logits.is_empty());
        Ok(())
    }

    #[test]
    fn test_single_key_logit_finite() -> Result<()> {
        let quantizer = build_test_quantizer()?;
        let scorer = FibScorer::new(quantizer.clone())?;

        let query: Vec<f32> = vec![0.1, -0.2, 0.3, 0.4, -0.5, 0.6, -0.7, 0.8];
        let key: Vec<f32> = vec![0.5, 0.5, -0.5, 0.1, 0.2, -0.3, 0.4, 0.5];
        let compressed_key = quantizer.encode(&key)?;

        let logits = compressed_attention_logits(&query, &[compressed_key], &scorer)?;
        assert_eq!(logits.len(), 1);
        assert!(logits[0].is_finite());
        Ok(())
    }

    #[test]
    fn test_topk_exceeds_n_clamps() -> Result<()> {
        let quantizer = build_test_quantizer()?;
        let scorer = FibScorer::new(quantizer.clone())?;
        let head_dim = 8usize;

        let query: Vec<f32> = vec![0.1, -0.2, 0.3, 0.4, -0.5, 0.6, -0.7, 0.8];
        let keys: Vec<Vec<f32>> = vec![
            vec![0.8, -0.1, 0.2, 0.3, -0.4, 0.5, -0.6, 0.7],
            vec![-0.3, 0.4, -0.5, 0.6, 0.7, -0.8, 0.1, -0.2],
            vec![0.5, 0.5, -0.5, 0.1, 0.2, -0.3, 0.4, 0.5],
        ];
        let compressed_keys: Vec<FibCodeV1> = keys
            .iter()
            .map(|k| quantizer.encode(k))
            .collect::<Result<Vec<_>>>()?;
        let compressed_values: Vec<FibCodeV1> = compressed_keys.clone();

        // top_k=10 but only 3 keys — should clamp to 3
        let out = compressed_attention_topk(
            &query,
            &compressed_keys,
            &compressed_values,
            &scorer,
            &quantizer,
            10,
        )?;
        assert_eq!(out.decompression_count, 3);
        assert_eq!(out.top_k_indices.len(), 3);
        assert_eq!(out.output.len(), head_dim);
        Ok(())
    }

    /// Local softmax for test comparisons (avoids importing private fns).
    fn softmax_local(logits: &[f32]) -> Result<Vec<f32>> {
        use crate::FibQuantError;
        if logits.is_empty() {
            return Err(FibQuantError::ZeroDimension);
        }
        let max = logits
            .iter()
            .copied()
            .fold(f32::NEG_INFINITY, |acc, v| acc.max(v));
        let mut sum = 0.0f64;
        let mut exps = Vec::with_capacity(logits.len());
        for &v in logits {
            let exp = f64::from(v - max).exp();
            sum += exp;
            exps.push(exp);
        }
        Ok(exps.into_iter().map(|e| (e / sum) as f32).collect())
    }
}
