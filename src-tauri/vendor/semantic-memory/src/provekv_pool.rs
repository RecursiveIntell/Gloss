//! proveKV/poly-kv pool generation adapter.
//!
//! `semantic-memory` keeps authoritative f32 embeddings in SQLite. This module builds a
//! generation-level compressed candidate artifact from those rows when the `poly-kv-pool` feature is
//! enabled. The artifact is a derived acceleration pool only: search still exact-reranks against the
//! authoritative f32 rows before returning results.

use crate::error::MemoryError;
use crate::types::{ProveKvPoolGenerationV1, ProveKvPoolItemMapEntryV1};
use crate::vector_snapshot::{embedding_row_digest, EmbeddingSnapshotV1};
use chrono::Utc;

#[cfg(feature = "poly-kv-pool")]
use poly_kv_core::{create_codec, AttentionType, CompressionPolicy, KvTensorShape, SharedKVPool};
#[cfg(feature = "poly-kv-pool")]
use std::collections::HashMap;
#[cfg(feature = "poly-kv-pool")]
use std::sync::{Arc, OnceLock, RwLock};

#[cfg(feature = "poly-kv-pool")]
const SEMANTIC_MEMORY_PROVEKV_POOL_PAYLOAD_MAGIC_V2: &[u8; 4] = b"SMP2";
#[cfg(feature = "poly-kv-pool")]
static DECODED_POOL_SEARCH_CACHE: OnceLock<
    RwLock<HashMap<String, Arc<ProveKvDecodedSearchCache>>>,
> = OnceLock::new();

#[cfg(feature = "poly-kv-pool")]
fn decoded_pool_search_cache() -> &'static RwLock<HashMap<String, Arc<ProveKvDecodedSearchCache>>> {
    DECODED_POOL_SEARCH_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

#[cfg(feature = "poly-kv-pool")]
#[derive(Debug, Clone)]
pub struct ProveKvDecodedSearchCache {
    pub dim: usize,
    pub row_count: usize,
    pub vectors_flat: Vec<f32>,
    pub row_norms: Vec<f32>,
    pub item_keys: Vec<String>,
}

#[cfg(feature = "poly-kv-pool")]
impl ProveKvDecodedSearchCache {
    pub fn from_vectors_and_item_map(
        vectors: Vec<Vec<f32>>,
        vector_dim: usize,
        item_map: &[ProveKvPoolItemMapEntryV1],
    ) -> Result<Self, MemoryError> {
        if vectors.len() != item_map.len() {
            return Err(MemoryError::Other(format!(
                "proveKV decoded search cache expected {} item-map rows but decoded {} vectors",
                item_map.len(),
                vectors.len()
            )));
        }

        let mut vectors_flat = Vec::with_capacity(vectors.len().saturating_mul(vector_dim));
        let mut row_norms = Vec::with_capacity(vectors.len());
        for (index, vector) in vectors.into_iter().enumerate() {
            crate::db::validate_embedding(&vector, vector_dim).map_err(|err| {
                MemoryError::Other(format!(
                    "proveKV decoded search cache vector {index} failed validation: {err}"
                ))
            })?;
            let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            row_norms.push(norm);
            vectors_flat.extend_from_slice(&vector);
        }
        let item_keys = item_map
            .iter()
            .map(|entry| format!("{}:{}", entry.source_type, entry.item_id))
            .collect::<Vec<_>>();

        Ok(Self {
            dim: vector_dim,
            row_count: item_map.len(),
            vectors_flat,
            row_norms,
            item_keys,
        })
    }

    pub fn row(&self, index: usize) -> Option<&[f32]> {
        let start = index.checked_mul(self.dim)?;
        let end = start.checked_add(self.dim)?;
        self.vectors_flat.get(start..end)
    }
}

#[cfg(feature = "poly-kv-pool")]
fn padded_vector_dim_for_poly_kv(vector_dim: usize) -> usize {
    let min_head_dim = vector_dim.div_ceil(2).max(8);
    let head_dim = min_head_dim.div_ceil(4) * 4;
    head_dim * 2
}

#[cfg(feature = "poly-kv-pool")]
fn semantic_embedding_shape(vector_dim: usize) -> KvTensorShape {
    let padded_vector_dim = padded_vector_dim_for_poly_kv(vector_dim);
    let head_dim = padded_vector_dim / 2;
    KvTensorShape {
        attention_type: AttentionType::MHA,
        num_layers: 1,
        num_heads: 1,
        num_kv_heads: 1,
        head_dim,
        hidden_size: head_dim,
    }
}

