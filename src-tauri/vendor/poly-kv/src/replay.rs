use crate::codec::create_codec;
use crate::error::{PolyKvError, Result};
use crate::policy::CODEC_TURBO_8BIT;
use crate::{AgentShell, SharedKVPool};
use serde::{Deserialize, Serialize};

/// Schema for model-shaped compressed-attention replay receipts.
pub const MODEL_REPLAY_RECEIPT_SCHEMA: &str = "poly_kv_model_replay_receipt_v1";

/// One replay query plus a synthetic label used for PPL-proxy comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelReplayQuery {
    /// Query vector for one layer/head.
    pub query: Vec<f32>,
    /// Label token in the deterministic projection vocabulary.
    pub label_token: usize,
}

/// Configuration for model-shaped replay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelReplayConfig {
    /// Layer index to replay.
    pub layer: usize,
    /// KV head index to replay.
    pub head: usize,
    /// Candidate budgets to sweep. First passing value is selected.
    pub candidate_ks: Vec<usize>,
    /// Synthetic projection vocabulary size.
    pub vocab_size: usize,
    /// Seed for deterministic output projection.
    pub projection_seed: u64,
    /// Minimum mean output cosine for a candidate budget.
    pub min_output_cosine: f64,
    /// Maximum mean attention-output MSE for a candidate budget.
    pub max_output_mse: f64,
    /// Maximum mean KL(exact || compressed) for projected logits.
    pub max_kl_divergence: f64,
    /// Maximum allowed PPL-proxy delta.
    pub max_ppl_delta: f64,
    /// Minimum top-1 logit agreement rate.
    pub min_top1_agreement: f64,
}

impl Default for ModelReplayConfig {
    fn default() -> Self {
        Self {
            layer: 0,
            head: 0,
            candidate_ks: Vec::new(),
            vocab_size: 64,
            projection_seed: 0,
            min_output_cosine: 0.75,
            max_output_mse: 0.25,
            max_kl_divergence: 0.50,
            max_ppl_delta: 1.0,
            min_top1_agreement: 0.25,
        }
    }
}

/// Metrics for one candidate-k replay pass.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CandidateReplayMetrics {
    pub candidate_k: usize,
    pub output_cosine_mean: f64,
    pub output_mse_mean: f64,
    pub kl_divergence_mean: f64,
    pub top1_agreement: f64,
    pub ppl_proxy_exact: f64,
    pub ppl_proxy_compressed: f64,
    pub ppl_proxy_delta: f64,
    pub decoded_values_total: u64,
    pub full_decode_value_count: u64,
    pub decode_reduction: f64,
    pub passed: bool,
    pub blockers: Vec<String>,
}

/// Aggregate replay metrics for the selected candidate budget.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelReplayMetrics {
    pub query_count: usize,
    pub exact_attention_outputs: u64,
    pub logit_vectors_compared: u64,
    pub output_cosine_mean: f64,
    pub output_mse_mean: f64,
    pub kl_divergence_mean: f64,
    pub top1_agreement: f64,
    pub ppl_proxy_exact: f64,
    pub ppl_proxy_compressed: f64,
    pub ppl_proxy_delta: f64,
    pub decoded_values_total: u64,
    pub full_decode_value_count: u64,
    pub decode_reduction: f64,
}

/// Model-shaped replay receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelReplayReceipt {
    pub schema_version: String,
    pub claim_boundary: String,
    pub config: ModelReplayConfig,
    pub selected_candidate_k: usize,
    pub candidate_results: Vec<CandidateReplayMetrics>,
    pub metrics: ModelReplayMetrics,
    pub passed: bool,
    pub blockers: Vec<String>,
}

/// Schema for captured-tensor model replay receipts.
pub const CAPTURED_MODEL_REPLAY_RECEIPT_SCHEMA: &str = "poly_kv_captured_model_replay_v1";

/// One captured query/key/value/logit sample.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedReplayQuery {
    pub query: Vec<f32>,
    pub keys: Vec<Vec<f32>>,
    pub values: Vec<Vec<f32>>,
    pub exact_attention_output: Vec<f32>,
    pub exact_logits: Vec<f32>,
    pub label_token: usize,
}

