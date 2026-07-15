//! poly-kv bridge for semantic-memory integration.
//!
//! Gated behind the `poly-kv-codec` feature.

use crate::error::MemoryError;

/// Build a SharedKVPool from (key, embedding) pairs.
pub fn build_pool_from_embeddings(
    embeddings: &[(String, Vec<f32>)],
    embedding_dim: usize,
    seed: u64,
) -> Result<(poly_kv::SharedKVPool, poly_kv::PoolBuildReceipt), MemoryError> {
    let shape = poly_kv::KvTensorShape {
        attention_type: poly_kv::AttentionType::MHA,
        num_layers: 1,
        num_heads: 1,
        num_kv_heads: 1,
        head_dim: embedding_dim,
        hidden_size: embedding_dim,
    };

    let (pool, receipt) = poly_kv::SharedKVPool::build(embeddings, &shape, seed)
        .map_err(|e| MemoryError::Other(format!("poly-kv pool build failed: {e}")))?;

    Ok((pool, receipt))
}

/// Convert a PoolBuildReceipt into a ProveKvPoolGenerationV1.
pub fn receipt_to_generation(
    receipt: &poly_kv::PoolBuildReceipt,
    snapshot_digest: &str,
) -> crate::types::ProveKvPoolGenerationV1 {
    crate::types::ProveKvPoolGenerationV1 {
        schema_version: "provekv_pool_generation_v1".into(),
        generation_id: receipt.pool_digest.hex().to_string(),
        embedding_snapshot_digest: snapshot_digest.to_string(),
        source_digest: receipt.pool_digest.hex().to_string(),
        pool_manifest_digest: String::new(),
        codec_family: "fib_quant".to_string(),
        codec_profile: "k4_n32_paper_default".to_string(),
        vector_dim: 0,
        item_count: receipt.total_tokens as usize,
        payload_bytes: receipt.pool_size_bytes,
        created_at: chrono::Utc::now(),
    }
}

/// Serialize a SharedKVPool to JSON bytes for DB storage.
pub fn serialize_pool(pool: &poly_kv::SharedKVPool) -> Result<Vec<u8>, MemoryError> {
    // SharedKVPool doesn't derive Serialize, but its fields do.
    // Use a manual envelope.
    #[derive(serde::Serialize)]
    struct Envelope<'a> {
        manifest: &'a poly_kv::PoolManifest,
        layers: &'a [poly_kv::PoolLayer],
        policy: &'a poly_kv::CompressionPolicy,
    }
    let env = Envelope {
        manifest: &pool.manifest,
        layers: &pool.layers,
        policy: &pool.policy,
    };
    serde_json::to_vec(&env).map_err(|e| MemoryError::Other(format!("pool serialize: {e}")))
}

/// Deserialize a SharedKVPool from JSON bytes.
pub fn deserialize_pool(data: &[u8]) -> Result<poly_kv::SharedKVPool, MemoryError> {
    #[derive(serde::Deserialize)]
    struct Envelope {
        manifest: poly_kv::PoolManifest,
        layers: Vec<poly_kv::PoolLayer>,
        policy: poly_kv::CompressionPolicy,
    }
    let env: Envelope = serde_json::from_slice(data)
        .map_err(|e| MemoryError::Other(format!("pool deserialize: {e}")))?;
    Ok(poly_kv::SharedKVPool {
        manifest: env.manifest,
        layers: env.layers,
        policy: env.policy,
    })
}

/// Search a pool layer for tokens similar to a query.
pub fn search_pool_layer(
    pool: &poly_kv::SharedKVPool,
    layer_idx: usize,
    query: &[f32],
    top_k: usize,
) -> Result<Vec<(usize, f32)>, MemoryError> {
    pool.search_similar_tokens(layer_idx, query, top_k)
        .map_err(|e| MemoryError::Other(format!("pool search failed: {e}")))
}
