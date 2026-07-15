use std::time::Instant;

use crate::codec::{create_codec, CompressedBlock};
use crate::digest_compat::Digest;
use crate::error::{PolyKvError, Result};
use crate::manifest::PoolManifest;
use crate::policy::{CompressionPolicy, CODEC_FIB_K4_N32};
use crate::receipt::{now_unix, CompressedAttentionSelectionReceipt, PoolBuildReceipt};
use crate::shape::KvTensorShape;

/// One layer's worth of compressed KV blocks in the shared pool.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PoolLayer {
    /// Zero-based layer index.
    pub layer_index: u32,
    /// Key blocks — one per token, fib-quant compressed.
    pub key_blocks: Vec<CompressedBlock>,
    /// Value blocks — one per token, fib-quant compressed.
    pub value_blocks: Vec<CompressedBlock>,
    /// Blake3 digest of all blocks in this layer (canonical JSON).
    pub block_digest: Digest,
}

impl PoolLayer {
    /// Compute a content digest over the blocks in this layer.
    fn compute_digest(&self) -> Result<Digest> {
        // Serialize key + value payloads to compute a deterministic digest
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

/// One selected compressed-attention hit from the shared cold pool.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompressedAttentionHit {
    /// Token index within the shared pool.
    pub token_index: usize,
    /// Compressed-domain key score for the query.
    pub score: f32,
    /// Decoded value vector for the selected token/head only.
    pub value: Vec<f32>,
}

/// Output of compressed-domain top-k attention selection over the cold pool.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CompressedAttentionSelection {
    /// Selected hits sorted by descending compressed-domain score.
    pub hits: Vec<CompressedAttentionHit>,
    /// Receipt proving candidate scoring did not fully decode the layer.
    pub receipt: CompressedAttentionSelectionReceipt,
}

/// A shared, compressed KV cache pool.
///
/// The pool holds fib-quant compressed KV blocks for tokens shared across
/// agents. It is immutable after construction. Agent shells can be materialized
/// from this pool by adding agent-specific tokens compressed with turbo-quant.
#[derive(Debug, Clone)]
pub struct SharedKVPool {
    /// Pool manifest with shape, policy, timestamps.
    pub manifest: PoolManifest,
    /// One PoolLayer per transformer layer.
    pub layers: Vec<PoolLayer>,
    /// The compression policy used.
    pub policy: CompressionPolicy,
}

impl SharedKVPool {
    /// Build a shared KV pool from a corpus of token vectors.
    ///
    /// # Arguments
    /// * `corpus` - List of (token_id, kv_vector) pairs. Each kv_vector must be
    ///   the concatenated keys and values for all layers and heads: `[layer0_head0_key,
    ///   layer0_head0_value, layer0_head1_key, ...]`.
    /// * `shape` - The tensor shape describing the model architecture.
    /// * `seed` - Deterministic seed for codec operations.
    ///
    /// # Returns
    /// The built SharedKVPool and a PoolBuildReceipt.
    pub fn build(
        corpus: &[(String, Vec<f32>)],
        shape: &KvTensorShape,
        seed: u64,
    ) -> Result<(Self, PoolBuildReceipt)> {
        let start = Instant::now();

        if corpus.is_empty() {
            return Err(PolyKvError::EmptyCorpus);
        }

        shape.validate()?;
        let policy = CompressionPolicy::default_two_tier();
        policy.validate()?;

        let num_tokens = corpus.len();
        let num_layers = shape.num_layers as usize;
        let num_kv_heads = shape.num_kv_heads as usize;
        let head_dim = shape.head_dim;

        // Validate that each corpus vector has the correct length
        let expected_len = num_layers * num_kv_heads * head_dim * 2; // key + value per head per layer
        for (token_id, vec) in corpus {
            if vec.len() != expected_len {
                return Err(PolyKvError::DimensionMismatch {
                    expected: expected_len,
                    got: vec.len(),
                });
            }
            if vec.iter().any(|v| !v.is_finite()) {
                return Err(PolyKvError::CorruptPayload(format!(
                    "token {} contains non-finite values",
                    token_id
                )));
            }
        }

        // Create the fib-quant codec for shared pool compression.
        // We compress per-head key and value vectors in batched calls. The
        // fib-quant adapter's encode_batch dispatches to the GPU backend when
        // the batch is large enough (>= 16 vectors) and dim is large enough
        // (>= 64), which is always true at the (layer, head) granularity for
        // a corpus of more than ~4 tokens.
        let codec = create_codec(CODEC_FIB_K4_N32, head_dim, Some(&policy.fib_config), None)?;

        let mut layers: Vec<PoolLayer> = Vec::with_capacity(num_layers);
        let mut total_compressed_bytes: u64 = 0;

        // Build a closure that builds one layer. Each layer is independent
        // (different head/data ranges in the corpus), so we can dispatch
        // them in parallel via Rayon when the feature is enabled.
        let build_layer = |layer_idx: usize| -> Result<(PoolLayer, u64)> {
            // Collect every (token, head) key vector and every (token, head)
            // value vector for this layer up front, then dispatch two batched
            // encode calls (one for keys, one for values). This is what lets
            // fib-quant reach its GPU batch threshold.
            let mut key_inputs: Vec<Vec<f32>> = Vec::with_capacity(num_tokens * num_kv_heads);
            let mut value_inputs: Vec<Vec<f32>> = Vec::with_capacity(num_tokens * num_kv_heads);
            for (_token_id, vec) in corpus.iter() {
                for head_idx in 0..num_kv_heads {
                    let base_offset =
                        layer_idx * num_kv_heads * head_dim * 2 + head_idx * head_dim * 2;
                    let key_end = base_offset + head_dim;
                    let value_end = key_end + head_dim;
                    key_inputs.push(vec[base_offset..key_end].to_vec());
                    value_inputs.push(vec[key_end..value_end].to_vec());
                }
            }

            let key_refs: Vec<&[f32]> = key_inputs.iter().map(|v| v.as_slice()).collect();
            let value_refs: Vec<&[f32]> = value_inputs.iter().map(|v| v.as_slice()).collect();

            let mut key_blocks: Vec<CompressedBlock>;
            let mut value_blocks: Vec<CompressedBlock>;
            if let (Some(key_payload), Some(value_payload)) = (
                codec.encode_batch_compact(&key_refs, seed)?,
                codec.encode_batch_compact(&value_refs, seed)?,
            ) {
                key_blocks = vec![CompressedBlock::new(
                    codec.codec_id(),
                    key_payload,
                    head_dim,
                )];
                value_blocks = vec![CompressedBlock::new(
                    codec.codec_id(),
                    value_payload,
                    head_dim,
                )];
            } else {
                let encoded_keys = codec.encode_batch(&key_refs, seed)?;
                let encoded_values = codec.encode_batch(&value_refs, seed)?;

                if encoded_keys.len() != num_tokens * num_kv_heads
                    || encoded_values.len() != num_tokens * num_kv_heads
                {
                    return Err(PolyKvError::Internal(format!(
                        "encode_batch returned {} keys / {} values, expected {} (layer {})",
                        encoded_keys.len(),
                        encoded_values.len(),
                        num_tokens * num_kv_heads,
                        layer_idx
                    )));
                }

                key_blocks = Vec::with_capacity(num_tokens * num_kv_heads);
                value_blocks = Vec::with_capacity(num_tokens * num_kv_heads);
                for (k_payload, v_payload) in encoded_keys.into_iter().zip(encoded_values) {
                    key_blocks.push(CompressedBlock::new(codec.codec_id(), k_payload, head_dim));
                    value_blocks.push(CompressedBlock::new(codec.codec_id(), v_payload, head_dim));
                }
            }
            let layer_bytes: u64 = key_blocks
                .iter()
                .map(|b| b.compressed_bytes as u64)
                .sum::<u64>()
                + value_blocks
                    .iter()
                    .map(|b| b.compressed_bytes as u64)
                    .sum::<u64>();

            let mut layer = PoolLayer {
                layer_index: layer_idx as u32,
                key_blocks,
                value_blocks,
                block_digest: Digest::from_hex_unchecked(""),
            };
            layer.block_digest = layer.compute_digest()?;
            Ok((layer, layer_bytes))
        };

        // Layer build: serial or parallel. Both paths preserve layer order
        // in the output (we collect by index, not by completion time).
        let layer_results: Vec<Result<(PoolLayer, u64)>> = {
            #[cfg(feature = "parallel_pool")]
            {
                use rayon::prelude::*;
                (0..num_layers).into_par_iter().map(build_layer).collect()
            }
            #[cfg(not(feature = "parallel_pool"))]
            {
                (0..num_layers).map(build_layer).collect()
            }
        };
        for r in layer_results {
            let (layer, layer_bytes) = r?;
            total_compressed_bytes += layer_bytes;
            layers.push(layer);
        }

        let raw_size_bytes = shape.total_kv_bytes(num_tokens) as u64;
        let fib_build_ms = start.elapsed().as_millis() as u64;
        let built_at_unix = now_unix();

        // Compute pool digest
        let layer_digests: Vec<Digest> = layers.iter().map(|l| l.block_digest.clone()).collect();
        let pool_id = crate::digest_compat::compute_json(&layer_digests)?;

        let manifest = PoolManifest::new(
            pool_id.clone(),
            shape.clone(),
            policy.clone(),
            num_tokens as u32,
            shape.num_layers,
            total_compressed_bytes,
            raw_size_bytes,
            seed,
            built_at_unix,
        )?;

        // Honest backend label: ask the codec whether the per-(layer,head)
        // batch we'd actually dispatch crossed the GPU threshold. The fib-quant
        // encoder only goes to GPU when batch size and dim clear the runtime
        // minimums; a 4-doc, 12-head corpus is GPU, but a 4-doc, 4-head
        // corpus (16 vectors exactly) is right at the edge and still GPU,
        // while a 2-doc corpus falls through to CPU even with --features gpu.
        let batch_n = num_tokens * num_kv_heads;
        let backend = if codec.is_gpu_accelerated_for(batch_n, head_dim) {
            "gpu"
        } else {
            "cpu"
        };
        let codebook_digest = codec
            .codebook_digest(seed)
            .map(Digest::from_hex_unchecked)
            .unwrap_or_else(|| Digest::from_hex_unchecked(""));
        let rotation_digest = codec
            .rotation_digest(seed)
            .map(Digest::from_hex_unchecked)
            .unwrap_or_else(|| Digest::from_hex_unchecked(""));
        let receipt = PoolBuildReceipt::new(
            pool_id,
            layer_digests,
            codebook_digest,
            rotation_digest,
            num_tokens as u32,
            fib_build_ms,
            total_compressed_bytes,
            raw_size_bytes,
            policy.clone(),
            seed,
            built_at_unix,
        )
        .with_backend(backend);

        Ok((
            Self {
                manifest,
                layers,
                policy,
            },
            receipt,
        ))
    }
    /// Materialize an agent shell from this pool.
    ///
    /// Agent-specific tokens (not in the shared corpus) are compressed with
    /// turbo-quant and appended as shell layers. Tokens already in the pool
    /// are referenced by digest only.
    ///
    /// # Arguments
    /// * `agent_id` - Identifier for this agent.
    /// * `agent_tokens` - Token vectors specific to this agent.
    /// * `seed` - Deterministic seed for turbo-quant operations.
    ///
    /// # Returns
    /// An AgentShell and a ShellMaterializeReceipt.
    pub fn materialize_shell(
        &self,
        agent_id: &str,
        agent_tokens: &[(String, Vec<f32>)],
        seed: u64,
    ) -> Result<(
        crate::shell::AgentShell,
        crate::receipt::ShellMaterializeReceipt,
    )> {
        crate::shell::materialize_shell(self, agent_id, agent_tokens, seed)
    }

