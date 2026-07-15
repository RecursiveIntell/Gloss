use std::time::Instant;

use crate::codec::{create_codec, CompressedBlock};
use crate::digest_compat::Digest;
use crate::error::{PolyKvError, Result};
use crate::ids_compat::AgentId;
use crate::manifest::ShellManifest;
use crate::policy::{CODEC_FIB_K4_N32, CODEC_TURBO_8BIT};
use crate::pool::SharedKVPool;
use crate::receipt::{now_unix, CompressedAttentionSelectionReceipt, ShellMaterializeReceipt};

/// One layer's worth of agent-unique compressed KV blocks.
#[derive(Debug, Clone)]
pub struct ShellLayer {
    /// Zero-based layer index.
    pub layer_index: u32,
    /// Key blocks unique to this agent (turbo-quant compressed).
    pub key_blocks: Vec<CompressedBlock>,
    /// Value blocks unique to this agent (turbo-quant compressed).
    pub value_blocks: Vec<CompressedBlock>,
    /// Blake3 digest of all blocks in this layer.
    pub block_digest: Digest,
}

/// A per-agent compressed context shell.
///
/// AgentShell stores turbo-quant compressed KV vectors for tokens that are
/// unique to a specific agent. Shared tokens are referenced from the parent
/// pool by digest, not duplicated.
#[derive(Debug, Clone)]
pub struct AgentShell {
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Shell manifest.
    pub shell_manifest: ShellManifest,
    /// Per-layer unique token blocks (turbo-quant compressed).
    pub unique_layers: Vec<ShellLayer>,
    /// Reference to the parent pool digest.
    pub pool_digest: Digest,
    /// Seed used for deterministic codec operations.
    pub build_seed: u64,
}

impl AgentShell {
    /// Compute attention over pool + shell for a given query.
    ///
    /// Decompresses the shared pool keys and the shell's unique keys for
    /// the specified layer, concatenates them, and returns the top-K
    /// token indices with their dot-product scores. Returns a marker
    /// indicating whether each hit came from the pool or the shell.
    #[cfg(feature = "fib")]
    pub fn attention_topk(
        &self,
        pool: &SharedKVPool,
        layer_idx: usize,
        query: &[f32],
        top_k: usize,
        turbo_config: &crate::policy::TurboConfig,
    ) -> Result<Vec<AttentionHit>> {
        let decomposed = pool.decompress_layer(layer_idx)?;
        let head_dim = decomposed.head_dim;
        let pool_keys = decomposed
            .keys
            .first()
            .ok_or_else(|| PolyKvError::Internal("no pool key head".into()))?;
        let num_pool_tokens = pool_keys.len() / head_dim;

        // Decode shell keys for this layer
        let shell_layer = self
            .unique_layers
            .iter()
            .find(|l| l.layer_index == layer_idx as u32)
            .ok_or_else(|| PolyKvError::Internal(format!("no shell layer {layer_idx}")))?;

        let turbo_codec = create_codec(
            crate::policy::CODEC_TURBO_8BIT,
            head_dim,
            None,
            Some(turbo_config),
        )?;

        let mut shell_keys: Vec<f32> = Vec::new();
        for block in &shell_layer.key_blocks {
            let decoded = turbo_codec.decode(&block.encoded_payload, self.build_seed)?;
            shell_keys.extend_from_slice(&decoded);
        }
        let num_shell_tokens = shell_keys.len() / head_dim;

        // Score all tokens (pool + shell)
        let total = num_pool_tokens + num_shell_tokens;
        let mut scored: Vec<(usize, f32, bool)> = Vec::with_capacity(total);
        for i in 0..num_pool_tokens {
            let start = i * head_dim;
            let dot: f32 = query
                .iter()
                .zip(&pool_keys[start..start + head_dim])
                .map(|(a, b)| a * b)
                .sum();
            scored.push((i, dot, false)); // false = pool
        }
        for i in 0..num_shell_tokens {
            let start = i * head_dim;
            let dot: f32 = query
                .iter()
                .zip(&shell_keys[start..start + head_dim])
                .map(|(a, b)| a * b)
                .sum();
            scored.push((i, dot, true)); // true = shell
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k.min(total));

        Ok(scored
            .into_iter()
            .map(|(idx, score, from_shell)| AttentionHit {
                token_index: idx,
                score,
                from_shell,
            })
            .collect())
    }