/// Captured tensor replay fixture.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedReplayFixture {
    pub schema_version: String,
    pub model_id: String,
    pub head_dim: usize,
    pub shared_tokens: usize,
    pub seed: u64,
    pub output_projection: Vec<Vec<f32>>,
    pub queries: Vec<CapturedReplayQuery>,
}

/// Captured replay gate config.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedReplayConfig {
    pub candidate_ks: Vec<usize>,
    pub min_output_cosine: f64,
    pub max_output_mse: f64,
    pub max_kl_divergence: f64,
    pub max_ppl_delta: f64,
    pub min_top1_agreement: f64,
}

/// Captured tensor replay receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CapturedReplayReceipt {
    pub schema_version: String,
    pub model_id: String,
    pub claim_boundary: String,
    pub config: CapturedReplayConfig,
    pub selected_candidate_k: usize,
    pub candidate_results: Vec<CandidateReplayMetrics>,
    pub metrics: ModelReplayMetrics,
    pub passed: bool,
    pub blockers: Vec<String>,
}

#[derive(Clone)]
struct ExactCandidate {
    key: Vec<f32>,
    value: Vec<f32>,
}

/// Run a deterministic model-shaped replay over the compressed pool+shell path.
///
/// This compares compressed candidate selection against exact full-decode
/// attention and then projects both outputs through a deterministic synthetic
/// output head to produce logit/KL/PPL-proxy metrics. It is not real model PPL.
pub fn run_model_replay(
    pool: &SharedKVPool,
    shell: &AgentShell,
    queries: &[ModelReplayQuery],
    config: ModelReplayConfig,
) -> Result<ModelReplayReceipt> {
    if config.candidate_ks.is_empty() {
        return Err(PolyKvError::InvalidPolicy(
            "candidate_ks must not be empty".to_string(),
        ));
    }
    if queries.is_empty() {
        return Err(PolyKvError::InvalidPolicy(
            "queries must not be empty".to_string(),
        ));
    }
    if config.vocab_size == 0 {
        return Err(PolyKvError::InvalidPolicy(
            "vocab_size must be greater than zero".to_string(),
        ));
    }

    let head_dim = pool.manifest.shape.head_dim;
    for query in queries {
        if query.query.len() != head_dim {
            return Err(PolyKvError::DimensionMismatch {
                expected: head_dim,
                got: query.query.len(),
            });
        }
        if query.label_token >= config.vocab_size {
            return Err(PolyKvError::InvalidPolicy(format!(
                "label_token {} >= vocab_size {}",
                query.label_token, config.vocab_size
            )));
        }
    }

    let exact_candidates = exact_candidates(pool, shell, config.layer, config.head)?;
    let full_decode_value_count = (exact_candidates.len() * queries.len()) as u64;
    let projection = output_projection(config.vocab_size, head_dim, config.projection_seed);
    let mut candidate_results = Vec::with_capacity(config.candidate_ks.len());

    for &candidate_k in &config.candidate_ks {
        candidate_results.push(eval_candidate_k(
            pool,
            shell,
            queries,
            &config,
            &exact_candidates,
            &projection,
            candidate_k,
            full_decode_value_count,
        )?);
    }

    let selected_idx = candidate_results
        .iter()
        .position(|r| r.passed)
        .unwrap_or(candidate_results.len() - 1);
    let selected = candidate_results[selected_idx].clone();
    let passed = selected.passed;
    let blockers = if passed {
        Vec::new()
    } else {
        selected.blockers.clone()
    };

    Ok(ModelReplayReceipt {
        schema_version: MODEL_REPLAY_RECEIPT_SCHEMA.to_string(),
        claim_boundary: "deterministic model-shaped replay over synthetic projection; not real model PPL, not production KV-cache preservation, and not provider/framework KV-cache byte-reduction evidence".to_string(),
        config,
        selected_candidate_k: selected.candidate_k,
        metrics: ModelReplayMetrics {
            query_count: queries.len(),
            exact_attention_outputs: queries.len() as u64,
            logit_vectors_compared: queries.len() as u64,
            output_cosine_mean: selected.output_cosine_mean,
            output_mse_mean: selected.output_mse_mean,
            kl_divergence_mean: selected.kl_divergence_mean,
            top1_agreement: selected.top1_agreement,
            ppl_proxy_exact: selected.ppl_proxy_exact,
            ppl_proxy_compressed: selected.ppl_proxy_compressed,
            ppl_proxy_delta: selected.ppl_proxy_delta,
            decoded_values_total: selected.decoded_values_total,
            full_decode_value_count,
            decode_reduction: selected.decode_reduction,
        },
        candidate_results,
        passed,
        blockers,
    })
}