    /// Inject a shell into a KV cache.
    ///
    /// The injection receipt traces every block from its source (pool or shell)
    /// to its target position in the cache.
    pub fn inject_into_cache(
        _shell: &crate::shell::AgentShell,
        _base_cache: &mut dyn CacheTarget,
    ) -> Result<crate::receipt::InjectionReceipt> {
        // The CacheTarget trait allows injection without knowing the concrete cache type.
        // This is a generic injection path — concrete adapters (e.g., for HF DynamicCache)
        // live in downstream crates.
        Err(PolyKvError::Internal(
            "inject_into_cache requires a concrete cache adapter; use inject_into_cache_with_adaptor"
                .into(),
        ))
    }

    /// Decompress all shared-pool blocks for a single layer, returning the
    /// reconstructed K and V tensors in the original model layout.
    ///
    /// Output shape: `keys[head_idx]` is a flat `Vec<f32>` of length
    /// `num_tokens * head_dim` containing all tokens' K vectors for that
    /// head, in token order. Same for `values`. Lossy (fib-quant) but
    /// reproducible: same corpus + same seed + same codec yields the same
    /// reconstructed floats.
    ///
    /// This is the inverse of `build` and the symmetric counterpart of
    /// `materialize_shell`'s per-agent shell decompression. It's the
    /// path HuggingFace `DynamicCache.update()` and similar KV-cache
    /// integrations use to populate a fresh cache from the pool.
    pub fn decompress_layer(&self, layer_idx: usize) -> Result<DecompressedLayer> {
        if layer_idx >= self.layers.len() {
            return Err(PolyKvError::Internal(format!(
                "decompress_layer: layer_idx {layer_idx} out of range (have {})",
                self.layers.len()
            )));
        }
        let layer = &self.layers[layer_idx];
        let head_dim = self.manifest.shape.head_dim;
        let num_heads = self.manifest.shape.num_kv_heads as usize;
        let num_tokens = if layer.key_blocks.len() == 1 && layer.value_blocks.len() == 1 {
            self.manifest.num_shared_tokens as usize
        } else {
            layer.key_blocks.len() / num_heads
        };
        if layer.value_blocks.len() != layer.key_blocks.len() {
            return Err(PolyKvError::Internal(format!(
                "layer {}: key/value block count mismatch ({} vs {})",
                layer_idx,
                layer.key_blocks.len(),
                layer.value_blocks.len()
            )));
        }
        if layer.key_blocks.len() != num_tokens * num_heads && layer.key_blocks.len() != 1 {
            return Err(PolyKvError::Internal(format!(
                "layer {}: block count {} != num_tokens * num_heads {}",
                layer_idx,
                layer.key_blocks.len(),
                num_tokens * num_heads
            )));
        }
        // All shared-pool blocks use the same codec (manifest.shared_codec).
        // Build a single codec and reuse for the whole layer.
        let shared_codec: crate::policy::CodecId = self.manifest.shared_codec.clone();
        let codec = create_codec(
            &shared_codec,
            head_dim,
            Some(&self.manifest.policy.fib_config),
            Some(&self.manifest.policy.turbo_config),
        )?;
        let seed = self.manifest.build_seed;
        // Block ordering: [token_0_head_0, token_0_head_1, ..., token_0_head_{H-1},
        //                  token_1_head_0, ..., token_{T-1}_head_{H-1}]
        // i.e. flat index = token_idx * num_heads + head_idx.
        // Per-head output: keys[head_idx] = concatenation of every token's K for that head.
        let mut keys_per_head: Vec<Vec<f32>> =
            vec![Vec::with_capacity(num_tokens * head_dim); num_heads];
        let mut values_per_head: Vec<Vec<f32>> =
            vec![Vec::with_capacity(num_tokens * head_dim); num_heads];
        if layer.key_blocks.len() == 1 && layer.value_blocks.len() == 1 {
            if let (Some(decoded_keys), Some(decoded_values)) = (
                codec.decode_batch_compact(&layer.key_blocks[0].encoded_payload, seed)?,
                codec.decode_batch_compact(&layer.value_blocks[0].encoded_payload, seed)?,
            ) {
                if decoded_keys.len() != num_tokens * num_heads
                    || decoded_values.len() != num_tokens * num_heads
                {
                    return Err(PolyKvError::Internal(format!(
                        "FB2 decoded {} keys / {} values, expected {} (layer {})",
                        decoded_keys.len(),
                        decoded_values.len(),
                        num_tokens * num_heads,
                        layer_idx
                    )));
                }
                for token_idx in 0..num_tokens {
                    for head_idx in 0..num_heads {
                        let block_idx = token_idx * num_heads + head_idx;
                        let k_decoded = &decoded_keys[block_idx];
                        let v_decoded = &decoded_values[block_idx];
                        if k_decoded.len() != head_dim || v_decoded.len() != head_dim {
                            return Err(PolyKvError::Internal(format!(
                                "FB2 decoded vector length mismatch (layer {}, token {}, head {})",
                                layer_idx, token_idx, head_idx
                            )));
                        }
                        keys_per_head[head_idx].extend_from_slice(k_decoded);
                        values_per_head[head_idx].extend_from_slice(v_decoded);
                    }
                }
            } else {
                if num_tokens != 1 || num_heads != 1 {
                    return Err(PolyKvError::Internal(format!(
                        "single non-compact block cannot decode shape tokens={} heads={} (layer {})",
                        num_tokens, num_heads, layer_idx
                    )));
                }
                let k_decoded = codec.decode(&layer.key_blocks[0].encoded_payload, seed)?;
                let v_decoded = codec.decode(&layer.value_blocks[0].encoded_payload, seed)?;
                keys_per_head[0].extend_from_slice(&k_decoded);
                values_per_head[0].extend_from_slice(&v_decoded);
            }
        } else {
            for token_idx in 0..num_tokens {
                for head_idx in 0..num_heads {
                    let block_idx = token_idx * num_heads + head_idx;
                    let k_payload = &layer.key_blocks[block_idx].encoded_payload;
                    let v_payload = &layer.value_blocks[block_idx].encoded_payload;
                    let k_decoded = codec.decode(k_payload, seed)?;
                    let v_decoded = codec.decode(v_payload, seed)?;
                    if k_decoded.len() != head_dim {
                        return Err(PolyKvError::Internal(format!(
                            "decoded key length {} != head_dim {} (layer {}, token {}, head {})",
                            k_decoded.len(),
                            head_dim,
                            layer_idx,
                            token_idx,
                            head_idx
                        )));
                    }
                    keys_per_head[head_idx].extend_from_slice(&k_decoded);
                    values_per_head[head_idx].extend_from_slice(&v_decoded);
                }
            }
        }
        Ok(DecompressedLayer {
            layer_index: layer_idx as u32,
            num_tokens,
            num_heads,
            head_dim,
            keys: keys_per_head,
            values: values_per_head,
        })
    }