    /// Compute global top-k attention over the compressed cold pool and this
    /// agent's compressed hot shell without full-layer decode.
    ///
    /// The method scores cold-pool Fib codes and hot-shell Turbo codes in their
    /// compressed forms, selects global top-k candidates, and decodes only the
    /// selected value vectors. The receipt is candidate-selection evidence only;
    /// model-quality claims still require exact attention/logit/PPL replay.
    #[cfg(all(feature = "fib", feature = "turbo"))]
    pub fn attention_topk_compressed(
        &self,
        pool: &SharedKVPool,
        layer_idx: usize,
        head_idx: usize,
        query: &[f32],
        top_k: usize,
    ) -> Result<CompressedShellAttentionSelection> {
        if self.pool_digest != pool.manifest.pool_id {
            return Err(PolyKvError::DigestMismatch {
                expected: self.pool_digest.hex().to_string(),
                got: pool.manifest.pool_id.hex().to_string(),
            });
        }
        if layer_idx >= pool.layers.len() {
            return Err(PolyKvError::LayerIndexOutOfBounds {
                index: layer_idx as u32,
                total: pool.layers.len() as u32,
            });
        }
        let head_dim = pool.manifest.shape.head_dim;
        if query.len() != head_dim {
            return Err(PolyKvError::DimensionMismatch {
                expected: head_dim,
                got: query.len(),
            });
        }
        let num_heads = pool.manifest.shape.num_kv_heads as usize;
        if head_idx >= num_heads {
            return Err(PolyKvError::Internal(format!(
                "head_idx {head_idx} out of range (have {num_heads})"
            )));
        }
        if pool.manifest.shared_codec != CODEC_FIB_K4_N32 {
            return Err(PolyKvError::InvalidPolicy(format!(
                "compressed cold-pool attention requires shared codec {CODEC_FIB_K4_N32}, got {}",
                pool.manifest.shared_codec
            )));
        }

        let pool_layer = &pool.layers[layer_idx];
        if pool_layer.key_blocks.len() != pool_layer.value_blocks.len() {
            return Err(PolyKvError::Internal(format!(
                "layer {layer_idx}: pool key/value block count mismatch ({} vs {})",
                pool_layer.key_blocks.len(),
                pool_layer.value_blocks.len()
            )));
        }
        let pool_tokens = pool.manifest.num_shared_tokens as usize;
        let expected_pool_codes = pool_tokens * num_heads;
        let fib_adapter = crate::codec::FibQuantAdapter::new(
            head_dim,
            pool.manifest.policy.fib_config.k,
            pool.manifest.policy.fib_config.n,
            pool.manifest.policy.fib_config.training_samples,
            pool.manifest.policy.fib_config.lloyd_restarts,
            pool.manifest.policy.fib_config.lloyd_iterations,
        )?;
        let pool_seed = pool.manifest.build_seed;
        let mut pool_key_codes = Vec::with_capacity(expected_pool_codes);
        for block in &pool_layer.key_blocks {
            pool_key_codes
                .extend(fib_adapter.decode_codes_payload(&block.encoded_payload, pool_seed)?);
        }
        let mut pool_value_codes = Vec::with_capacity(expected_pool_codes);
        for block in &pool_layer.value_blocks {
            pool_value_codes
                .extend(fib_adapter.decode_codes_payload(&block.encoded_payload, pool_seed)?);
        }
        if pool_key_codes.len() != expected_pool_codes
            || pool_value_codes.len() != expected_pool_codes
        {
            return Err(PolyKvError::Internal(format!(
                "layer {layer_idx}: decoded {} pool key codes / {} pool value codes, expected {expected_pool_codes}",
                pool_key_codes.len(), pool_value_codes.len()
            )));
        }
        let fib_quantizer = fib_adapter.build_quantizer(pool_seed)?;
        let fib_scorer = fib_quant::FibScorer::new(fib_quantizer).map_err(|e| {
            PolyKvError::Internal(format!("fib compressed scorer construction failed: {e}"))
        })?;
        let fib_prepared = fib_scorer.prepare_query(query).map_err(|e| {
            PolyKvError::Internal(format!("fib compressed query preparation failed: {e}"))
        })?;

        let shell_layer = self
            .unique_layers
            .iter()
            .find(|l| l.layer_index == layer_idx as u32)
            .ok_or_else(|| PolyKvError::Internal(format!("no shell layer {layer_idx}")))?;
        if shell_layer.key_blocks.len() != shell_layer.value_blocks.len() {
            return Err(PolyKvError::Internal(format!(
                "layer {layer_idx}: shell key/value block count mismatch ({} vs {})",
                shell_layer.key_blocks.len(),
                shell_layer.value_blocks.len()
            )));
        }
        let shell_tokens = shell_layer.key_blocks.len() / num_heads;
        if shell_layer.key_blocks.len() != shell_tokens * num_heads {
            return Err(PolyKvError::Internal(format!(
                "layer {layer_idx}: shell block count {} is not divisible by num_heads {num_heads}",
                shell_layer.key_blocks.len()
            )));
        }
        let turbo_quantizer = turbo_quant::TurboQuantizer::new(
            head_dim,
            pool.policy.turbo_config.bits,
            pool.policy.turbo_config.projections,
            self.build_seed,
        )
        .map_err(|e| PolyKvError::Internal(format!("turbo quantizer init failed: {e}")))?;
        let turbo_prepared = turbo_quantizer
            .prepare_query(query)
            .map_err(|e| PolyKvError::Internal(format!("turbo query preparation failed: {e}")))?;
        let turbo_value_codec = create_codec(
            CODEC_TURBO_8BIT,
            head_dim,
            None,
            Some(&pool.policy.turbo_config),
        )?;

        let mut scored: Vec<CompressedShellCandidate> =
            Vec::with_capacity(pool_tokens + shell_tokens);
        for token_idx in 0..pool_tokens {
            let code_idx = token_idx * num_heads + head_idx;
            let score = fib_scorer
                .score_prepared(&fib_prepared, &pool_key_codes[code_idx])
                .map_err(|e| PolyKvError::Internal(format!("fib compressed score failed: {e}")))?;
            scored.push(CompressedShellCandidate {
                token_index: token_idx,
                code_index: code_idx,
                score,
                from_shell: false,
            });
        }
        for token_idx in 0..shell_tokens {
            let code_idx = token_idx * num_heads + head_idx;
            let payload = &shell_layer.key_blocks[code_idx].encoded_payload;
            let code = decode_turbo_code_payload(payload, &turbo_quantizer)?;
            let score = turbo_quantizer
                .inner_product_estimate_prepared(&code, &turbo_prepared)
                .map_err(|e| {
                    PolyKvError::Internal(format!("turbo compressed score failed: {e}"))
                })?;
            scored.push(CompressedShellCandidate {
                token_index: token_idx,
                code_index: code_idx,
                score,
                from_shell: true,
            });
        }

        let selected = top_k.min(scored.len());
        if selected > 0 && selected < scored.len() {
            scored.select_nth_unstable_by(selected - 1, |a, b| {
                b.score
                    .total_cmp(&a.score)
                    .then_with(|| a.from_shell.cmp(&b.from_shell))
                    .then_with(|| a.token_index.cmp(&b.token_index))
            });
            scored.truncate(selected);
        }
        scored.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.from_shell.cmp(&b.from_shell))
                .then_with(|| a.token_index.cmp(&b.token_index))
        });
        let mut hits = Vec::with_capacity(selected);
        let mut selected_pool_count = 0u32;
        let mut selected_shell_count = 0u32;
        for cand in scored.iter().take(selected) {
            let value = if cand.from_shell {
                selected_shell_count += 1;
                turbo_value_codec.decode(
                    &shell_layer.value_blocks[cand.code_index].encoded_payload,
                    self.build_seed,
                )?
            } else {
                selected_pool_count += 1;
                fib_scorer
                    .quantizer()
                    .decode(&pool_value_codes[cand.code_index])
                    .map_err(|e| {
                        PolyKvError::DecompressionFailed(format!(
                            "fib selected pool value decode failed: {e}"
                        ))
                    })?
            };
            if value.len() != head_dim {
                return Err(PolyKvError::DimensionMismatch {
                    expected: head_dim,
                    got: value.len(),
                });
            }
            hits.push(CompressedShellAttentionHit {
                token_index: cand.token_index,
                score: cand.score,
                from_shell: cand.from_shell,
                value,
            });
        }

        let shell_digest = self.shell_manifest.digest()?;
        let receipt = CompressedAttentionSelectionReceipt::new(
            pool.manifest.pool_id.clone(),
            layer_idx as u32,
            head_idx as u32,
            (pool_tokens + shell_tokens) as u32,
            hits.len() as u32,
            (pool_tokens + shell_tokens) as u64,
            hits.len() as u64,
            false,
            "fib_pool_and_turbo_shell_compressed_score_topk_value_decode",
            format!("{}+{}", pool.manifest.shared_codec, CODEC_TURBO_8BIT),
            now_unix(),
        )
        .with_shell_source_counts(
            self.agent_id.to_string(),
            shell_digest,
            pool_tokens as u32,
            shell_tokens as u32,
            selected_pool_count,
            selected_shell_count,
        );
        receipt.validate()?;
        Ok(CompressedShellAttentionSelection { hits, receipt })
    }

    #[cfg(not(all(feature = "fib", feature = "turbo")))]
    pub fn attention_topk_compressed(
        &self,
        pool: &SharedKVPool,
        layer_idx: usize,
        head_idx: usize,
        query: &[f32],
        top_k: usize,
    ) -> Result<CompressedShellAttentionSelection> {
        let _ = (pool, layer_idx, head_idx, query, top_k);
        Err(PolyKvError::CodecUnavailable {
            codec: "fib+turbo".into(),
            feature: "fib,turbo".into(),
        })
    }
}

