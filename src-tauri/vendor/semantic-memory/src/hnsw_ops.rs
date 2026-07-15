//! HNSW sidecar lifecycle helpers.

#[cfg(feature = "hnsw")]
use crate::episodes;
#[cfg(feature = "hnsw")]
use crate::error::MemoryError;
#[cfg(feature = "hnsw")]
use crate::types::VectorArtifactBuildReceiptV1;
#[cfg(feature = "hnsw")]
use crate::StoragePaths;
#[cfg(feature = "hnsw")]
use crate::{db, pool::SqlitePool, MemoryStoreInner};
#[cfg(feature = "hnsw")]
use crate::{hnsw::HnswConfig, hnsw::HnswIndex};
#[cfg(feature = "hnsw")]
use rusqlite::Connection;
#[cfg(feature = "hnsw")]
use std::collections::BTreeMap;

#[cfg(feature = "hnsw")]
enum PendingIndexMutation {
    Upsert {
        item_key: String,
        embedding: Vec<f32>,
    },
    Delete {
        item_key: String,
    },
}

#[cfg(feature = "hnsw")]
pub(crate) fn ensure_hnsw_dir(dir: &std::path::Path) -> Result<(), MemoryError> {
    std::fs::create_dir_all(dir).map_err(|err| {
        MemoryError::StorageError(format!(
            "failed to create HNSW directory {}: {}",
            dir.display(),
            err
        ))
    })
}

#[cfg(feature = "hnsw")]
pub(crate) fn save_hnsw_sidecar(
    index: &HnswIndex,
    dir: &std::path::Path,
    basename: &str,
) -> Result<(), MemoryError> {
    ensure_hnsw_dir(dir)?;
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| index.save(dir, basename))).map_err(
        |_| {
            MemoryError::HnswError(format!(
                "failed to save HNSW sidecar under {}: underlying hnsw writer panicked",
                dir.display()
            ))
        },
    )?
}