    /// Query the compressed shared cold pool without fully decoding the layer.
    ///
    /// This scores compressed Fib codes for one layer/head, selects the top-k
    /// tokens, then decodes only the selected value vectors. It is the ProveKV
    /// cold-pool read path: compressed candidate scoring first, bounded value
    /// decode second, and a receipt proving no full-layer decode occurred.
    pub fn attention_topk_compressed(
        &self,
        layer_idx: usize,
        head_idx: usize,
        query: &[f32],
        top_k: usize,
    ) -> Result<CompressedAttentionSelection> {
        #[cfg(not(feature = "fib"))]
        {
            let _ = (layer_idx, head_idx, query, top_k);
            return Err(PolyKvError::CodecUnavailable {
                codec: CODEC_FIB_K4_N32.into(),
                feature: "fib".into(),
            });
        }

        #[cfg(feature = "fib")]
        {
            if layer_idx >= self.layers.len() {
                return Err(PolyKvError::LayerIndexOutOfBounds {
                    index: layer_idx as u32,
                    total: self.layers.len() as u32,
                });
            }
            let head_dim = self.manifest.shape.head_dim;
            if query.len() != head_dim {
                return Err(PolyKvError::DimensionMismatch {
                    expected: head_dim,
                    got: query.len(),
                });
            }
            if head_idx >= self.manifest.shape.num_kv_heads as usize {
                return Err(PolyKvError::Internal(format!(
                    "head_idx {head_idx} out of range (have {})",
                    self.manifest.shape.num_kv_heads
                )));
            }
            if self.manifest.shared_codec != CODEC_FIB_K4_N32 {
                return Err(PolyKvError::InvalidPolicy(format!(
                    "compressed cold-pool attention requires shared codec {CODEC_FIB_K4_N32}, got {}",
                    self.manifest.shared_codec
                )));
            }

            let layer = &self.layers[layer_idx];
            if layer.key_blocks.len() != layer.value_blocks.len() {
                return Err(PolyKvError::Internal(format!(
                    "layer {layer_idx}: key/value block count mismatch ({} vs {})",
                    layer.key_blocks.len(),
                    layer.value_blocks.len()
                )));
            }
            let num_heads = self.manifest.shape.num_kv_heads as usize;
            let num_tokens = self.manifest.num_shared_tokens as usize;
            let expected_codes = num_tokens * num_heads;
            let adapter = crate::codec::FibQuantAdapter::new(
                head_dim,
                self.manifest.policy.fib_config.k,
                self.manifest.policy.fib_config.n,
                self.manifest.policy.fib_config.training_samples,
                self.manifest.policy.fib_config.lloyd_restarts,
                self.manifest.policy.fib_config.lloyd_iterations,
            )?;
            let seed = self.manifest.build_seed;
            let mut key_codes = Vec::with_capacity(expected_codes);
            for block in &layer.key_blocks {
                key_codes.extend(adapter.decode_codes_payload(&block.encoded_payload, seed)?);
            }
            let mut value_codes = Vec::with_capacity(expected_codes);
            for block in &layer.value_blocks {
                value_codes.extend(adapter.decode_codes_payload(&block.encoded_payload, seed)?);
            }
            if key_codes.len() != expected_codes || value_codes.len() != expected_codes {
                return Err(PolyKvError::Internal(format!(
                    "layer {layer_idx}: decoded {} key codes / {} value codes, expected {expected_codes}",
                    key_codes.len(),
                    value_codes.len()
                )));
            }

            let quantizer = adapter.build_quantizer(seed)?;
            let scorer = fib_quant::FibScorer::new(quantizer).map_err(|e| {
                PolyKvError::Internal(format!("fib compressed scorer construction failed: {e}"))
            })?;
            let prepared = scorer.prepare_query(query).map_err(|e| {
                PolyKvError::Internal(format!("fib compressed query preparation failed: {e}"))
            })?;
            let mut scored = Vec::with_capacity(num_tokens);
            for token_idx in 0..num_tokens {
                let code_idx = token_idx * num_heads + head_idx;
                let score = scorer
                    .score_prepared(&prepared, &key_codes[code_idx])
                    .map_err(|e| {
                        PolyKvError::Internal(format!("fib compressed score failed: {e}"))
                    })?;
                scored.push((token_idx, code_idx, score));
            }
            let selected = top_k.min(scored.len());
            if selected > 0 && selected < scored.len() {
                scored.select_nth_unstable_by(selected - 1, |a, b| {
                    b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0))
                });
                scored.truncate(selected);
            }
            scored.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
            let mut hits = Vec::with_capacity(selected);
            for &(token_index, code_idx, score) in scored.iter().take(selected) {
                let value = scorer
                    .quantizer()
                    .decode(&value_codes[code_idx])
                    .map_err(|e| {
                        PolyKvError::DecompressionFailed(format!(
                            "fib selected value decode failed: {e}"
                        ))
                    })?;
                if value.len() != head_dim {
                    return Err(PolyKvError::DimensionMismatch {
                        expected: head_dim,
                        got: value.len(),
                    });
                }
                hits.push(CompressedAttentionHit {
                    token_index,
                    score,
                    value,
                });
            }
            let receipt = CompressedAttentionSelectionReceipt::new(
                self.manifest.pool_id.clone(),
                layer_idx as u32,
                head_idx as u32,
                num_tokens as u32,
                hits.len() as u32,
                num_tokens as u64,
                hits.len() as u64,
                false,
                "fib_cold_pool_compressed_score_topk_value_decode",
                self.manifest.shared_codec.clone(),
                now_unix(),
            );
            receipt.validate()?;
            Ok(CompressedAttentionSelection { hits, receipt })
        }
    }

    // ---------- pool search ----------

    /// Build a prepared compressed index for one layer/head.
    ///
    /// This decodes key/value codes and builds the FibScorer once, so that
    /// subsequent `attention_topk_compressed_prepared` calls only need to
    /// prepare the query and score candidates without rebuilding codec state.
    #[cfg(feature = "fib")]
    pub fn prepare_compressed_index(
        &self,
        layer_idx: usize,
        head_idx: usize,
    ) -> Result<PreparedCompressedIndex> {
        if layer_idx >= self.layers.len() {
            return Err(PolyKvError::LayerIndexOutOfBounds {
                index: layer_idx as u32,
                total: self.layers.len() as u32,
            });
        }
        let head_dim = self.manifest.shape.head_dim;
        let num_heads = self.manifest.shape.num_kv_heads as usize;
        if head_idx >= num_heads {
            return Err(PolyKvError::Internal(format!(
                "head_idx {head_idx} out of range (have {num_heads})"
            )));
        }
        if self.manifest.shared_codec != CODEC_FIB_K4_N32 {
            return Err(PolyKvError::InvalidPolicy(format!(
                "compressed cold-pool attention requires shared codec {CODEC_FIB_K4_N32}, got {}",
                self.manifest.shared_codec
            )));
        }
        let layer = &self.layers[layer_idx];
        if layer.key_blocks.len() != layer.value_blocks.len() {
            return Err(PolyKvError::Internal(format!(
                "layer {layer_idx}: key/value block count mismatch ({} vs {})",
                layer.key_blocks.len(),
                layer.value_blocks.len()
            )));
        }
        let num_tokens = self.manifest.num_shared_tokens as usize;
        let expected_codes = num_tokens * num_heads;
        let adapter = crate::codec::FibQuantAdapter::new(
            head_dim,
            self.manifest.policy.fib_config.k,
            self.manifest.policy.fib_config.n,
            self.manifest.policy.fib_config.training_samples,
            self.manifest.policy.fib_config.lloyd_restarts,
            self.manifest.policy.fib_config.lloyd_iterations,
        )?;
        let seed = self.manifest.build_seed;
        let mut key_codes = Vec::with_capacity(expected_codes);
        for block in &layer.key_blocks {
            key_codes.extend(adapter.decode_codes_payload(&block.encoded_payload, seed)?);
        }
        let mut value_codes = Vec::with_capacity(expected_codes);
        for block in &layer.value_blocks {
            value_codes.extend(adapter.decode_codes_payload(&block.encoded_payload, seed)?);
        }
        if key_codes.len() != expected_codes || value_codes.len() != expected_codes {
            return Err(PolyKvError::Internal(format!(
                "layer {layer_idx}: decoded {} key codes / {} value codes, expected {expected_codes}",
                key_codes.len(),
                value_codes.len()
            )));
        }
        let quantizer = adapter.build_quantizer(seed)?;
        let scorer = fib_quant::FibScorer::new(quantizer).map_err(|e| {
            PolyKvError::Internal(format!("fib compressed scorer construction failed: {e}"))
        })?;
        Ok(PreparedCompressedIndex {
            layer_idx,
            head_idx,
            head_dim,
            num_tokens,
            num_heads,
            key_codes,
            value_codes,
            scorer,
        })
    }

    /// Compressed top-k attention using a pre-built index.
    ///
    /// This avoids rebuilding the codec adapter, decoding codes, and
    /// constructing the scorer on every call. Only the query is prepared
    /// per call (O(dim)), then candidates are scored (O(num_tokens)).
    #[cfg(feature = "fib")]
    pub fn attention_topk_compressed_prepared(
        &self,
        index: &PreparedCompressedIndex,
        query: &[f32],
        top_k: usize,
    ) -> Result<CompressedAttentionSelection> {
        if query.len() != index.head_dim {
            return Err(PolyKvError::DimensionMismatch {
                expected: index.head_dim,
                got: query.len(),
            });
        }
        let head_idx = index.head_idx;
        let num_heads = index.num_heads;
        let num_tokens = index.num_tokens;
        let prepared = index.scorer.prepare_query(query).map_err(|e| {
            PolyKvError::Internal(format!("fib compressed query preparation failed: {e}"))
        })?;
        let mut scored: Vec<(usize, usize, f32)> = Vec::with_capacity(num_tokens);
        for token_idx in 0..num_tokens {
            let code_idx = token_idx * num_heads + head_idx;
            let score = index
                .scorer
                .score_prepared(&prepared, &index.key_codes[code_idx])
                .map_err(|e| PolyKvError::Internal(format!("fib compressed score failed: {e}")))?;
            scored.push((token_idx, code_idx, score));
        }
        let selected = top_k.min(scored.len());
        if selected > 0 && selected < scored.len() {
            scored.select_nth_unstable_by(selected - 1, |a, b| {
                b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0))
            });
            scored.truncate(selected);
        }
        scored.sort_by(|a, b| b.2.total_cmp(&a.2).then_with(|| a.0.cmp(&b.0)));
        let mut hits = Vec::with_capacity(selected);
        for &(token_index, code_idx, score) in scored.iter().take(selected) {
            let value = index
                .scorer
                .quantizer()
                .decode(&index.value_codes[code_idx])
                .map_err(|e| {
                    PolyKvError::DecompressionFailed(format!(
                        "fib selected value decode failed: {e}"
                    ))
                })?;
            if value.len() != index.head_dim {
                return Err(PolyKvError::DimensionMismatch {
                    expected: index.head_dim,
                    got: value.len(),
                });
            }
            hits.push(CompressedAttentionHit {
                token_index,
                score,
                value,
            });
        }
        let receipt = CompressedAttentionSelectionReceipt::new(
            self.manifest.pool_id.clone(),
            index.layer_idx as u32,
            head_idx as u32,
            num_tokens as u32,
            hits.len() as u32,
            num_tokens as u64,
            hits.len() as u64,
            false,
            "fib_cold_pool_prepared_compressed_score_topk_value_decode",
            self.manifest.shared_codec.clone(),
            now_unix(),
        );
        receipt.validate()?;
        Ok(CompressedAttentionSelection { hits, receipt })
    }

    /// Build a fully prepared index that pre-unpacks all key indices and norms.
    ///
    /// This eliminates per-call `unpack_indices()` and `decode_stored_norm()`
    /// overhead, making the scoring loop just Gram table lookups.
    #[cfg(feature = "fib")]
    pub fn prepare_fully_compressed_index(
        &self,
        layer_idx: usize,
        head_idx: usize,
    ) -> Result<FullyPreparedCompressedIndex> {
        let prep = self.prepare_compressed_index(layer_idx, head_idx)?;
        let block_count = prep.scorer.quantizer().profile().block_count() as usize;
        let wire_bits = prep.scorer.quantizer().profile().wire_index_bits;
        let num_entries = prep.num_tokens * prep.num_heads;
        let mut key_indices_flat = Vec::with_capacity(num_entries * block_count);
        let mut key_norms = Vec::with_capacity(num_entries);
        for i in 0..num_entries {
            let indices = fib_quant::bitpack::unpack_indices(
                &prep.key_codes[i].indices,
                block_count,
                wire_bits,
            )
            .map_err(|e| PolyKvError::Internal(format!("fib unpack_indices failed: {e}")))?;
            key_indices_flat.extend_from_slice(&indices);
            // Decode stored norm using the public fib-quant API.
            let norm = fib_quant::scoring::decode_stored_norm(
                &prep.key_codes[i],
                prep.scorer.quantizer().profile(),
            )
            .map_err(|e| PolyKvError::Internal(format!("fib decode_stored_norm failed: {e}")))?;
            key_norms.push(norm as f32);
        }
        Ok(FullyPreparedCompressedIndex {
            layer_idx: prep.layer_idx,
            head_idx: prep.head_idx,
            head_dim: prep.head_dim,
            num_tokens: prep.num_tokens,
            num_heads: prep.num_heads,
            key_indices_flat,
            key_norms,
            block_count,
            value_codes: prep.value_codes,
            scorer: prep.scorer,
        })
    }

    /// Compressed top-k attention using a fully prepared index.
    ///
    /// Delegates to `attention_topk_prefetched` which pre-fetches Gram rows
    /// into a contiguous buffer for cache-friendly scoring. This is the
    /// fastest single-head path.
    #[cfg(feature = "fib")]
    pub fn attention_topk_fully_prepared(
        &self,
        index: &FullyPreparedCompressedIndex,
        query: &[f32],
        top_k: usize,
    ) -> Result<CompressedAttentionSelection> {
        self.attention_topk_prefetched(index, query, top_k)
    }

    /// Compressed top-k attention using pre-fetched Gram rows.
    ///
    /// This is the fastest scoring path: query preparation + Gram row
    /// pre-fetch happens once, then the per-token scoring loop is just
    /// sequential gathers from a small contiguous buffer.
    #[cfg(feature = "fib")]
    pub fn attention_topk_prefetched(
        &self,
        index: &FullyPreparedCompressedIndex,
        query: &[f32],
        top_k: usize,
    ) -> Result<CompressedAttentionSelection> {
        let prefetched = index.prepare_gram_rows(query)?;
        let mut scored = index.score_all_tokens(&prefetched)?;

        let selected = top_k.min(scored.len());
        if selected > 0 && selected < scored.len() {
            scored.select_nth_unstable_by(selected - 1, |a, b| {
                b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0))
            });
            scored.truncate(selected);
        }
        scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

        let num_tokens = index.num_tokens;
        let head_idx = index.head_idx;
        let num_heads = index.num_heads;
        let mut hits = Vec::with_capacity(selected);
        for &(token_index, score) in scored.iter().take(selected) {
            let code_idx = token_index * num_heads + head_idx;
            let value = index
                .scorer
                .quantizer()
                .decode(&index.value_codes[code_idx])
                .map_err(|e| {
                    PolyKvError::DecompressionFailed(format!(
                        "fib selected value decode failed: {e}"
                    ))
                })?;
            if value.len() != index.head_dim {
                return Err(PolyKvError::DimensionMismatch {
                    expected: index.head_dim,
                    got: value.len(),
                });
            }
            hits.push(CompressedAttentionHit {
                token_index,
                score,
                value,
            });
        }
        let receipt = CompressedAttentionSelectionReceipt::new(
            self.manifest.pool_id.clone(),
            index.layer_idx as u32,
            head_idx as u32,
            num_tokens as u32,
            hits.len() as u32,
            num_tokens as u64,
            hits.len() as u64,
            false,
            "fib_cold_pool_prefetched_gram_rows_topk_value_decode",
            self.manifest.shared_codec.clone(),
            now_unix(),
        );
        receipt.validate()?;
        Ok(CompressedAttentionSelection { hits, receipt })
    }

    /// Batch multi-head compressed top-k attention using pre-fetched Gram rows.
    ///
    /// Scores all heads in one pass: prepares gram rows for each head's query,
    /// then iterates tokens once, scoring all heads per token. This amortizes
    /// the token loop overhead across heads and improves cache utilization.
    #[cfg(feature = "fib")]
    pub fn attention_topk_batch_heads(
        &self,
        index: &FullyPreparedCompressedIndex,
        queries: &[&[f32]],
        top_k: usize,
    ) -> Result<Vec<CompressedAttentionSelection>> {
        let num_heads = index.num_heads;
        let num_tokens = index.num_tokens;
        let head_dim = index.head_dim;
        if queries.len() != num_heads {
            return Err(PolyKvError::DimensionMismatch {
                expected: num_heads,
                got: queries.len(),
            });
        }

        // Prepare gram rows for each head's query
        let mut all_prefetched: Vec<PrefetchedGramRows> = Vec::with_capacity(num_heads);
        for q in queries.iter() {
            if q.len() != head_dim {
                return Err(PolyKvError::DimensionMismatch {
                    expected: head_dim,
                    got: q.len(),
                });
            }
            // Build a per-head index view by temporarily using prepare_gram_rows
            // with the head_idx set. But prepare_gram_rows uses self.head_idx.
            // We need a different approach: prepare gram rows directly.
            let prepared = index
                .scorer
                .prepare_query(q)
                .map_err(|e| PolyKvError::Internal(format!("fib batch query prep failed: {e}")))?;
            let n = index.scorer.quantizer().profile().codebook_size as usize;
            let block_count = index.scorer.quantizer().profile().block_count() as usize;
            let gram = index.scorer.gram_table();
            let mut gram_rows = vec![0.0f32; block_count * n];
            for (block_idx, &query_idx) in prepared.query_indices.iter().enumerate() {
                let qi = query_idx as usize;
                if qi >= n {
                    return Err(PolyKvError::Internal(format!(
                        "fib batch: query_idx {qi} >= {n}"
                    )));
                }
                let src = &gram.values()[qi * n..(qi + 1) * n];
                gram_rows[block_idx * n..(block_idx + 1) * n].copy_from_slice(src);
            }
            all_prefetched.push(PrefetchedGramRows {
                gram_rows,
                block_count,
                n,
                query_norm: prepared.query_norm,
            });
        }

        // Score all heads for all tokens in one pass
        let n = all_prefetched[0].n;
        let block_count = all_prefetched[0].block_count;
        let mut all_scored: Vec<Vec<(usize, f32)>> =
            vec![vec![(0usize, 0.0f32); num_tokens]; num_heads];

        for token_idx in 0..num_tokens {
            for head_idx in 0..num_heads {
                let code_idx = token_idx * num_heads + head_idx;
                let indices = index.key_block(code_idx);
                let stored_norm = index.key_norms[code_idx];
                let q_norm = all_prefetched[head_idx].query_norm as f32;
                let gram_rows = &all_prefetched[head_idx].gram_rows;

                let mut total = 0.0f32;
                for (block_idx, &stored_idx) in indices.iter().enumerate().take(block_count) {
                    let si = stored_idx as usize;
                    if si >= n {
                        return Err(PolyKvError::Internal(format!(
                            "fib batch: stored_idx {si} >= {n}"
                        )));
                    }
                    total += gram_rows[block_idx * n + si];
                }
                let score = total * q_norm * stored_norm;
                all_scored[head_idx][token_idx] = (token_idx, score);
            }
        }

        // Top-k selection per head
        let mut results = Vec::with_capacity(num_heads);
        for (head_idx, scored) in all_scored.iter_mut().enumerate() {
            let selected = top_k.min(scored.len());
            if selected > 0 && selected < scored.len() {
                scored.select_nth_unstable_by(selected - 1, |a, b| {
                    b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0))
                });
                scored.truncate(selected);
            }
            scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            let mut hits = Vec::with_capacity(selected);
            for &(token_index, score) in scored.iter().take(selected) {
                let code_idx = token_index * num_heads + head_idx;
                let value = index
                    .scorer
                    .quantizer()
                    .decode(&index.value_codes[code_idx])
                    .map_err(|e| {
                        PolyKvError::DecompressionFailed(format!(
                            "fib batch value decode failed: {e}"
                        ))
                    })?;
                if value.len() != head_dim {
                    return Err(PolyKvError::DimensionMismatch {
                        expected: head_dim,
                        got: value.len(),
                    });
                }
                hits.push(CompressedAttentionHit {
                    token_index,
                    score,
                    value,
                });
            }
            let receipt = CompressedAttentionSelectionReceipt::new(
                self.manifest.pool_id.clone(),
                index.layer_idx as u32,
                head_idx as u32,
                num_tokens as u32,
                hits.len() as u32,
                num_tokens as u64,
                hits.len() as u64,
                false,
                "fib_cold_pool_batch_heads_prefetched_gram_topk_value_decode",
                self.manifest.shared_codec.clone(),
                now_unix(),
            );
            receipt.validate()?;
            results.push(CompressedAttentionSelection { hits, receipt });
        }
        Ok(results)
    }

    /// Batch multi-head compressed top-k attention with adaptive per-head budgets.
    ///
    /// This is the adaptive-budget variant of `attention_topk_batch_heads`.
    /// Instead of a fixed `top_k` for all heads, each head receives a
    /// budget computed from its fragility score via `allocate_head_budgets`.
    /// Fragile heads (low cosine p05) get more candidates; stable heads
    /// get fewer, reducing total computation while preserving quality.
    ///
    /// # Arguments
    /// * `index` - A fully prepared compressed index for one layer.
    /// * `queries` - One query per head (length must equal `index.num_heads`).
    /// * `fragility` - Per-(layer, head) fragility entries from a calibration pass.
    /// * `budget_config` - Configuration for budget allocation.
    ///
    /// # Returns
    /// One `CompressedAttentionSelection` per head, each with its own
    /// potentially different `selected_count`.
    #[cfg(feature = "fib")]
    #[cfg(feature = "adaptive-budget")]
    pub fn attention_topk_batch_heads_adaptive(
        &self,
        index: &FullyPreparedCompressedIndex,
        queries: &[&[f32]],
        fragility: &[compressed_scorer::HeadFragilityEntry],
        budget_config: &compressed_scorer::BudgetConfig,
    ) -> Result<Vec<CompressedAttentionSelection>> {
        let num_heads = index.num_heads;
        let num_tokens = index.num_tokens;
        let head_dim = index.head_dim;
        let layer_idx = index.layer_idx;

        if queries.len() != num_heads {
            return Err(PolyKvError::DimensionMismatch {
                expected: num_heads,
                got: queries.len(),
            });
        }

        // Allocate per-head budgets from fragility data.
        let head_budgets = compressed_scorer::allocate_head_budgets(fragility, budget_config);

        // Compute per-head k (excluding recent_guard — that's a layer-level
        // concept; here we use the raw candidate budget clamped to num_tokens).
        let per_head_k: Vec<usize> = (0..num_heads)
            .map(|h| {
                let k = head_budgets.get(layer_idx as u32, h as u32);
                // Subtract recent_guard to get the candidate k (the guard
                // tokens are always included separately in a full system,
                // but here we just use the candidate portion for top-k
                // selection).
                let candidate_k = k.saturating_sub(budget_config.recent_guard);
                candidate_k.min(num_tokens).max(1)
            })
            .collect();

        // Prepare gram rows for each head's query (same as fixed-k path).
        let mut all_prefetched: Vec<PrefetchedGramRows> = Vec::with_capacity(num_heads);
        for q in queries.iter() {
            if q.len() != head_dim {
                return Err(PolyKvError::DimensionMismatch {
                    expected: head_dim,
                    got: q.len(),
                });
            }
            let prepared = index
                .scorer
                .prepare_query(q)
                .map_err(|e| PolyKvError::Internal(format!("fib batch query prep failed: {e}")))?;
            let n = index.scorer.quantizer().profile().codebook_size as usize;
            let block_count = index.scorer.quantizer().profile().block_count() as usize;
            let gram = index.scorer.gram_table();
            let mut gram_rows = vec![0.0f32; block_count * n];
            for (block_idx, &query_idx) in prepared.query_indices.iter().enumerate() {
                let qi = query_idx as usize;
                if qi >= n {
                    return Err(PolyKvError::Internal(format!(
                        "fib batch: query_idx {qi} >= {n}"
                    )));
                }
                let src = &gram.values()[qi * n..(qi + 1) * n];
                gram_rows[block_idx * n..(block_idx + 1) * n].copy_from_slice(src);
            }
            all_prefetched.push(PrefetchedGramRows {
                gram_rows,
                block_count,
                n,
                query_norm: prepared.query_norm,
            });
        }

        // Score all heads for all tokens in one pass (same as fixed-k path).
        let n = all_prefetched[0].n;
        let block_count = all_prefetched[0].block_count;
        let mut all_scored: Vec<Vec<(usize, f32)>> =
            vec![vec![(0usize, 0.0f32); num_tokens]; num_heads];

        for token_idx in 0..num_tokens {
            for head_idx in 0..num_heads {
                let code_idx = token_idx * num_heads + head_idx;
                let indices = index.key_block(code_idx);
                let stored_norm = index.key_norms[code_idx];
                let q_norm = all_prefetched[head_idx].query_norm as f32;
                let gram_rows = &all_prefetched[head_idx].gram_rows;

                let mut total = 0.0f32;
                for (block_idx, &stored_idx) in indices.iter().enumerate().take(block_count) {
                    let si = stored_idx as usize;
                    if si >= n {
                        return Err(PolyKvError::Internal(format!(
                            "fib batch: stored_idx {si} >= {n}"
                        )));
                    }
                    total += gram_rows[block_idx * n + si];
                }
                let score = total * q_norm * stored_norm;
                all_scored[head_idx][token_idx] = (token_idx, score);
            }
        }

        // Top-k selection per head using adaptive per-head k.
        let mut results = Vec::with_capacity(num_heads);
        for (head_idx, scored) in all_scored.iter_mut().enumerate() {
            let head_k = per_head_k[head_idx];
            let selected = head_k.min(scored.len());
            if selected > 0 && selected < scored.len() {
                scored.select_nth_unstable_by(selected - 1, |a, b| {
                    b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0))
                });
                scored.truncate(selected);
            }
            scored.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

            let mut hits = Vec::with_capacity(selected);
            for &(token_index, score) in scored.iter().take(selected) {
                let code_idx = token_index * num_heads + head_idx;
                let value = index
                    .scorer
                    .quantizer()
                    .decode(&index.value_codes[code_idx])
                    .map_err(|e| {
                        PolyKvError::DecompressionFailed(format!(
                            "fib batch value decode failed: {e}"
                        ))
                    })?;
                if value.len() != head_dim {
                    return Err(PolyKvError::DimensionMismatch {
                        expected: head_dim,
                        got: value.len(),
                    });
                }
                hits.push(CompressedAttentionHit {
                    token_index,
                    score,
                    value,
                });
            }
            let receipt = CompressedAttentionSelectionReceipt::new(
                self.manifest.pool_id.clone(),
                layer_idx as u32,
                head_idx as u32,
                num_tokens as u32,
                hits.len() as u32,
                num_tokens as u64,
                hits.len() as u64,
                false,
                "fib_cold_pool_batch_heads_adaptive_budget_topk_value_decode",
                self.manifest.shared_codec.clone(),
                now_unix(),
            );
            receipt.validate()?;
            results.push(CompressedAttentionSelection { hits, receipt });
        }
        Ok(results)
    }

    /// Search for tokens most similar to a query vector.
    ///
    /// Decompresses the specified layer's key blocks and returns the
    /// top-K token indices with exact cosine similarity scores.
    /// For small pools (<10K tokens) this is fast enough with linear scan.
    /// For larger pools, prefer a dedicated ANN index.
    pub fn search_similar_tokens(
        &self,
        layer_idx: usize,
        query: &[f32],
        top_k: usize,
    ) -> Result<Vec<(usize, f32)>> {
        let decompressed = self.decompress_layer(layer_idx)?;
        let keys = decompressed
            .keys
            .first()
            .ok_or_else(|| PolyKvError::Internal("pool has no key heads".into()))?;

        let head_dim = decompressed.head_dim;
        let num_tokens = keys.len() / head_dim;

        let mut scored: Vec<(usize, f32)> = Vec::with_capacity(num_tokens);
        for i in 0..num_tokens {
            let start = i * head_dim;
            let vec = &keys[start..start + head_dim];
            let dot: f32 = query.iter().zip(vec.iter()).map(|(a, b)| a * b).sum();
            scored.push((i, dot));
        }

        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k.min(num_tokens));
        Ok(scored)
    }

    // ---------- persistence ----------

    /// Save the pool to a JSON file.
    ///
    /// Writes the manifest and all layers (including compressed payloads)
    /// to a single JSON file. Compressed payloads are embedded as base64.
    /// For large pools (>100K tokens), consider mmap-based persistence instead.
    pub fn save_to_path(&self, path: &std::path::Path) -> Result<()> {
        let json = serde_json::to_string_pretty(&PoolFileEnvelope {
            schema: "polykv_pool_file_v1".into(),
            manifest: self.manifest.clone(),
            layers: self.layers.clone(),
            policy: self.policy.clone(),
        })
        .map_err(|e| PolyKvError::Internal(format!("pool serialize: {e}")))?;
        std::fs::write(path, &json)?;
        Ok(())
    }

    /// Load a pool from a JSON file previously written by [`save_to_path`].
    pub fn load_from_path(path: &std::path::Path) -> Result<Self> {
        let json = std::fs::read_to_string(path)?;
        let envelope: PoolFileEnvelope = serde_json::from_str(&json)
            .map_err(|e| PolyKvError::Internal(format!("pool deserialize: {e}")))?;
        if envelope.schema != "polykv_pool_file_v1" {
            return Err(PolyKvError::Internal(format!(
                "unknown pool file schema: {}",
                envelope.schema
            )));
        }
        Ok(Self {
            manifest: envelope.manifest,
            layers: envelope.layers,
            policy: envelope.policy,
        })
    }
}