#[cfg(feature = "poly-kv-pool")]
fn semantic_embedding_corpus(snapshot: &EmbeddingSnapshotV1) -> Vec<(String, Vec<f32>)> {
    let padded_vector_dim = padded_vector_dim_for_poly_kv(snapshot.vector_dim);
    snapshot
        .rows
        .iter()
        .map(|row| {
            let mut embedding = row.embedding.clone();
            embedding.resize(padded_vector_dim, 0.0);
            let head_dim = padded_vector_dim / 2;
            if embedding[..head_dim].iter().all(|value| *value == 0.0) {
                embedding[0] = f32::EPSILON;
            }
            if embedding[head_dim..].iter().all(|value| *value == 0.0) {
                embedding[head_dim] = f32::EPSILON;
            }
            (format!("{}:{}", row.source_type, row.item_id), embedding)
        })
        .collect()
}

#[cfg(feature = "poly-kv-pool")]
fn push_u16_len_prefixed_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), MemoryError> {
    let len = u16::try_from(bytes.len()).map_err(|_| {
        MemoryError::Other(format!(
            "proveKV compact payload field too large for u16 length prefix: {} bytes",
            bytes.len()
        ))
    })?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

#[cfg(feature = "poly-kv-pool")]
fn push_u32_len_prefixed_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Result<(), MemoryError> {
    let len = u32::try_from(bytes.len()).map_err(|_| {
        MemoryError::Other(format!(
            "proveKV compact payload field too large for u32 length prefix: {} bytes",
            bytes.len()
        ))
    })?;
    out.extend_from_slice(&len.to_le_bytes());
    out.extend_from_slice(bytes);
    Ok(())
}

#[cfg(feature = "poly-kv-pool")]
fn compact_pool_payload(pool: &SharedKVPool, seed: u64) -> Result<Vec<u8>, MemoryError> {
    let shared_codec = pool.manifest.shared_codec.as_bytes();
    let mut payload = Vec::new();
    payload.extend_from_slice(SEMANTIC_MEMORY_PROVEKV_POOL_PAYLOAD_MAGIC_V2);
    payload.push(1);
    payload.extend_from_slice(&(pool.layers.len() as u32).to_le_bytes());
    payload.extend_from_slice(&seed.to_le_bytes());
    push_u16_len_prefixed_bytes(&mut payload, shared_codec)?;
    for layer in &pool.layers {
        payload.extend_from_slice(&layer.layer_index.to_le_bytes());
        payload.extend_from_slice(&(layer.key_blocks.len() as u32).to_le_bytes());
        for block in &layer.key_blocks {
            push_u32_len_prefixed_bytes(&mut payload, &block.encoded_payload)?;
        }
        payload.extend_from_slice(&(layer.value_blocks.len() as u32).to_le_bytes());
        for block in &layer.value_blocks {
            push_u32_len_prefixed_bytes(&mut payload, &block.encoded_payload)?;
        }
    }
    Ok(payload)
}

#[cfg(feature = "poly-kv-pool")]
fn read_exact_bytes<'a>(
    payload: &'a [u8],
    cursor: &mut usize,
    len: usize,
    field: &str,
) -> Result<&'a [u8], MemoryError> {
    let end = cursor.checked_add(len).ok_or_else(|| {
        MemoryError::Other(format!(
            "proveKV compact payload overflow while reading {field}"
        ))
    })?;
    let bytes = payload.get(*cursor..end).ok_or_else(|| {
        MemoryError::Other(format!(
            "proveKV compact payload truncated while reading {field}: need {} bytes at offset {}",
            len, *cursor
        ))
    })?;
    *cursor = end;
    Ok(bytes)
}

#[cfg(feature = "poly-kv-pool")]
fn read_u16_le(payload: &[u8], cursor: &mut usize, field: &str) -> Result<u16, MemoryError> {
    let bytes = read_exact_bytes(payload, cursor, 2, field)?;
    Ok(u16::from_le_bytes(
        bytes.try_into().expect("u16 slice length checked"),
    ))
}

