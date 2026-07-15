use serde::{Deserialize, Serialize};

use crate::{metrics, FibQuantError, Result};

/// Per-layer/head quality summary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvLayerHeadQualityV1 {
    /// Layer index.
    pub layer: u32,
    /// KV head index.
    pub kv_head: u32,
    /// Reconstruction MSE.
    pub reconstruction_mse: f64,
    /// Reconstruction cosine similarity.
    pub cosine_similarity: f64,
    /// Key logit MSE.
    pub key_logit_mse: Option<f64>,
    /// Attention total variation.
    pub attention_tv: Option<f64>,
    /// Attention KL divergence.
    pub attention_kl: Option<f64>,
    /// Top-k agreement ratio.
    pub topk_attention_agreement: Option<f64>,
    /// Value aggregation MSE.
    pub value_aggregation_mse: Option<f64>,
}

/// Synthetic attention quality report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvAttentionQualityReportV1 {
    /// Schema marker.
    pub schema_version: String,
    /// Overall reconstruction MSE.
    pub reconstruction_mse: f64,
    /// Overall reconstruction cosine similarity.
    pub cosine_similarity: f64,
    /// Key logit MSE if keys were evaluated.
    pub key_logit_mse: Option<f64>,
    /// Attention total variation distance.
    pub attention_tv: Option<f64>,
    /// Attention KL divergence.
    pub attention_kl: Option<f64>,
    /// Top-k agreement ratio.
    pub topk_attention_agreement: Option<f64>,
    /// Value aggregation MSE.
    pub value_aggregation_mse: Option<f64>,
    /// Optional per-layer/head reports.
    pub layer_head: Vec<KvLayerHeadQualityV1>,
}

impl KvAttentionQualityReportV1 {
    /// Build a reconstruction-only report.
    pub fn reconstruction_only(raw: &[f32], decoded: &[f32]) -> Result<Self> {
        Ok(Self {
            schema_version: "fib_quant_kv_attention_quality_v1".into(),
            reconstruction_mse: metrics::mse(raw, decoded)?,
            cosine_similarity: metrics::cosine_similarity(raw, decoded)?,
            key_logit_mse: None,
            attention_tv: None,
            attention_kl: None,
            topk_attention_agreement: None,
            value_aggregation_mse: None,
            layer_head: Vec::new(),
        })
    }

    /// Validate metric finiteness.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != "fib_quant_kv_attention_quality_v1" {
            return Err(FibQuantError::CorruptPayload(
                "invalid kv quality schema".into(),
            ));
        }
        for (name, value) in [
            ("reconstruction_mse", Some(self.reconstruction_mse)),
            ("cosine_similarity", Some(self.cosine_similarity)),
            ("key_logit_mse", self.key_logit_mse),
            ("attention_tv", self.attention_tv),
            ("attention_kl", self.attention_kl),
            ("topk_attention_agreement", self.topk_attention_agreement),
            ("value_aggregation_mse", self.value_aggregation_mse),
        ] {
            if let Some(value) = value {
                if !value.is_finite() {
                    return Err(FibQuantError::CorruptPayload(format!(
                        "{name} must be finite"
                    )));
                }
            }
        }
        Ok(())
    }
}

pub(crate) fn total_variation(left: &[f32], right: &[f32]) -> Result<f64> {
    same_len_nonempty(left, right)?;
    Ok(0.5
        * left
            .iter()
            .zip(right)
            .map(|(a, b)| (f64::from(*a) - f64::from(*b)).abs())
            .sum::<f64>())
}

pub(crate) fn kl_divergence(left: &[f32], right: &[f32]) -> Result<f64> {
    same_len_nonempty(left, right)?;
    let eps = 1.0e-12;
    let kl = left
        .iter()
        .zip(right)
        .map(|(p, q)| {
            let p = f64::from(*p).max(eps);
            let q = f64::from(*q).max(eps);
            p * (p / q).ln()
        })
        .sum();
    Ok(kl)
}

pub(crate) fn topk_agreement(left: &[f32], right: &[f32], k: usize) -> Result<f64> {
    same_len_nonempty(left, right)?;
    let k = k.min(left.len()).max(1);
    let left_top = topk_indices(left, k);
    let right_top = topk_indices(right, k);
    let overlap = left_top
        .iter()
        .filter(|idx| right_top.contains(idx))
        .count();
    Ok(overlap as f64 / k as f64)
}

fn topk_indices(values: &[f32], k: usize) -> Vec<usize> {
    let mut indexed: Vec<(usize, f32)> = values.iter().copied().enumerate().collect();
    indexed.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    indexed.into_iter().take(k).map(|(idx, _)| idx).collect()
}

fn same_len_nonempty(left: &[f32], right: &[f32]) -> Result<()> {
    if left.len() != right.len() {
        return Err(FibQuantError::CorruptPayload(
            "kv quality length mismatch".into(),
        ));
    }
    if left.is_empty() {
        return Err(FibQuantError::ZeroDimension);
    }
    if left.iter().chain(right).any(|value| !value.is_finite()) {
        return Err(FibQuantError::CorruptPayload(
            "kv quality inputs must be finite".into(),
        ));
    }
    Ok(())
}