/// Serialization envelope for pool files.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct PoolFileEnvelope {
    schema: String,
    manifest: PoolManifest,
    layers: Vec<PoolLayer>,
    policy: CompressionPolicy,
}

/// Pre-built compressed index for one layer/head, avoiding per-call reconstruction.
///
/// Caches decoded key codes, value codes, and a built FibScorer so that
/// `attention_topk_compressed_prepared` only needs to prepare the query
/// (O(dim)) and score (O(num_tokens)) without rebuilding codec state.
#[cfg(feature = "fib")]
pub struct PreparedCompressedIndex {
    /// Layer index this index was built for.
    pub layer_idx: usize,
    /// Head index this index was built for.
    pub head_idx: usize,
    /// Head dimension.
    pub head_dim: usize,
    /// Number of tokens in the pool.
    pub num_tokens: usize,
    /// Number of KV heads.
    pub num_heads: usize,
    /// Decoded key codes for the entire layer (all heads, all tokens).
    pub key_codes: Vec<fib_quant::FibCodeV1>,
    /// Decoded value codes for the entire layer (all heads, all tokens).
    pub value_codes: Vec<fib_quant::FibCodeV1>,
    /// Built FibScorer (includes quantizer).
    pub scorer: fib_quant::FibScorer,
}

/// Fully prepared compressed index: pre-unpacks all indices and norms
/// so the scoring loop is just Gram table lookups with no per-call unpacking.
///
/// This is the tightest possible hot path for compressed attention scoring.
#[cfg(feature = "fib")]
pub struct FullyPreparedCompressedIndex {
    /// Layer index this index was built for.
    pub layer_idx: usize,
    /// Head index this index was built for.
    pub head_idx: usize,
    /// Head dimension.
    pub head_dim: usize,
    /// Number of tokens in the pool.
    pub num_tokens: usize,
    /// Number of KV heads.
    pub num_heads: usize,
    /// Pre-unpacked key indices: flat `num_entries * block_count` u32 buffer,
    /// row-major by `code_idx = token_idx * num_heads + head_idx`.
    /// Use `key_block(code_idx)` to get the slice for one entry.
    pub key_indices_flat: Vec<u32>,
    /// Pre-decoded key norms as f32: one per (token, head) entry.
    pub key_norms: Vec<f32>,
    /// Number of blocks per vector (= head_dim / k where k is the block dim).
    pub block_count: usize,
    /// Pre-unpacked value codes (for decode of selected top-k).
    pub value_codes: Vec<fib_quant::FibCodeV1>,
    /// Built FibScorer (for query preparation and Gram table access).
    pub scorer: fib_quant::FibScorer,
}