/// Run captured-tensor replay through the existing compressed pool+shell path.
pub fn run_captured_model_replay(
    fixture: &CapturedReplayFixture,
    config: CapturedReplayConfig,
) -> Result<CapturedReplayReceipt> {
    validate_captured_fixture(fixture, &config)?;
    let full_decode_value_count = (fixture
        .queries
        .iter()
        .map(|query| query.values.len())
        .sum::<usize>()
        * config.candidate_ks.len().max(1)
        / config.candidate_ks.len().max(1)) as u64;
    let mut candidate_results = Vec::with_capacity(config.candidate_ks.len());
    for &candidate_k in &config.candidate_ks {
        candidate_results.push(eval_captured_candidate_k(fixture, &config, candidate_k)?);
    }
    let selected_idx = candidate_results
        .iter()
        .position(|result| result.passed)
        .unwrap_or(candidate_results.len() - 1);
    let selected = candidate_results[selected_idx].clone();
    let passed = selected.passed;
    let blockers = if passed {
        Vec::new()
    } else {
        selected.blockers.clone()
    };
    Ok(CapturedReplayReceipt {
        schema_version: CAPTURED_MODEL_REPLAY_RECEIPT_SCHEMA.to_string(),
        model_id: fixture.model_id.clone(),
        claim_boundary: "captured tensor replay against fixture Q/K/V/logits; not pretrained LLM PPL, not production KV-cache preservation, and not provider/framework KV-cache byte-reduction evidence".to_string(),
        config,
        selected_candidate_k: selected.candidate_k,
        metrics: ModelReplayMetrics {
            query_count: fixture.queries.len(),
            exact_attention_outputs: fixture.queries.len() as u64,
            logit_vectors_compared: fixture.queries.len() as u64,
            output_cosine_mean: selected.output_cosine_mean,
            output_mse_mean: selected.output_mse_mean,
            kl_divergence_mean: selected.kl_divergence_mean,
            top1_agreement: selected.top1_agreement,
            ppl_proxy_exact: selected.ppl_proxy_exact,
            ppl_proxy_compressed: selected.ppl_proxy_compressed,
            ppl_proxy_delta: selected.ppl_proxy_delta,
            decoded_values_total: selected.decoded_values_total,
            full_decode_value_count,
            decode_reduction: selected.decode_reduction,
        },
        candidate_results,
        passed,
        blockers,
    })
}