#[cfg(feature = "poly-kv-pool")]
fn read_u32_le(payload: &[u8], cursor: &mut usize, field: &str) -> Result<u32, MemoryError> {
    let bytes = read_exact_bytes(payload, cursor, 4, field)?;
    Ok(u32::from_le_bytes(
        bytes.try_into().expect("u32 slice length checked"),
    ))
}

#[cfg(feature = "poly-kv-pool")]
fn read_u64_le(payload: &[u8], cursor: &mut usize, field: &str) -> Result<u64, MemoryError> {
    let bytes = read_exact_bytes(payload, cursor, 8, field)?;
    Ok(u64::from_le_bytes(
        bytes.try_into().expect("u64 slice length checked"),
    ))
}

#[cfg(feature = "poly-kv-pool")]
pub fn decode_compact_pool_payload(
    payload: &[u8],
    vector_dim: usize,
    item_count: usize,
) -> Result<Vec<Vec<f32>>, MemoryError> {
    if payload.len() < 4 || &payload[..4] != SEMANTIC_MEMORY_PROVEKV_POOL_PAYLOAD_MAGIC_V2 {
        return Err(MemoryError::Other(
            "proveKV compact payload missing SMP2 magic".to_string(),
        ));
    }
    let mut cursor = 4usize;
    let version = *read_exact_bytes(payload, &mut cursor, 1, "version")?
        .first()
        .expect("single-byte version");
    if version != 1 {
        return Err(MemoryError::Other(format!(
            "proveKV compact payload version {version} is unsupported"
        )));
    }
    let layer_count = read_u32_le(payload, &mut cursor, "layer_count")? as usize;
    if layer_count != 1 {
        return Err(MemoryError::Other(format!(
            "semantic-memory proveKV payload expected exactly 1 layer, found {layer_count}"
        )));
    }
    let seed = read_u64_le(payload, &mut cursor, "seed")?;
    let shared_codec_len = read_u16_le(payload, &mut cursor, "shared_codec_len")? as usize;
    let shared_codec = std::str::from_utf8(read_exact_bytes(
        payload,
        &mut cursor,
        shared_codec_len,
        "shared_codec",
    )?)
    .map_err(|err| MemoryError::Other(format!("proveKV shared codec is not valid UTF-8: {err}")))?
    .to_string();

    let layer_index = read_u32_le(payload, &mut cursor, "layer_index")?;
    if layer_index != 0 {
        return Err(MemoryError::Other(format!(
            "semantic-memory proveKV payload expected layer_index 0, found {layer_index}"
        )));
    }
    let key_block_count = read_u32_le(payload, &mut cursor, "key_block_count")? as usize;
    let mut key_payloads = Vec::with_capacity(key_block_count);
    for idx in 0..key_block_count {
        let len = read_u32_le(payload, &mut cursor, &format!("key_block_len[{idx}]"))? as usize;
        key_payloads.push(
            read_exact_bytes(payload, &mut cursor, len, &format!("key_block[{idx}]"))?.to_vec(),
        );
    }
    let value_block_count = read_u32_le(payload, &mut cursor, "value_block_count")? as usize;
    let mut value_payloads = Vec::with_capacity(value_block_count);
    for idx in 0..value_block_count {
        let len = read_u32_le(payload, &mut cursor, &format!("value_block_len[{idx}]"))? as usize;
        value_payloads.push(
            read_exact_bytes(payload, &mut cursor, len, &format!("value_block[{idx}]"))?.to_vec(),
        );
    }
    if cursor != payload.len() {
        return Err(MemoryError::Other(format!(
            "proveKV compact payload has {} trailing bytes",
            payload.len() - cursor
        )));
    }

    let shape = semantic_embedding_shape(vector_dim);
    let head_dim = shape.head_dim;
    let policy = CompressionPolicy::default_two_tier();
    let codec = create_codec(
        &shared_codec,
        head_dim,
        Some(&policy.fib_config),
        Some(&policy.turbo_config),
    )
    .map_err(|err| MemoryError::Other(format!("failed to create proveKV codec: {err}")))?;

    let expected_vectors = item_count;
    let decode_blocks = |block_kind: &str,
                         payloads: &[Vec<u8>]|
     -> Result<Vec<Vec<f32>>, MemoryError> {
        if payloads.len() == 1 {
            if let Some(decoded) =
                codec
                    .decode_batch_compact(&payloads[0], seed)
                    .map_err(|err| {
                        MemoryError::Other(format!("proveKV {block_kind} FB2 decode failed: {err}"))
                    })?
            {
                Ok(decoded)
            } else if expected_vectors == 1 {
                Ok(vec![codec.decode(&payloads[0], seed).map_err(|err| {
                    MemoryError::Other(format!(
                        "proveKV {block_kind} single-block decode failed: {err}"
                    ))
                })?])
            } else {
                Err(MemoryError::Other(format!(
                    "proveKV compact payload stored 1 {block_kind} block for {expected_vectors} vectors without batched decode support"
                )))
            }
        } else {
            let payload_refs: Vec<&[u8]> = payloads.iter().map(|bytes| bytes.as_slice()).collect();
            codec.decode_batch(&payload_refs, seed).map_err(|err| {
                MemoryError::Other(format!("proveKV {block_kind} block decode failed: {err}"))
            })
        }
    };

    let decoded_keys = decode_blocks("key", &key_payloads)?;
    let decoded_values = decode_blocks("value", &value_payloads)?;

    if decoded_keys.len() != expected_vectors {
        return Err(MemoryError::Other(format!(
            "proveKV decoded {} key vectors but item map expects {}",
            decoded_keys.len(),
            expected_vectors
        )));
    }
    if decoded_values.len() != expected_vectors {
        return Err(MemoryError::Other(format!(
            "proveKV decoded {} value vectors but item map expects {}",
            decoded_values.len(),
            expected_vectors
        )));
    }

    let mut embeddings = Vec::with_capacity(decoded_keys.len());
    for (index, (key_vector, value_vector)) in decoded_keys
        .into_iter()
        .zip(decoded_values.into_iter())
        .enumerate()
    {
        let mut combined = key_vector;
        combined.extend(value_vector);
        if combined.len() < vector_dim {
            return Err(MemoryError::Other(format!(
                "proveKV decoded combined vector {index} too short: {} < {vector_dim}",
                combined.len()
            )));
        }
        let candidate = combined[..vector_dim].to_vec();
        crate::db::validate_embedding(&candidate, vector_dim)?;
        embeddings.push(candidate);
    }
    Ok(embeddings)
}