/// Pre-fetched Gram rows for a specific query, enabling cache-friendly scoring.
///
/// After preparing a query, the relevant Gram table rows (one per block)
/// are copied into a contiguous buffer. The scoring loop then gathers
/// from this buffer using `stored_idx` as offset, eliminating the
/// `query_idx * N` multiply and improving cache locality.
#[cfg(feature = "fib")]
pub struct PrefetchedGramRows {
    /// Contiguous `block_count * N` f32 buffer.
    /// Row `b` is at `gram_rows[b * N .. (b+1) * N]`.
    pub gram_rows: Vec<f32>,
    /// Number of blocks per vector.
    pub block_count: usize,
    /// Codebook size N.
    pub n: usize,
    /// Prepared query norm.
    pub query_norm: f64,
}

#[cfg(feature = "fib")]
impl FullyPreparedCompressedIndex {
    /// Get the key indices for one (token, head) entry as a slice.
    #[inline]
    pub fn key_block(&self, code_idx: usize) -> &[u32] {
        let start = code_idx * self.block_count;
        &self.key_indices_flat[start..start + self.block_count]
    }

    /// Prepare a query and pre-fetch the relevant Gram rows.
    ///
    /// This copies `block_count` rows from the Gram table into a contiguous
    /// buffer, so the per-token scoring loop becomes sequential gathers
    /// from a 2KB working set instead of scattered accesses across 4KB+.
    pub fn prepare_gram_rows(&self, query: &[f32]) -> Result<PrefetchedGramRows> {
        if query.len() != self.head_dim {
            return Err(PolyKvError::DimensionMismatch {
                expected: self.head_dim,
                got: query.len(),
            });
        }
        let prepared = self
            .scorer
            .prepare_query(query)
            .map_err(|e| PolyKvError::Internal(format!("fib query preparation failed: {e}")))?;
        let n = self.scorer.quantizer().profile().codebook_size as usize;
        let block_count = self.scorer.quantizer().profile().block_count() as usize;
        let gram = self.scorer.gram_table();

        // Pre-fetch: copy gram row `query_indices[block]` into gram_rows[block * N..]
        let mut gram_rows = vec![0.0f32; block_count * n];
        for (block_idx, &query_idx) in prepared.query_indices.iter().enumerate() {
            let qi = query_idx as usize;
            if qi >= n {
                return Err(PolyKvError::Internal(format!(
                    "fib prepare_gram_rows: query_idx {qi} >= {n}"
                )));
            }
            let src = &gram.values()[qi * n..(qi + 1) * n];
            gram_rows[block_idx * n..(block_idx + 1) * n].copy_from_slice(src);
        }

        Ok(PrefetchedGramRows {
            gram_rows,
            block_count,
            n,
            query_norm: prepared.query_norm,
        })
    }