#[allow(clippy::too_many_arguments)]
fn eval_candidate_k(
    pool: &SharedKVPool,
    shell: &AgentShell,
    queries: &[ModelReplayQuery],
    config: &ModelReplayConfig,
    exact_candidates: &[ExactCandidate],
    projection: &[Vec<f32>],
    candidate_k: usize,
    full_decode_value_count: u64,
) -> Result<CandidateReplayMetrics> {
    let mut cosines = Vec::with_capacity(queries.len());
    let mut mses = Vec::with_capacity(queries.len());
    let mut kls = Vec::with_capacity(queries.len());
    let mut exact_nlls = Vec::with_capacity(queries.len());
    let mut compressed_nlls = Vec::with_capacity(queries.len());
    let mut top1_matches = 0usize;
    let mut decoded_values_total = 0u64;

    for query in queries {
        let exact = exact_attention_output(&query.query, exact_candidates);
        let exact_logits = project_logits(&exact, projection);
        let exact_probs = softmax(&exact_logits);
        let exact_top1 = argmax(&exact_logits);
        let exact_nll = nll(&exact_probs, query.label_token);

        let compressed = shell.attention_topk_compressed(
            pool,
            config.layer,
            config.head,
            &query.query,
            candidate_k,
        )?;
        decoded_values_total += compressed.receipt.decoded_value_vectors;
        let compressed_output = compressed_attention_output(&compressed.hits);
        let compressed_logits = project_logits(&compressed_output, projection);
        let compressed_probs = softmax(&compressed_logits);
        let compressed_top1 = argmax(&compressed_logits);
        let compressed_nll = nll(&compressed_probs, query.label_token);

        cosines.push(cosine(&exact, &compressed_output));
        mses.push(mse(&exact, &compressed_output));
        kls.push(kl_divergence(&exact_probs, &compressed_probs));
        exact_nlls.push(exact_nll);
        compressed_nlls.push(compressed_nll);
        if exact_top1 == compressed_top1 {
            top1_matches += 1;
        }
    }

    let output_cosine_mean = mean(&cosines);
    let output_mse_mean = mean(&mses);
    let kl_divergence_mean = mean(&kls);
    let exact_nll = mean(&exact_nlls);
    let compressed_nll = mean(&compressed_nlls);
    let ppl_proxy_exact = exact_nll.exp();
    let ppl_proxy_compressed = compressed_nll.exp();
    let ppl_proxy_delta = ppl_proxy_compressed - ppl_proxy_exact;
    let top1_agreement = top1_matches as f64 / queries.len() as f64;
    let decode_reduction = full_decode_value_count as f64 / decoded_values_total.max(1) as f64;

    let mut blockers = Vec::new();
    if output_cosine_mean < config.min_output_cosine {
        blockers.push(format!(
            "output_cosine_mean {output_cosine_mean:.4} < {:.4}",
            config.min_output_cosine
        ));
    }
    if output_mse_mean > config.max_output_mse {
        blockers.push(format!(
            "output_mse_mean {output_mse_mean:.4} > {:.4}",
            config.max_output_mse
        ));
    }
    if kl_divergence_mean > config.max_kl_divergence {
        blockers.push(format!(
            "kl_divergence_mean {kl_divergence_mean:.4} > {:.4}",
            config.max_kl_divergence
        ));
    }
    let ppl_proxy_delta_abs = ppl_proxy_delta.abs();
    if ppl_proxy_delta_abs > config.max_ppl_delta {
        blockers.push(format!(
            "abs(ppl_proxy_delta) {ppl_proxy_delta_abs:.4} > {:.4}",
            config.max_ppl_delta
        ));
    }
    if top1_agreement < config.min_top1_agreement {
        blockers.push(format!(
            "top1_agreement {top1_agreement:.4} < {:.4}",
            config.min_top1_agreement
        ));
    }

    Ok(CandidateReplayMetrics {
        candidate_k,
        output_cosine_mean,
        output_mse_mean,
        kl_divergence_mean,
        top1_agreement,
        ppl_proxy_exact,
        ppl_proxy_compressed,
        ppl_proxy_delta,
        decoded_values_total,
        full_decode_value_count,
        decode_reduction,
        passed: blockers.is_empty(),
        blockers,
    })
}

fn validate_captured_fixture(
    fixture: &CapturedReplayFixture,
    config: &CapturedReplayConfig,
) -> Result<()> {
    if config.candidate_ks.is_empty() {
        return Err(PolyKvError::InvalidPolicy(
            "candidate_ks must not be empty".to_string(),
        ));
    }
    if fixture.queries.is_empty() {
        return Err(PolyKvError::InvalidPolicy(
            "captured fixture must contain at least one query".to_string(),
        ));
    }
    if fixture.head_dim == 0 {
        return Err(PolyKvError::InvalidPolicy(
            "head_dim must be greater than zero".to_string(),
        ));
    }
    if fixture.output_projection.is_empty() {
        return Err(PolyKvError::InvalidPolicy(
            "output_projection must not be empty".to_string(),
        ));
    }
    for row in &fixture.output_projection {
        if row.len() != fixture.head_dim {
            return Err(PolyKvError::DimensionMismatch {
                expected: fixture.head_dim,
                got: row.len(),
            });
        }
    }
    for query in &fixture.queries {
        if query.query.len() != fixture.head_dim
            || query.exact_attention_output.len() != fixture.head_dim
        {
            return Err(PolyKvError::DimensionMismatch {
                expected: fixture.head_dim,
                got: query.query.len(),
            });
        }
        if query.keys.len() != query.values.len() || query.keys.is_empty() {
            return Err(PolyKvError::InvalidPolicy(
                "captured keys/values must be non-empty and same length".to_string(),
            ));
        }
        if fixture.shared_tokens == 0 || fixture.shared_tokens >= query.keys.len() {
            return Err(PolyKvError::InvalidPolicy(
                "shared_tokens must split captured rows into non-empty pool and shell tiers"
                    .to_string(),
            ));
        }
        if query.exact_logits.len() != fixture.output_projection.len() {
            return Err(PolyKvError::DimensionMismatch {
                expected: fixture.output_projection.len(),
                got: query.exact_logits.len(),
            });
        }
        if query.label_token >= query.exact_logits.len() {
            return Err(PolyKvError::InvalidPolicy(format!(
                "label_token {} >= logits len {}",
                query.label_token,
                query.exact_logits.len()
            )));
        }
        for row in query.keys.iter().chain(query.values.iter()) {
            if row.len() != fixture.head_dim {
                return Err(PolyKvError::DimensionMismatch {
                    expected: fixture.head_dim,
                    got: row.len(),
                });
            }
        }
    }
    Ok(())
}