#[cfg(feature = "poly-kv-pool")]
pub(crate) fn load_or_decode_compact_pool_search_cache(
    cache_key: &str,
    payload: &[u8],
    vector_dim: usize,
    item_count: usize,
    item_map: &[ProveKvPoolItemMapEntryV1],
) -> Result<Arc<ProveKvDecodedSearchCache>, MemoryError> {
    if let Ok(cache) = decoded_pool_search_cache().read() {
        if let Some(search_cache) = cache.get(cache_key) {
            return Ok(Arc::clone(search_cache));
        }
    }

    let decoded = decode_compact_pool_payload(payload, vector_dim, item_count)?;
    let search_cache = Arc::new(ProveKvDecodedSearchCache::from_vectors_and_item_map(
        decoded, vector_dim, item_map,
    )?);
    let mut cache = decoded_pool_search_cache()
        .write()
        .map_err(|_| MemoryError::Other("proveKV decoded search cache poisoned".to_string()))?;
    let entry = cache
        .entry(cache_key.to_string())
        .or_insert_with(|| Arc::clone(&search_cache));
    Ok(Arc::clone(entry))
}

/// Build a proveKV/poly-kv generation envelope from an authoritative embedding snapshot.
#[cfg(feature = "poly-kv-pool")]
pub fn build_provekv_pool_generation(
    snapshot: EmbeddingSnapshotV1,
    seed: u64,
) -> Result<
    (
        ProveKvPoolGenerationV1,
        Vec<u8>,
        Vec<ProveKvPoolItemMapEntryV1>,
    ),
    MemoryError,