/// A single attention hit identifying whether the token came from the
/// shared pool or the agent shell.
#[derive(Debug, Clone)]
pub struct AttentionHit {
    /// Token index within its source (pool or shell).
    pub token_index: usize,
    /// Dot-product score with the query.
    pub score: f32,
    /// True if this token is from the agent shell, false if from the pool.
    pub from_shell: bool,
}

/// One selected compressed-attention hit across cold pool + hot shell.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressedShellAttentionHit {
    /// Token index within its source (pool or shell).
    pub token_index: usize,
    /// Compressed-domain approximate score.
    pub score: f32,
    /// True if this token is from the agent shell, false if from the pool.
    pub from_shell: bool,
    /// Decoded value vector for selected candidates only.
    pub value: Vec<f32>,
}

/// Output of compressed-domain top-k selection over cold pool + hot shell.
#[derive(Debug, Clone, PartialEq)]
pub struct CompressedShellAttentionSelection {
    /// Selected hits sorted by descending compressed-domain score.
    pub hits: Vec<CompressedShellAttentionHit>,
    /// Candidate-selection receipt with source counts and claim boundary.
    pub receipt: CompressedAttentionSelectionReceipt,
}

#[derive(Debug, Clone)]
struct CompressedShellCandidate {
    token_index: usize,
    code_index: usize,
    score: f32,
    from_shell: bool,
}