fn eval_captured_candidate_k(
    fixture: &CapturedReplayFixture,
    config: &CapturedReplayConfig,
    candidate_k: usize,
) -> Result<CandidateReplayMetrics> {
    let mut cosines = Vec::with_capacity(fixture.queries.len());
    let mut mses = Vec::with_capacity(fixture.queries.len());
    let mut kls = Vec::with_capacity(fixture.queries.len());
    let mut exact_nlls = Vec::with_capacity(fixture.queries.len());
    let mut compressed_nlls = Vec::with_capacity(fixture.queries.len());
    let mut top1_matches = 0usize;
    let mut decoded_values_total = 0u64;
    let mut full_decode_value_count = 0u64;

    for (query_idx, query) in fixture.queries.iter().enumerate() {
        let (pool, shell) = build_captured_pool_shell(fixture, query, query_idx)?;
        let compressed = shell.attention_topk_compressed(&pool, 0, 0, &query.query, candidate_k)?;
        decoded_values_total += compressed.receipt.decoded_value_vectors;
        full_decode_value_count += query.values.len() as u64;

        let compressed_output = compressed_attention_output(&compressed.hits);
        let compressed_logits = project_logits(&compressed_output, &fixture.output_projection);
        let exact_probs = softmax(&query.exact_logits);
        let compressed_probs = softmax(&compressed_logits);
        let exact_nll = nll(&exact_probs, query.label_token);
        let compressed_nll = nll(&compressed_probs, query.label_token);
        cosines.push(cosine(&query.exact_attention_output, &compressed_output));
        mses.push(mse(&query.exact_attention_output, &compressed_output));
        kls.push(kl_divergence(&exact_probs, &compressed_probs));
        exact_nlls.push(exact_nll);
        compressed_nlls.push(compressed_nll);
        if argmax(&query.exact_logits) == argmax(&compressed_logits) {
            top1_matches += 1;
        }
    }

    let output_cosine_mean = mean(&cosines);
    let output_mse_mean = mean(&mses);
    let kl_divergence_mean = mean(&kls);
    let ppl_proxy_exact = mean(&exact_nlls).exp();
    let ppl_proxy_compressed = mean(&compressed_nlls).exp();
    let ppl_proxy_delta = ppl_proxy_compressed - ppl_proxy_exact;
    let top1_agreement = top1_matches as f64 / fixture.queries.len() as f64;
    let decode_reduction = full_decode_value_count as f64 / decoded_values_total.max(1) as f64;

    let mut blockers = Vec::new();
    if output_cosine_mean < config.min_output_cosine {
        blockers.push(format!(
            "output_cosine_mean {output_cosine_mean:.4} < {:.4}",
            config.min_output_cosine
        ));
    }
    if output_mse_mean > config.max_output_mse {
        blockers.push(format!(
            "output_mse_mean {output_mse_mean:.4} > {:.4}",
            config.max_output_mse
        ));
    }
    if kl_divergence_mean > config.max_kl_divergence {
        blockers.push(format!(
            "kl_divergence_mean {kl_divergence_mean:.4} > {:.4}",
            config.max_kl_divergence
        ));
    }
    let ppl_proxy_delta_abs = ppl_proxy_delta.abs();
    if ppl_proxy_delta_abs > config.max_ppl_delta {
        blockers.push(format!(
            "abs(ppl_proxy_delta) {ppl_proxy_delta_abs:.4} > {:.4}",
            config.max_ppl_delta
        ));
    }
    if top1_agreement < config.min_top1_agreement {
        blockers.push(format!(
            "top1_agreement {top1_agreement:.4} < {:.4}",
            config.min_top1_agreement
        ));
    }

    Ok(CandidateReplayMetrics {
        candidate_k,
        output_cosine_mean,
        output_mse_mean,
        kl_divergence_mean,
        top1_agreement,
        ppl_proxy_exact,
        ppl_proxy_compressed,
        ppl_proxy_delta,
        decoded_values_total,
        full_decode_value_count,
        decode_reduction,
        passed: blockers.is_empty(),
        blockers,
    })
}