> {
    if snapshot.rows.is_empty() {
        return Err(MemoryError::Other(
            "cannot build proveKV pool generation from an empty embedding snapshot".to_string(),
        ));
    }
    if snapshot.embedding_snapshot_digest.is_empty() || snapshot.source_digest.is_empty() {
        return Err(MemoryError::Other(
            "proveKV pool snapshot digests must be non-empty".to_string(),
        ));
    }

    let generation_id = uuid::Uuid::new_v4().to_string();
    let mut item_map = Vec::with_capacity(snapshot.rows.len());
    for (pool_index, row) in snapshot.rows.iter().enumerate() {
        item_map.push(ProveKvPoolItemMapEntryV1 {
            generation_id: generation_id.clone(),
            item_id: row.item_id.clone(),
            source_type: row.source_type.clone(),
            pool_index,
            embedding_digest: embedding_row_digest(row, snapshot.vector_dim)?,
        });
    }

    let shape = semantic_embedding_shape(snapshot.vector_dim);
    let corpus = semantic_embedding_corpus(&snapshot);
    let (pool, _receipt) = SharedKVPool::build(&corpus, &shape, seed).map_err(|err| {
        MemoryError::Other(format!("failed to build proveKV/poly-kv pool: {err}"))
    })?;
    let payload = compact_pool_payload(&pool, seed)?;

    let mut manifest_hasher = blake3::Hasher::new();
    manifest_hasher.update(b"semantic-memory.provekv_pool_manifest.v1");
    manifest_hasher.update(&[0]);
    manifest_hasher.update(snapshot.embedding_snapshot_digest.as_bytes());
    manifest_hasher.update(&[0]);
    manifest_hasher.update(snapshot.source_digest.as_bytes());
    manifest_hasher.update(&[0]);
    manifest_hasher.update(&(snapshot.vector_dim as u64).to_le_bytes());
    manifest_hasher.update(&(snapshot.rows.len() as u64).to_le_bytes());
    manifest_hasher.update(&seed.to_le_bytes());
    manifest_hasher.update(&payload);
    for entry in &item_map {
        manifest_hasher.update(entry.item_id.as_bytes());
        manifest_hasher.update(&[0]);
        manifest_hasher.update(entry.source_type.as_bytes());
        manifest_hasher.update(&[0]);
        manifest_hasher.update(&(entry.pool_index as u64).to_le_bytes());
        manifest_hasher.update(entry.embedding_digest.as_bytes());
        manifest_hasher.update(&[0]);
    }
    let pool_manifest_digest = format!("blake3:{}", manifest_hasher.finalize().to_hex());

    let generation = ProveKvPoolGenerationV1 {
        schema_version: "semantic_memory_provekv_pool_generation_v1".to_string(),
        generation_id,
        embedding_snapshot_digest: snapshot.embedding_snapshot_digest,
        source_digest: snapshot.source_digest,
        pool_manifest_digest,
        codec_family: "provekv_pool".to_string(),
        codec_profile: "semantic-memory-f32-derived-candidate-v1".to_string(),
        vector_dim: snapshot.vector_dim,
        item_count: item_map.len(),
        payload_bytes: payload.len() as u64,
        created_at: Utc::now(),
    };
    Ok((generation, payload, item_map))
}

#[cfg(all(test, feature = "poly-kv-pool"))]
mod tests {
    use super::*;

    #[test]
    fn decoded_pool_cache_reuses_materialized_vectors() {
        let snapshot = crate::vector_snapshot::build_embedding_snapshot(
            vec![
                crate::vector_snapshot::EmbeddingSnapshotRow {
                    item_id: "a".to_string(),
                    source_type: "fact".to_string(),
                    embedding: vec![1.0, 0.0, 0.0, 0.0],
                },
                crate::vector_snapshot::EmbeddingSnapshotRow {
                    item_id: "b".to_string(),
                    source_type: "fact".to_string(),
                    embedding: vec![0.0, 1.0, 0.0, 0.0],
                },
            ],
            4,
        )
        .expect("snapshot");
        let (generation, payload, item_map) =
            build_provekv_pool_generation(snapshot, 42).expect("generation");
        let cache_key = format!(
            "test:{}:{}:{}:{}",
            generation.generation_id,
            generation.pool_manifest_digest,
            generation.embedding_snapshot_digest,
            generation.item_count
        );

        let first = load_or_decode_compact_pool_search_cache(
            &cache_key,
            &payload,
            generation.vector_dim,
            generation.item_count,
            &item_map,
        )
        .expect("first decode");
        let second = load_or_decode_compact_pool_search_cache(
            &cache_key,
            &payload,
            generation.vector_dim,
            generation.item_count,
            &item_map,
        )
        .expect("second decode");

        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.row_count, generation.item_count);
        assert_eq!(first.dim, generation.vector_dim);
        assert_eq!(
            first.vectors_flat.len(),
            generation.item_count * generation.vector_dim
        );
    }
}