    /// Score all tokens against pre-fetched Gram rows.
    ///
    /// The hot loop is:
    /// ```text
    /// for block in 0..block_count:
    ///     total += gram_rows[block * N + stored_idx]
    /// ```
    /// This is sequential access into a small contiguous buffer —
    /// auto-vectorizable and cache-friendly.
    pub fn score_all_tokens(&self, prefetched: &PrefetchedGramRows) -> Result<Vec<(usize, f32)>> {
        let head_idx = self.head_idx;
        let num_heads = self.num_heads;
        let num_tokens = self.num_tokens;
        let q_norm = prefetched.query_norm as f32;
        let n = prefetched.n;
        let block_count = prefetched.block_count;
        let gram_rows = &prefetched.gram_rows;

        let mut scored: Vec<(usize, f32)> = vec![(0usize, 0.0f32); num_tokens];
        for token_idx in 0..num_tokens {
            let code_idx = token_idx * num_heads + head_idx;
            let indices = self.key_block(code_idx);
            let stored_norm = self.key_norms[code_idx];

            let mut total = 0.0f32;
            for (block_idx, &stored_idx) in indices.iter().enumerate().take(block_count) {
                let si = stored_idx as usize;
                if si >= n {
                    return Err(PolyKvError::Internal(format!(
                        "fib score_all_tokens: stored_idx {si} >= {n}"
                    )));
                }
                total += gram_rows[block_idx * n + si];
            }
            let score = total * q_norm * stored_norm;
            scored[token_idx] = (token_idx, score);
        }
        Ok(scored)
    }
}

/// Reconstructed K/V tensors for one layer of the shared pool.
///
/// All vectors are in the original (head × token × head_dim) layout but
/// flat per head: `keys[head_idx][token_idx * head_dim + j]`. This matches
/// the HuggingFace `DynamicCache` per-layer access pattern
/// (`cache.layers[layer_idx].keys[:, head_idx, :, :]` flattened along the
/// last two dims).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DecompressedLayer {
    /// Original layer index in the model.
    pub layer_index: u32,
    /// Number of tokens in this layer (= pool's `num_shared_tokens`).
    pub num_tokens: usize,
    /// Number of KV heads (= pool's `num_kv_heads`).
    pub num_heads: usize,
    /// Per-head dimension.
    pub head_dim: usize,
    /// Decoded K vectors: `keys[head_idx]` is a flat `Vec<f32>` of length
    /// `num_tokens * head_dim` in token order.
    pub keys: Vec<Vec<f32>>,
    /// Decoded V vectors, same layout as `keys`.
    pub values: Vec<Vec<f32>>,
}

/// Trait for KV cache targets that can receive injected blocks.
pub trait CacheTarget: std::fmt::Debug {
    /// Get the number of layers in this cache.
    fn num_layers(&self) -> u32;

