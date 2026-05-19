//! HNSW sidecar lifecycle helpers.

#[cfg(feature = "hnsw")]
use crate::episodes;
#[cfg(feature = "hnsw")]
use crate::error::MemoryError;
#[cfg(feature = "hnsw")]
use crate::StoragePaths;
#[cfg(feature = "hnsw")]
use crate::{db, pool::SqlitePool, MemoryStoreInner};
#[cfg(feature = "hnsw")]
use crate::{hnsw::HnswConfig, hnsw::HnswIndex};
#[cfg(feature = "hnsw")]
use rusqlite::Connection;

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
) -> Result<HnswIndex, MemoryError> {
    let new_index = HnswIndex::new(config.clone())?;
    let mut skipped_invalid = 0usize;

    // Load fact embeddings
    {
        let mut stmt =
            conn.prepare("SELECT id, embedding FROM facts WHERE embedding IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (id, blob) = row?;
            match db::decode_f32_le(&blob, config.dimensions) {
                Ok(emb) => {
                    let key = format!("fact:{}", id);
                    if let Err(e) = new_index.insert(key.clone(), &emb) {
                        tracing::warn!("Failed to insert {} into HNSW: {}", key, e);
                    }
                }
                Err(error) => {
                    skipped_invalid += 1;
                    tracing::warn!(error = %error, id, "Skipping invalid fact embedding during HNSW rebuild");
                }
            }
        }
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

    Ok(new_index)
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
            Ok(rebuilt) => {
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
    save_hnsw_sidecar(&recovered, &paths.hnsw_dir, &paths.hnsw_basename)?;
    pool.with_write_conn(|conn| {
        recovered.flush_keymap(conn)?;
        db::clear_all_pending_index_ops(conn)?;
        db::set_sidecar_dirty(conn, false)?;
        Ok(())
    })?;
    Ok(recovered)
}