fn build_captured_pool_shell(
    fixture: &CapturedReplayFixture,
    query: &CapturedReplayQuery,
    query_idx: usize,
) -> Result<(SharedKVPool, AgentShell)> {
    let shape = crate::KvTensorShape {
        attention_type: crate::AttentionType::MHA,
        num_layers: 1,
        num_heads: 1,
        num_kv_heads: 1,
        head_dim: fixture.head_dim,
        hidden_size: fixture.head_dim,
    };
    let rows: Vec<Vec<f32>> = query
        .keys
        .iter()
        .zip(&query.values)
        .map(|(key, value)| {
            let mut row = Vec::with_capacity(fixture.head_dim * 2);
            row.extend_from_slice(key);
            row.extend_from_slice(value);
            row
        })
        .collect();
    let shared: Vec<(String, Vec<f32>)> = rows
        .iter()
        .take(fixture.shared_tokens)
        .enumerate()
        .map(|(idx, row)| (format!("q{query_idx}_shared_{idx}"), row.clone()))
        .collect();
    let hot: Vec<(String, Vec<f32>)> = rows
        .iter()
        .skip(fixture.shared_tokens)
        .enumerate()
        .map(|(idx, row)| (format!("q{query_idx}_hot_{idx}"), row.clone()))
        .collect();
    let (pool, _) = SharedKVPool::build(&shared, &shape, fixture.seed + query_idx as u64)?;
    let (shell, _) = pool.materialize_shell(
        &format!("captured_{}_{query_idx}", fixture.model_id),
        &hot,
        fixture.seed + 10_000 + query_idx as u64,
    )?;
    Ok((pool, shell))
}

fn exact_candidates(
    pool: &SharedKVPool,
    shell: &AgentShell,
    layer_idx: usize,
    head_idx: usize,
) -> Result<Vec<ExactCandidate>> {
    let layer = pool.decompress_layer(layer_idx)?;
    if head_idx >= layer.num_heads {
        return Err(PolyKvError::Internal(format!(
            "head_idx {head_idx} out of range (have {})",
            layer.num_heads
        )));
    }
    let head_dim = layer.head_dim;
    let mut out = Vec::with_capacity(layer.num_tokens);
    let pool_keys = &layer.keys[head_idx];
    let pool_values = &layer.values[head_idx];
    for token_idx in 0..layer.num_tokens {
        let start = token_idx * head_dim;
        out.push(ExactCandidate {
            key: pool_keys[start..start + head_dim].to_vec(),
            value: pool_values[start..start + head_dim].to_vec(),
        });
    }

    if let Some(shell_layer) = shell
        .unique_layers
        .iter()
        .find(|l| l.layer_index == layer_idx as u32)
    {
        let num_heads = pool.manifest.shape.num_kv_heads as usize;
        let shell_tokens = shell_layer.key_blocks.len() / num_heads;
        let turbo_codec = create_codec(
            CODEC_TURBO_8BIT,
            head_dim,
            None,
            Some(&pool.policy.turbo_config),
        )?;
        for token_idx in 0..shell_tokens {
            let block_idx = token_idx * num_heads + head_idx;
            out.push(ExactCandidate {
                key: turbo_codec.decode(
                    &shell_layer.key_blocks[block_idx].encoded_payload,
                    shell.build_seed,
                )?,
                value: turbo_codec.decode(
                    &shell_layer.value_blocks[block_idx].encoded_payload,
                    shell.build_seed,
                )?,
            });
        }
    }
    Ok(out)
}

