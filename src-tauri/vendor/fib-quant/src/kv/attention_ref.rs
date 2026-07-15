use crate::{metrics, FibQuantError, Result};

use super::quality::{kl_divergence, topk_agreement, total_variation, KvAttentionQualityReportV1};

/// Compute reference attention logits for one query and a flat key matrix.
pub fn reference_attention_logits(
    query: &[f32],
    keys: &[f32],
    head_dim: usize,
) -> Result<Vec<f32>> {
    if head_dim == 0 || query.len() != head_dim || keys.len() % head_dim != 0 {
        return Err(FibQuantError::CorruptPayload(
            "invalid attention logit dimensions".into(),
        ));
    }
    check_finite(query)?;
    check_finite(keys)?;
    let scale = (head_dim as f64).sqrt();
    let mut logits = Vec::with_capacity(keys.len() / head_dim);
    for key in keys.chunks_exact(head_dim) {
        let dot = query
            .iter()
            .zip(key)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum::<f64>();
        logits.push((dot / scale) as f32);
    }
    Ok(logits)
}

/// Compute reference value aggregation from attention probabilities.
pub fn reference_value_aggregation(
    probabilities: &[f32],
    values: &[f32],
    head_dim: usize,
) -> Result<Vec<f32>> {
    if head_dim == 0
        || values.len() % head_dim != 0
        || values.len() / head_dim != probabilities.len()
    {
        return Err(FibQuantError::CorruptPayload(
            "invalid value aggregation dimensions".into(),
        ));
    }
    check_finite(probabilities)?;
    check_finite(values)?;
    let mut out = vec![0.0f64; head_dim];
    for (prob, value) in probabilities.iter().zip(values.chunks_exact(head_dim)) {
        for (idx, channel) in value.iter().enumerate() {
            out[idx] += f64::from(*prob) * f64::from(*channel);
        }
    }
    Ok(out.into_iter().map(|value| value as f32).collect())
}

/// Compare raw and decoded synthetic attention fixtures.
pub fn compare_attention_fixture(
    query: &[f32],
    raw_keys: &[f32],
    decoded_keys: &[f32],
    raw_values: &[f32],
    decoded_values: &[f32],
    head_dim: usize,
    top_k: usize,
) -> Result<KvAttentionQualityReportV1> {
    let raw_logits = reference_attention_logits(query, raw_keys, head_dim)?;
    let decoded_logits = reference_attention_logits(query, decoded_keys, head_dim)?;
    let raw_probs = softmax(&raw_logits)?;
    let decoded_probs = softmax(&decoded_logits)?;
    let raw_agg = reference_value_aggregation(&raw_probs, raw_values, head_dim)?;
    let decoded_agg = reference_value_aggregation(&decoded_probs, decoded_values, head_dim)?;
    let mut report = KvAttentionQualityReportV1::reconstruction_only(raw_keys, decoded_keys)?;
    report.key_logit_mse = Some(metrics::mse(&raw_logits, &decoded_logits)?);
    report.attention_tv = Some(total_variation(&raw_probs, &decoded_probs)?);
    report.attention_kl = Some(kl_divergence(&raw_probs, &decoded_probs)?);
    report.topk_attention_agreement = Some(topk_agreement(&raw_logits, &decoded_logits, top_k)?);
    report.value_aggregation_mse = Some(metrics::mse(&raw_agg, &decoded_agg)?);
    report.validate()?;
    Ok(report)
}

fn softmax(logits: &[f32]) -> Result<Vec<f32>> {
    if logits.is_empty() {
        return Err(FibQuantError::ZeroDimension);
    }
    check_finite(logits)?;
    let max = logits
        .iter()
        .copied()
        .fold(f32::NEG_INFINITY, |acc, value| acc.max(value));
    let mut sum = 0.0f64;
    let mut out = Vec::with_capacity(logits.len());
    for value in logits {
        let exp = f64::from(*value - max).exp();
        sum += exp;
        out.push(exp);
    }
    if !sum.is_finite() || sum <= 0.0 {
        return Err(FibQuantError::NumericalFailure(
            "attention softmax underflow".into(),
        ));
    }
    Ok(out.into_iter().map(|value| (value / sum) as f32).collect())
}

fn check_finite(values: &[f32]) -> Result<()> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(FibQuantError::CorruptPayload(
            "attention input contains non-finite value".into(),
        ));
    }
    Ok(())
}