#[cfg(feature = "turbo")]
fn decode_turbo_code_payload(
    payload: &[u8],
    quantizer: &turbo_quant::TurboQuantizer,
) -> Result<turbo_quant::TurboCode> {
    if payload.len() >= 4 && &payload[0..4] == turbo_quant::TURBO_CODE_WIRE_MAGIC {
        turbo_quant::TurboCodeWireV1::decode(payload, quantizer)
            .map_err(|e| PolyKvError::DecompressionFailed(format!("turbo wire decode failed: {e}")))
    } else {
        serde_json::from_slice(payload).map_err(|e| {
            PolyKvError::DecompressionFailed(format!("turbo code deserialize failed: {e}"))
        })
    }
}

impl ShellLayer {
    /// Compute a content digest over the blocks in this layer.
    fn compute_digest(&self) -> Result<Digest> {
        let key_digests: Vec<&str> = self
            .key_blocks
            .iter()
            .map(|b| b.payload_digest.hex())
            .collect();
        let value_digests: Vec<&str> = self
            .value_blocks
            .iter()
            .map(|b| b.payload_digest.hex())
            .collect();
        let payload = serde_json::json!({
            "layer_index": self.layer_index,
            "key_digests": key_digests,
            "value_digests": value_digests,
        });
        crate::digest_compat::compute_json(&payload)
    }
}