fn exact_attention_output(query: &[f32], candidates: &[ExactCandidate]) -> Vec<f32> {
    let scores: Vec<f32> = candidates.iter().map(|c| dot(query, &c.key)).collect();
    let weights = softmax_f32(&scores);
    let dim = candidates.first().map(|c| c.value.len()).unwrap_or(0);
    let mut out = vec![0.0f32; dim];
    for (weight, cand) in weights.iter().zip(candidates) {
        for (dst, value) in out.iter_mut().zip(&cand.value) {
            *dst += *weight * *value;
        }
    }
    out
}

fn compressed_attention_output(hits: &[crate::CompressedShellAttentionHit]) -> Vec<f32> {
    if hits.is_empty() {
        return Vec::new();
    }
    let scores: Vec<f32> = hits.iter().map(|h| h.score).collect();
    let weights = softmax_f32(&scores);
    let dim = hits[0].value.len();
    let mut out = vec![0.0f32; dim];
    for (weight, hit) in weights.iter().zip(hits) {
        for (dst, value) in out.iter_mut().zip(&hit.value) {
            *dst += *weight * *value;
        }
    }
    out
}

fn output_projection(vocab_size: usize, dim: usize, seed: u64) -> Vec<Vec<f32>> {
    (0..vocab_size)
        .map(|token| {
            (0..dim)
                .map(|i| {
                    let x = (token as f32 + 1.0) * 0.013
                        + (i as f32 + 1.0) * 0.017
                        + seed as f32 * 0.0001;
                    x.sin() * 0.25 + x.cos() * 0.10
                })
                .collect()
        })
        .collect()
}

fn project_logits(output: &[f32], projection: &[Vec<f32>]) -> Vec<f32> {
    projection.iter().map(|row| dot(output, row)).collect()
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn softmax_f32(scores: &[f32]) -> Vec<f32> {
    let max = scores.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let mut exps: Vec<f32> = scores.iter().map(|v| (*v - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    if sum <= f32::EPSILON || !sum.is_finite() {
        return vec![1.0 / scores.len().max(1) as f32; scores.len()];
    }
    for v in &mut exps {
        *v /= sum;
    }
    exps
}

fn softmax(scores: &[f32]) -> Vec<f64> {
    softmax_f32(scores).into_iter().map(f64::from).collect()
}

fn nll(probs: &[f64], label: usize) -> f64 {
    -probs[label].max(1e-12).ln()
}

fn kl_divergence(p: &[f64], q: &[f64]) -> f64 {
    p.iter()
        .zip(q)
        .map(|(pi, qi)| {
            if *pi <= 0.0 {
                0.0
            } else {
                pi * (pi / qi.max(1e-12)).ln()
            }
        })
        .sum()
}

fn argmax(values: &[f32]) -> usize {
    values
        .iter()
        .enumerate()
        .max_by(|a, b| a.1.total_cmp(b.1))
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn cosine(a: &[f32], b: &[f32]) -> f64 {
    let dot: f64 = a
        .iter()
        .zip(b)
        .map(|(x, y)| f64::from(*x) * f64::from(*y))
        .sum();
    let na: f64 = a
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    let nb: f64 = b
        .iter()
        .map(|x| f64::from(*x) * f64::from(*x))
        .sum::<f64>()
        .sqrt();
    if na <= f64::EPSILON || nb <= f64::EPSILON {
        0.0
    } else {
        dot / (na * nb)
    }
}

fn mse(a: &[f32], b: &[f32]) -> f64 {
    if a.is_empty() {
        return 0.0;
    }
    a.iter()
        .zip(b)
        .map(|(x, y)| {
            let d = f64::from(*x) - f64::from(*y);
            d * d
        })
        .sum::<f64>()
        / a.len() as f64
}

fn mean(values: &[f64]) -> f64 {
    if values.is_empty() {
        0.0
    } else {
        values.iter().sum::<f64>() / values.len() as f64
    }
}