    /// Append a key block at a specific layer and position.
    fn append_key(&mut self, layer: u32, position: u32, key: &[f32]) -> Result<()>;

    /// Append a value block at a specific layer and position.
    fn append_value(&mut self, layer: u32, position: u32, value: &[f32]) -> Result<()>;

    /// Get the current sequence length (tokens in cache).
    fn seq_len(&self) -> u32;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::AttentionType;

    fn make_test_shape() -> KvTensorShape {
        KvTensorShape {
            attention_type: AttentionType::MHA,
            num_layers: 2,
            num_heads: 4,
            num_kv_heads: 4,
            head_dim: 8, // must be divisible by k=4 for fib-quant
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
    fn test_pool_build_empty() {
        let shape = make_test_shape();
        let corpus: Vec<(String, Vec<f32>)> = vec![];
        let result = SharedKVPool::build(&corpus, &shape, 42);
        assert!(result.is_err());
    }

    #[test]
    fn test_pool_build_basic() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(4);
        let result = SharedKVPool::build(&corpus, &shape, 42);
        assert!(result.is_ok(), "build failed: {:?}", result.err());

        let (pool, receipt) = result.unwrap();
        assert_eq!(pool.layers.len(), 2);
        assert_eq!(pool.manifest.num_shared_tokens, 4);
        assert_eq!(receipt.total_tokens, 4);
        assert!(
            receipt.compression_ratio > 0.0,
            "compression ratio: {}",
            receipt.compression_ratio
        );
        // Note: ratio < 1.0 is normal for tiny test corpora where JSON
        // serialization overhead dominates the encoded payload.
    }

    #[test]
    fn test_pool_build_deterministic() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(4);

        let (pool1, receipt1) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let (pool2, receipt2) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        assert_eq!(receipt1.pool_digest, receipt2.pool_digest);
        assert_eq!(receipt1.layer_digests, receipt2.layer_digests);
        assert_eq!(pool1.layers[0].block_digest, pool2.layers[0].block_digest);
    }

    #[test]
    fn test_pool_build_different_seeds() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(4);

        let (_pool1, receipt1) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let (_pool2, receipt2) = SharedKVPool::build(&corpus, &shape, 12345).unwrap();