/// Materialize an AgentShell from a SharedKVPool and agent-specific tokens.
///
/// Agent tokens that are not found in the shared pool are compressed with
/// turbo-quant and stored in shell layers.
pub fn materialize_shell(
    pool: &SharedKVPool,
    agent_id: &str,
    agent_tokens: &[(String, Vec<f32>)],
    seed: u64,
) -> Result<(AgentShell, ShellMaterializeReceipt)> {
    let start = Instant::now();
    let agent_id = AgentId::new(agent_id);

    if agent_tokens.is_empty() {
        // Empty shell — agent uses only shared pool tokens
        let shell_digest = crate::digest_compat::compute_str(&format!(
            "empty_shell:{}:{}",
            agent_id,
            pool.manifest.pool_id.hex()
        ));

        let shell_manifest = ShellManifest::new(
            agent_id.clone(),
            pool.manifest.pool_id.clone(),
            0,
            pool.manifest.num_layers,
            0,
            seed,
            now_unix(),
        )?;

        let receipt = ShellMaterializeReceipt::new(
            agent_id.clone(),
            pool.manifest.pool_id.clone(),
            shell_digest.clone(),
            0,
            0,
            start.elapsed().as_millis() as u64,
            now_unix(),
        );

        return Ok((
            AgentShell {
                agent_id,
                shell_manifest,
                unique_layers: Vec::new(),
                pool_digest: pool.manifest.pool_id.clone(),
                build_seed: seed,
            },
            receipt,
        ));
    }

    let num_layers = pool.manifest.num_layers as usize;
    let num_kv_heads = pool.manifest.shape.num_kv_heads as usize;
    let head_dim = pool.manifest.shape.head_dim;

    // Create the turbo-quant codec
    let shell_codec = create_codec(
        CODEC_TURBO_8BIT,
        head_dim,
        None,
        Some(&pool.policy.turbo_config),
    )?;

    // Validate agent token shapes
    let expected_len = num_layers * num_kv_heads * head_dim * 2;
    for (token_id, vec) in agent_tokens {
        if vec.len() != expected_len {
            return Err(PolyKvError::DimensionMismatch {
                expected: expected_len,
                got: vec.len(),
            });
        }
        if vec.iter().any(|v| !v.is_finite()) {
            return Err(PolyKvError::CorruptPayload(format!(
                "agent token {} contains non-finite values",
                token_id
            )));
        }
    }

    let mut unique_layers: Vec<ShellLayer> = Vec::with_capacity(num_layers);
    let mut total_shell_bytes: u64 = 0;
    let num_unique_tokens = agent_tokens.len() as u32;

    for layer_idx in 0..num_layers {
        let mut key_blocks: Vec<CompressedBlock> =
            Vec::with_capacity(num_unique_tokens as usize * num_kv_heads);
        let mut value_blocks: Vec<CompressedBlock> =
            Vec::with_capacity(num_unique_tokens as usize * num_kv_heads);

        for (_token_id, vec) in agent_tokens.iter() {
            for head_idx in 0..num_kv_heads {
                let base_offset = layer_idx * num_kv_heads * head_dim * 2 + head_idx * head_dim * 2;

                let key_start = base_offset;
                let key_end = key_start + head_dim;
                let key: Vec<f32> = vec[key_start..key_end].to_vec();

                let value_start = key_end;
                let value_end = value_start + head_dim;
                let value: Vec<f32> = vec[value_start..value_end].to_vec();

                let encoded_key = shell_codec.encode(&key, seed)?;
                let encoded_value = shell_codec.encode(&value, seed)?;

                key_blocks.push(CompressedBlock::new(
                    shell_codec.codec_id(),
                    encoded_key,
                    head_dim,
                ));
                value_blocks.push(CompressedBlock::new(
                    shell_codec.codec_id(),
                    encoded_value,
                    head_dim,
                ));

                total_shell_bytes += key_blocks.last().unwrap().compressed_bytes as u64;
                total_shell_bytes += value_blocks.last().unwrap().compressed_bytes as u64;
            }
        }

        let mut layer = ShellLayer {
            layer_index: layer_idx as u32,
            key_blocks,
            value_blocks,
            block_digest: Digest::from_hex_unchecked(""),
        };
        layer.block_digest = layer.compute_digest()?;
        unique_layers.push(layer);
    }

    let materialize_ms = start.elapsed().as_millis() as u64;
    let materialized_at_unix = now_unix();

    // Compute shell digest
    let layer_digests: Vec<Digest> = unique_layers
        .iter()
        .map(|l| l.block_digest.clone())
        .collect();
    let shell_digest_input = serde_json::json!({
        "agent_id": agent_id,
        "pool_digest": pool.manifest.pool_id.hex(),
        "layer_digests": layer_digests.iter().map(|d| d.hex()).collect::<Vec<_>>(),
        "seed": seed,
    });
    let shell_digest = crate::digest_compat::compute_json(&shell_digest_input)?;

    let shell_manifest = ShellManifest::new(
        agent_id.clone(),
        pool.manifest.pool_id.clone(),
        num_unique_tokens,
        num_layers as u32,
        total_shell_bytes,
        seed,
        materialized_at_unix,
    )?;

    let receipt = ShellMaterializeReceipt::new(
        agent_id.clone(),
        pool.manifest.pool_id.clone(),
        shell_digest.clone(),
        num_unique_tokens,
        total_shell_bytes,
        materialize_ms,
        materialized_at_unix,
    );

    Ok((
        AgentShell {
            agent_id,
            shell_manifest,
            unique_layers,
            pool_digest: pool.manifest.pool_id.clone(),
            build_seed: seed,
        },
        receipt,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::SharedKVPool;
    use crate::shape::{AttentionType, KvTensorShape};

    fn make_test_shape() -> KvTensorShape {
        KvTensorShape {
            attention_type: AttentionType::MHA,
            num_layers: 2,
            num_heads: 4,
            num_kv_heads: 4,
            head_dim: 8,
            hidden_size: 32,
        }
    }

    fn make_test_corpus(n: usize) -> Vec<(String, Vec<f32>)> {
        use rand::Rng;
        use rand_chacha::{rand_core::SeedableRng, ChaCha8Rng};
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let shape = make_test_shape();
        let vec_len = shape.num_layers as usize * shape.num_kv_heads as usize * shape.head_dim * 2;

        (0..n)
            .map(|i| {
                let vec: Vec<f32> = (0..vec_len).map(|_| rng.gen_range(-1.0..1.0)).collect();
                (format!("token_{}", i), vec)
            })
            .collect()
    }

    #[test]
    fn test_shell_materialize_empty() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(4);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        let agent_tokens: Vec<(String, Vec<f32>)> = vec![];
        let (shell, mat_receipt) = materialize_shell(&pool, "agent_1", &agent_tokens, 42).unwrap();

        assert_eq!(shell.agent_id, AgentId::new("agent_1"));
        assert_eq!(mat_receipt.num_unique_tokens, 0);
        assert_eq!(mat_receipt.shell_size_bytes, 0);
    }

    #[test]
    fn test_shell_materialize_basic() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(4);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        let agent_tokens = make_test_corpus(2);
        let (shell, mat_receipt) = materialize_shell(&pool, "agent_x", &agent_tokens, 42).unwrap();

        assert_eq!(shell.agent_id, AgentId::new("agent_x"));
        assert_eq!(shell.unique_layers.len(), 2);
        assert_eq!(mat_receipt.num_unique_tokens, 2);
        assert!(mat_receipt.shell_size_bytes > 0);
        assert_eq!(shell.pool_digest, pool.manifest.pool_id);
    }

    #[test]
    fn test_shell_materialize_deterministic() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(4);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let agent_tokens = make_test_corpus(2);

        let (shell1, receipt1) = materialize_shell(&pool, "agent_x", &agent_tokens, 42).unwrap();
        let (shell2, receipt2) = materialize_shell(&pool, "agent_x", &agent_tokens, 42).unwrap();

        assert_eq!(receipt1.shell_digest, receipt2.shell_digest);
        assert_eq!(
            shell1.unique_layers[0].block_digest,
            shell2.unique_layers[0].block_digest
        );
    }

    #[test]
    fn test_shell_compressed_attention_topk_scores_pool_and_shell_without_full_decode() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(16);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let agent_tokens = make_test_corpus(3);
        let (shell, _mat_receipt) = materialize_shell(&pool, "agent_x", &agent_tokens, 42).unwrap();
        let query: Vec<f32> = (0..shape.head_dim).map(|x| x as f32 * 0.125).collect();

        let out = shell
            .attention_topk_compressed(&pool, 0, 0, &query, 5)
            .expect("shell compressed attention selection should work");

        assert_eq!(out.hits.len(), 5);
        assert!(out.hits.iter().any(|hit| !hit.from_shell));
        assert!(out.hits.iter().all(|hit| hit.value.len() == shape.head_dim));
        assert_eq!(out.receipt.candidate_count, 19);
        assert_eq!(out.receipt.pool_candidate_count, 16);
        assert_eq!(out.receipt.shell_candidate_count, 3);
        assert_eq!(out.receipt.selected_count, 5);
        assert_eq!(
            out.receipt.selected_pool_count + out.receipt.selected_shell_count,
            5
        );
        assert_eq!(out.receipt.decoded_value_vectors, 5);
        assert!(!out.receipt.full_layer_decoded);
        assert!(out.receipt.exact_fallback_required);
        assert_eq!(out.receipt.agent_id.as_deref(), Some("agent_x"));
        assert!(out.receipt.shell_digest.is_some());
        assert_eq!(
            out.receipt.scoring_path,
            "fib_pool_and_turbo_shell_compressed_score_topk_value_decode"
        );
        assert!(out
            .receipt
            .claim_boundary
            .contains("compressed candidate selection only"));
        for window in out.hits.windows(2) {
            assert!(window[0].score >= window[1].score);
        }
    }

    #[test]
    fn test_shell_compressed_attention_rejects_wrong_query_dimension() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(8);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let agent_tokens = make_test_corpus(1);
        let (shell, _mat_receipt) = materialize_shell(&pool, "agent_x", &agent_tokens, 42).unwrap();

        let err = shell
            .attention_topk_compressed(&pool, 0, 0, &[1.0, 2.0], 3)
            .expect_err("wrong query dimension must fail");

        assert!(matches!(
            err,
            PolyKvError::DimensionMismatch {
                expected: 8,
                got: 2
            }
        ));
    }
}