#[cfg(feature = "hnsw")]
pub(crate) fn rebuild_hnsw_from_sqlite(
    conn: &Connection,
    config: &HnswConfig,
) -> Result<(HnswIndex, VectorArtifactBuildReceiptV1), MemoryError> {
    use chrono::{DateTime, Utc};

    let started = std::time::Instant::now();
    let new_index = HnswIndex::new(config.clone())?;
    let mut skipped_invalid = 0usize;

    // Count authoritative source rows for the receipt.
    let fact_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM facts WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let chunk_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let message_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM messages WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let episode_count: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE embedding IS NOT NULL",
            [],
            |r| r.get(0),
        )
        .unwrap_or(0);
    let source_row_count = fact_count + chunk_count + message_count + episode_count;
    let build_receipt_id = uuid::Uuid::new_v4().to_string();

    // Collect per-source-row inserts for deterministic artifact manifest digest.
    // BTreeMap gives canonical insertion order regardless of DB iteration order.
    let mut fact_inserts = BTreeMap::new();
    let mut chunk_inserts = BTreeMap::new();
    let mut message_inserts = BTreeMap::new();
    let mut episode_inserts = BTreeMap::new();

    // Load fact embeddings
    {
        let mut stmt =
            conn.prepare("SELECT id, embedding FROM facts WHERE embedding IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        let mut fact_count_inserted = 0usize;
        for row in rows {
            let (id, blob) = row?;
            match db::decode_f32_le(&blob, config.dimensions) {
                Ok(emb) => {
                    let key = format!("fact:{}", id);
                    if let Err(e) = new_index.insert(key.clone(), &emb) {
                        tracing::warn!("Failed to insert {} into HNSW: {}", key, e);
                    } else {
                        fact_inserts.insert(key, ());
                        fact_count_inserted += 1;
                    }
                }
                Err(error) => {
                    skipped_invalid += 1;
                    tracing::warn!(error = %error, id, "Skipping invalid fact embedding during HNSW rebuild");
                }
            }
        }
        let _ = fact_count_inserted; // suppress unused warning
    }

    // Load chunk embeddings
    {
        let mut stmt =
            conn.prepare("SELECT id, embedding FROM chunks WHERE embedding IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (id, blob) = row?;
            match db::decode_f32_le(&blob, config.dimensions) {
                Ok(emb) => {
                    let key = format!("chunk:{}", id);
                    if let Err(e) = new_index.insert(key.clone(), &emb) {
                        tracing::warn!("Failed to insert {} into HNSW: {}", key, e);
                    } else {
                        chunk_inserts.insert(key, ());
                    }
                }
                Err(error) => {
                    skipped_invalid += 1;
                    tracing::warn!(error = %error, id, "Skipping invalid chunk embedding during HNSW rebuild");
                }
            }
        }
    }

    // Load message embeddings
    {
        let mut stmt =
            conn.prepare("SELECT id, embedding FROM messages WHERE embedding IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (id, blob) = row?;
            match db::decode_f32_le(&blob, config.dimensions) {
                Ok(emb) => {
                    let key = format!("msg:{}", id);
                    if let Err(e) = new_index.insert(key.clone(), &emb) {
                        tracing::warn!("Failed to insert {} into HNSW: {}", key, e);
                    } else {
                        message_inserts.insert(key, ());
                    }
                }
                Err(error) => {
                    skipped_invalid += 1;
                    tracing::warn!(error = %error, id, "Skipping invalid message embedding during HNSW rebuild");
                }
            }
        }
    }

    // Load episode embeddings (keyed by episode_id)
    {
        let mut stmt =
            conn.prepare("SELECT episode_id, embedding FROM episodes WHERE embedding IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (episode_id, blob) = row?;
            match db::decode_f32_le(&blob, config.dimensions) {
                Ok(emb) => {
                    let key = episodes::episode_item_key(&episode_id);
                    if let Err(e) = new_index.insert(key.clone(), &emb) {
                        tracing::warn!("Failed to insert {} into HNSW: {}", key, e);
                    } else {
                        episode_inserts.insert(key, ());
                    }
                }
                Err(error) => {
                    skipped_invalid += 1;
                    tracing::warn!(error = %error, episode_id, "Skipping invalid episode embedding during HNSW rebuild");
                }
            }
        }
    }

    if skipped_invalid > 0 {
        return Err(MemoryError::HnswError(format!(
            "HNSW rebuild skipped {skipped_invalid} invalid embedding rows"
        )));
    }

    let artifact_count =
        fact_inserts.len() + chunk_inserts.len() + message_inserts.len() + episode_inserts.len();

    // Build deterministic codec profile digest from HnswConfig fields.
    let codec_profile_digest = format!(
        "hnsw:dim={},m={},ef_construction={},max_elements={}",
        config.dimensions, config.m, config.ef_construction, config.max_elements
    );

    // Deterministic artifact manifest digest from canonical BTreeMap key ordering.
    let artifact_manifest_digest = format!(
        "hnsw-manifest:v1:facts={},chunks={},msgs={},episodes={}:{}",
        fact_inserts.len(),
        chunk_inserts.len(),
        message_inserts.len(),
        episode_inserts.len(),
        serde_json::to_string(&(
            fact_inserts.keys().take(100).collect::<Vec<_>>(),
            chunk_inserts.keys().take(100).collect::<Vec<_>>(),
        ))
        .unwrap_or_default()
    );

    let receipt = VectorArtifactBuildReceiptV1 {
        schema_version: "vector_artifact_build_receipt_v1".to_string(),
        codec_family: "hnsw".to_string(),
        codec_profile_digest,
        source_row_count,
        artifact_count,
        generation_id: None,
        source_snapshot_digest: None,
        artifact_manifest_digest: Some(artifact_manifest_digest),
        build_receipt_id: Some(build_receipt_id),
        skipped_row_count: skipped_invalid,
        elapsed_ms: started.elapsed().as_millis(),
        created_at: DateTime::<Utc>::from(std::time::SystemTime::now()),
        degradations: vec![],
    };

    Ok((new_index, receipt))
}