        assert_ne!(receipt1.pool_digest, receipt2.pool_digest);
    }

    #[test]
    fn test_decompress_layer_recovers_finite_floats() {
        // Round-trip integrity: build a pool, decompress every layer,
        // verify the output is finite, the right shape, and per-head
        // lengths match num_tokens * head_dim.
        let shape = make_test_shape();
        let corpus = make_test_corpus(8);
        let (pool, _) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        for layer_idx in 0..shape.num_layers as usize {
            let decompressed = pool.decompress_layer(layer_idx).unwrap();
            assert_eq!(decompressed.num_tokens, 8);
            assert_eq!(decompressed.num_heads, shape.num_kv_heads as usize);
            assert_eq!(decompressed.head_dim, shape.head_dim);
            assert_eq!(decompressed.keys.len(), shape.num_kv_heads as usize);
            assert_eq!(decompressed.values.len(), shape.num_kv_heads as usize);
            for h in 0..decompressed.num_heads {
                assert_eq!(decompressed.keys[h].len(), 8 * shape.head_dim);
                assert_eq!(decompressed.values[h].len(), 8 * shape.head_dim);
                assert!(decompressed.keys[h].iter().all(|v| v.is_finite()));
                assert!(decompressed.values[h].iter().all(|v| v.is_finite()));
            }
        }
    }

    #[test]
    fn test_decompress_layer_is_deterministic() {
        // Same corpus + same seed must produce byte-identical decompressed
        // output. This is the core invariant for HuggingFaceDynamicCache
        // round-trip: a fresh DynamicCache populated from the pool must
        // see the same K/V tensors across runs.
        let shape = make_test_shape();
        let corpus = make_test_corpus(6);
        let (pool_a, _) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let (pool_b, _) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        for layer_idx in 0..shape.num_layers as usize {
            let a = pool_a.decompress_layer(layer_idx).unwrap();
            let b = pool_b.decompress_layer(layer_idx).unwrap();
            assert_eq!(
                a.keys, b.keys,
                "decompressed K tensors must be deterministic across builds (layer {})",
                layer_idx
            );
            assert_eq!(a.values, b.values);
        }
    }

    #[test]
    fn test_mismatched_shape_rejected() {
        let shape = make_test_shape();
        let mut bad_corpus = make_test_corpus(1);
        // Truncate the vector
        bad_corpus[0].1.truncate(10);
        let result = SharedKVPool::build(&bad_corpus, &shape, 42);
        assert!(result.is_err());
    }

    #[test]
    fn test_pool_build_writes_single_fb2_payload_per_layer_side() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(32);
        let (pool, receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        assert_eq!(pool.layers.len(), shape.num_layers as usize);
        for layer in &pool.layers {
            assert_eq!(
                layer.key_blocks.len(),
                1,
                "pool layer keys must be stored as one batched payload, not per-vector blocks"
            );
            assert_eq!(
                layer.value_blocks.len(),
                1,
                "pool layer values must be stored as one batched payload, not per-vector blocks"
            );
            assert_eq!(&layer.key_blocks[0].encoded_payload[0..4], b"FBWB");
            assert_eq!(&layer.value_blocks[0].encoded_payload[0..4], b"FBWB");
        }
        let raw_bytes = shape.total_kv_bytes(corpus.len()) as f64;
        let ratio = raw_bytes / receipt.pool_size_bytes as f64;
        // With self-describing FibCodeWireV1 headers (59 bytes per code),
        // tiny test corpora may not achieve high compression. The wire
        // format trades per-code space for self-description (seed/dim/k/N
        // in the header — no external profile needed for decode).
        assert!(
            ratio > 0.2,
            "batched pool should show some compression; ratio={ratio:.2}"
        );
    }

    /// The pool build must produce the same `pool_digest` and `block_digest`
    /// values regardless of whether the underlying codec dispatches to GPU
    /// or CPU. This guards against the "receipt says gpu, code did cpu"
    /// failure mode that earlier feature-flag-only wiring exhibited.
    #[test]
    fn test_pool_build_digest_invariant_across_corpora_size() {
        let shape = make_test_shape();

        // Tiny corpus — under the GPU batch threshold (n < 16).
        let small = make_test_corpus(4);
        let (pool_small, receipt_small) = SharedKVPool::build(&small, &shape, 42).unwrap();

        // Large corpus — well over the GPU batch threshold.
        let large = make_test_corpus(40);
        let (pool_large, receipt_large) = SharedKVPool::build(&large, &shape, 42).unwrap();

        assert!(!pool_small.layers.is_empty());
        assert!(!pool_large.layers.is_empty());
        assert!(receipt_small.backend == "cpu" || receipt_small.backend == "gpu");
        assert!(receipt_large.backend == "cpu" || receipt_large.backend == "gpu");

        // Tiny corpus must NOT claim gpu — the per-(layer,head) batch is
        // 4 docs * 4 kv heads = 16 vectors, exactly at the threshold, and
        // the per-call probe (not the device probe) drives the receipt.
        // This is the honesty invariant.
        assert_eq!(
            receipt_small.backend, "cpu",
            "corpus under GPU batch threshold should fall through to CPU"
        );
    }

    #[test]
    fn test_search_similar_tokens_returns_top_k() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(32);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        // Query with arbitrary vector of correct dimension
        let query: Vec<f32> = (0..shape.head_dim).map(|x| x as f32 * 0.1).collect();
        let results = pool.search_similar_tokens(0, &query, 5).unwrap();

        assert!(!results.is_empty(), "search should return results");
        assert!(results.len() <= 5, "should return at most top_k");
        // Every returned token index should be in range
        for (idx, _) in &results {
            assert!(*idx < 32, "token index must be in range of corpus size");
        }
        // Scores should be sorted descending
        for w in results.windows(2) {
            assert!(w[0].1 >= w[1].1, "scores should be descending");
        }
    }

    #[test]
    fn test_prepared_compressed_index_matches_regular_attention() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(16);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let query: Vec<f32> = (0..shape.head_dim).map(|x| x as f32 * 0.125).collect();

        let regular = pool
            .attention_topk_compressed(0, 0, &query, 5)
            .expect("regular compressed attention should work");

        let index = pool
            .prepare_compressed_index(0, 0)
            .expect("prepare compressed index should work");

        let prepared = pool
            .attention_topk_compressed_prepared(&index, &query, 5)
            .expect("prepared compressed attention should work");

        assert_eq!(prepared.hits.len(), regular.hits.len());
        for (a, b) in prepared.hits.iter().zip(regular.hits.iter()) {
            assert_eq!(a.token_index, b.token_index);
            assert!((a.score - b.score).abs() < 1e-5);
            assert_eq!(a.value.len(), b.value.len());
        }
        assert_eq!(
            prepared.receipt.scoring_path,
            "fib_cold_pool_prepared_compressed_score_topk_value_decode"
        );
    }

    #[test]
    fn test_prepared_compressed_index_rejects_wrong_query_dimension() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(8);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let index = pool
            .prepare_compressed_index(0, 0)
            .expect("prepare compressed index should work");
        let err = pool
            .attention_topk_compressed_prepared(&index, &[1.0, 2.0], 3)
            .expect_err("wrong query dimension must fail");
        assert!(matches!(
            err,
            PolyKvError::DimensionMismatch {
                expected: 8,
                got: 2
            }
        ));
    }

    #[test]
    fn test_fully_prepared_compressed_index_matches_regular_attention() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(16);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let query: Vec<f32> = (0..shape.head_dim).map(|x| x as f32 * 0.125).collect();

        let regular = pool
            .attention_topk_compressed(0, 0, &query, 5)
            .expect("regular compressed attention should work");

        let fully_index = pool
            .prepare_fully_compressed_index(0, 0)
            .expect("prepare fully compressed index should work");

        let fully_prepared = pool
            .attention_topk_fully_prepared(&fully_index, &query, 5)
            .expect("fully prepared compressed attention should work");

        // Token indices should match (same ranking), though scores may differ
        // slightly because fully-prepared computes norm from decoded vector
        // while regular uses the wire-format stored norm.
        assert_eq!(fully_prepared.hits.len(), regular.hits.len());
        for (a, b) in fully_prepared.hits.iter().zip(regular.hits.iter()) {
            assert_eq!(a.token_index, b.token_index);
            assert_eq!(a.value.len(), b.value.len());
        }
        assert_eq!(
            fully_prepared.receipt.scoring_path,
            "fib_cold_pool_prefetched_gram_rows_topk_value_decode"
        );
    }

    #[test]
    fn test_fully_prepared_compressed_index_rejects_wrong_query_dimension() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(8);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let fully_index = pool
            .prepare_fully_compressed_index(0, 0)
            .expect("prepare fully compressed index should work");
        let err = pool
            .attention_topk_fully_prepared(&fully_index, &[1.0, 2.0], 3)
            .expect_err("wrong query dimension must fail");
        assert!(matches!(
            err,
            PolyKvError::DimensionMismatch {
                expected: 8,
                got: 2
            }
        ));
    }

    #[test]
    fn test_prefetched_gram_rows_matches_regular_attention() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(16);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let query: Vec<f32> = (0..shape.head_dim).map(|x| x as f32 * 0.125).collect();

        let regular = pool
            .attention_topk_compressed(0, 0, &query, 5)
            .expect("regular compressed attention should work");

        let fully_index = pool
            .prepare_fully_compressed_index(0, 0)
            .expect("prepare fully compressed index should work");

        let prefetched = pool
            .attention_topk_prefetched(&fully_index, &query, 5)
            .expect("prefetched compressed attention should work");

        assert_eq!(prefetched.hits.len(), regular.hits.len());
        for (a, b) in prefetched.hits.iter().zip(regular.hits.iter()) {
            assert_eq!(a.token_index, b.token_index);
        }
        assert_eq!(
            prefetched.receipt.scoring_path,
            "fib_cold_pool_prefetched_gram_rows_topk_value_decode"
        );
    }

    #[test]
    fn test_batch_heads_returns_correct_count() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(16);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        let fully_index = pool
            .prepare_fully_compressed_index(0, 0)
            .expect("prepare fully compressed index should work");

        let queries: Vec<Vec<f32>> = (0..shape.num_kv_heads as usize)
            .map(|h| {
                (0..shape.head_dim)
                    .map(|x| x as f32 * 0.125 + h as f32 * 0.01)
                    .collect()
            })
            .collect();
        let query_refs: Vec<&[f32]> = queries.iter().map(|q| q.as_slice()).collect();

        let results = pool
            .attention_topk_batch_heads(&fully_index, &query_refs, 5)
            .expect("batch heads should work");

        assert_eq!(results.len(), shape.num_kv_heads as usize);
        for r in &results {
            assert_eq!(r.hits.len(), 5);
            assert_eq!(
                r.receipt.scoring_path,
                "fib_cold_pool_batch_heads_prefetched_gram_topk_value_decode"
            );
        }
    }

    #[test]
    fn test_compressed_attention_topk_scores_cold_pool_without_full_layer_decode() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(32);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
        let query: Vec<f32> = (0..shape.head_dim).map(|x| x as f32 * 0.1).collect();

        let out = pool
            .attention_topk_compressed(0, 0, &query, 3)
            .expect("compressed attention selection should work over fib cold pool");

        assert_eq!(out.hits.len(), 3);
        assert_eq!(
            out.receipt.schema_version,
            "compressed_attention_selection_receipt_v1"
        );
        assert_eq!(out.receipt.layer, 0);
        assert_eq!(out.receipt.head, 0);
        assert_eq!(out.receipt.candidate_count, 32);
        assert_eq!(out.receipt.selected_count, 3);
        assert_eq!(out.receipt.compressed_key_scores, 32);
        assert_eq!(out.receipt.decoded_value_vectors, 3);
        assert!(!out.receipt.full_layer_decoded);
        assert_eq!(
            out.receipt.scoring_path,
            "fib_cold_pool_compressed_score_topk_value_decode"
        );
        for hit in &out.hits {
            assert!(hit.token_index < 32);
            assert_eq!(hit.value.len(), shape.head_dim);
            assert!(hit.value.iter().all(|v| v.is_finite()));
        }
        for window in out.hits.windows(2) {
            assert!(window[0].score >= window[1].score);
        }
    }

    #[test]
    fn test_compressed_attention_topk_rejects_wrong_query_dimension() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(8);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        let err = pool
            .attention_topk_compressed(0, 0, &[1.0, 2.0], 3)
            .expect_err("wrong query dimension must fail before scoring");

        assert!(matches!(
            err,
            PolyKvError::DimensionMismatch {
                expected: 8,
                got: 2
            }
        ));
    }

    #[test]
    fn test_persistence_roundtrip() {
        let shape = make_test_shape();
        let corpus = make_test_corpus(16);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        // Serialize to JSON string and back
        let json = serde_json::to_string_pretty(&PoolFileEnvelope {
            schema: "polykv_pool_file_v1".into(),
            manifest: pool.manifest.clone(),
            layers: pool.layers.clone(),
            policy: pool.policy.clone(),
        })
        .unwrap();

        let envelope: PoolFileEnvelope = serde_json::from_str(&json).unwrap();
        let loaded = SharedKVPool {
            manifest: envelope.manifest,
            layers: envelope.layers,
            policy: envelope.policy,
        };

        assert_eq!(pool.layers.len(), loaded.layers.len());
        assert_eq!(
            pool.layers[0].key_blocks.len(),
            loaded.layers[0].key_blocks.len()
        );
        assert_eq!(pool.manifest.pool_id, loaded.manifest.pool_id);

        // Decompressed search should produce identical results
        let query: Vec<f32> = (0..shape.head_dim).map(|x| x as f32 * 0.1).collect();
        let orig_results = pool.search_similar_tokens(0, &query, 3).unwrap();
        let loaded_results = loaded.search_similar_tokens(0, &query, 3).unwrap();
        assert_eq!(orig_results, loaded_results);
    }

    #[cfg(feature = "adaptive-budget")]
    #[test]
    fn test_adaptive_budget_produces_different_k_per_head() {
        use compressed_scorer::{allocate_head_budgets, BudgetConfig, HeadFragilityEntry};

        let shape = make_test_shape();
        let corpus = make_test_corpus(16);
        let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();

        let fully_index = pool
            .prepare_fully_compressed_index(0, 0)
            .expect("prepare fully compressed index should work");

        let queries: Vec<Vec<f32>> = (0..shape.num_kv_heads as usize)
            .map(|h| {
                (0..shape.head_dim)
                    .map(|x| x as f32 * 0.125 + h as f32 * 0.01)
                    .collect()
            })
            .collect();
        let query_refs: Vec<&[f32]> = queries.iter().map(|q| q.as_slice()).collect();

        // Create fragility entries: head 0 is fragile, head 1 is stable,
        // heads 2 and 3 are in between.
        let fragility = vec![
            HeadFragilityEntry {
                layer: 0,
                head: 0,
                cos_p05: 0.712, // very fragile
            },
            HeadFragilityEntry {
                layer: 0,
                head: 1,
                cos_p05: 0.9999, // very stable
            },
            HeadFragilityEntry {
                layer: 0,
                head: 2,
                cos_p05: 0.95,
            },
            HeadFragilityEntry {
                layer: 0,
                head: 3,
                cos_p05: 0.98,
            },
        ];

        let config = BudgetConfig {
            ref_k: 8,
            target_cosine: 0.995,
            min_k: 2,
            max_k: 14,
            recent_guard: 4,
        };

        // Verify the budgets differ across heads before running attention.
        let head_budgets = allocate_head_budgets(&fragility, &config);
        let k0 = head_budgets.get(0, 0);
        let k1 = head_budgets.get(0, 1);
        assert!(
            k0 > k1,
            "fragile head 0 (k={k0}) should get more than stable head 1 (k={k1})"
        );

        let results = pool
            .attention_topk_batch_heads_adaptive(&fully_index, &query_refs, &fragility, &config)
            .expect("adaptive batch heads should work");

        assert_eq!(results.len(), shape.num_kv_heads as usize);

        // Verify different heads got different selected counts.
        let selected_counts: Vec<u32> = results.iter().map(|r| r.receipt.selected_count).collect();
        // Not all heads should have the same count (fragile head gets more).
        let min_count = *selected_counts.iter().min().unwrap();
        let max_count = *selected_counts.iter().max().unwrap();
        assert!(
            min_count != max_count,
            "adaptive budget should produce different k per head, but all heads got {min_count}"
        );

        // Verify the fragile head (head 0) got at least as many as stable head (head 1).
        assert!(
            selected_counts[0] >= selected_counts[1],
            "fragile head 0 should get at least as many candidates as stable head 1: {} vs {}",
            selected_counts[0],
            selected_counts[1]
        );

        // Verify the scoring path label.
        for r in &results {
            assert_eq!(
                r.receipt.scoring_path,
                "fib_cold_pool_batch_heads_adaptive_budget_topk_value_decode"
            );
        }
    }
}