#[cfg(feature = "hnsw")]
pub(crate) fn sync_pending_hnsw_sidecar(inner: &MemoryStoreInner) -> Result<usize, MemoryError> {
    let pending_ops = inner.pool.with_read_conn(db::list_pending_index_ops)?;
    if pending_ops.is_empty() {
        let dirty = inner.pool.with_read_conn(db::is_sidecar_dirty)?;
        if !dirty {
            return Ok(0);
        }

        let index = inner.hnsw_index.write().unwrap_or_else(|e| e.into_inner());
        save_hnsw_sidecar(&index, &inner.paths.hnsw_dir, &inner.paths.hnsw_basename)?;
        inner.pool.with_write_conn(|conn| {
            index.flush_keymap(conn)?;
            index.update_last_flush_epoch();
            db::set_sidecar_dirty(conn, false)?;
            Ok(())
        })?;
        return Ok(0);
    }

    let mutations = inner.pool.with_read_conn(|conn| {
        let mut mutations = Vec::with_capacity(pending_ops.len());
        for op in &pending_ops {
            match op.op_kind {
                db::IndexOpKind::Upsert => {
                    match db::load_embedding_for_index_key(conn, &op.item_key)? {
                        Some(embedding) => mutations.push(PendingIndexMutation::Upsert {
                            item_key: op.item_key.clone(),
                            embedding,
                        }),
                        None => mutations.push(PendingIndexMutation::Delete {
                            item_key: op.item_key.clone(),
                        }),
                    }
                }
                db::IndexOpKind::Delete => mutations.push(PendingIndexMutation::Delete {
                    item_key: op.item_key.clone(),
                }),
            }
        }
        Ok::<_, MemoryError>(mutations)
    })?;

    let result: Result<usize, MemoryError> = (|| {
        let processed_keys: Vec<String> =
            pending_ops.iter().map(|op| op.item_key.clone()).collect();
        {
            let guard = inner.hnsw_index.write().unwrap_or_else(|e| e.into_inner());
            for mutation in &mutations {
                match mutation {
                    PendingIndexMutation::Upsert {
                        item_key,
                        embedding,
                    } => guard.update(item_key.clone(), embedding)?,
                    PendingIndexMutation::Delete { item_key } => guard.delete(item_key)?,
                }
            }
            save_hnsw_sidecar(&guard, &inner.paths.hnsw_dir, &inner.paths.hnsw_basename)?;
            inner.pool.with_write_conn(|conn| {
                guard.flush_keymap(conn)?;
                guard.update_last_flush_epoch();
                db::clear_pending_index_ops(conn, &processed_keys)?;
                db::set_sidecar_dirty(conn, false)?;
                Ok(())
            })?;
        }
        Ok(pending_ops.len())
    })();

    if let Err(err) = result {
        let err_text = err.to_string();
        let keys: Vec<String> = pending_ops.iter().map(|op| op.item_key.clone()).collect();
        if let Err(mark_err) = inner
            .pool
            .with_write_conn(|conn| db::mark_pending_index_ops_failed(conn, &keys, &err_text))
        {
            tracing::warn!(
                error = %mark_err,
                "failed to mark pending HNSW index ops as failed"
            );
        }
        match inner
            .pool
            .with_read_conn(|conn| rebuild_hnsw_from_sqlite(conn, &inner.config.hnsw))
        {
            Ok((rebuilt, _receipt)) => {
                *inner.hnsw_index.write().unwrap_or_else(|e| e.into_inner()) = rebuilt;
                tracing::warn!(
                    error = %err,
                    "discarded failed in-memory HNSW mutation state and rebuilt from SQLite; sidecar remains dirty until a later successful flush"
                );
            }
            Err(rebuild_err) => {
                match HnswIndex::new(inner.config.hnsw.clone()) {
                    Ok(empty) => {
                        *inner.hnsw_index.write().unwrap_or_else(|e| e.into_inner()) = empty;
                    }
                    Err(new_err) => {
                        tracing::error!(
                            error = %new_err,
                            "failed to create empty HNSW fallback after sidecar sync failure"
                        );
                    }
                }
                tracing::warn!(
                    error = %err,
                    rebuild_error = %rebuild_err,
                    "failed HNSW sidecar sync could not be rebuilt in memory; HNSW will under-return and exact search fallback remains authoritative"
                );
            }
        }
        return Err(err);
    }

    result
}

#[cfg(feature = "hnsw")]
pub(crate) fn recover_hnsw_sidecar_sync(
    pool: &SqlitePool,
    paths: &StoragePaths,
    config: &HnswConfig,
) -> Result<HnswIndex, MemoryError> {
    let recovered = pool.with_read_conn(|conn| rebuild_hnsw_from_sqlite(conn, config))?;
    let (recovered_index, _receipt) = recovered;
    save_hnsw_sidecar(&recovered_index, &paths.hnsw_dir, &paths.hnsw_basename)?;
    pool.with_write_conn(|conn| {
        recovered_index.flush_keymap(conn)?;
        db::clear_all_pending_index_ops(conn)?;
        db::set_sidecar_dirty(conn, false)?;
        Ok(())
    })?;
    Ok(recovered_index)
}
