//! Deterministic snapshots over authoritative SQLite f32 embeddings.
//!
//! These snapshots are used only to build/check rebuildable derived candidate artifacts. The
//! authoritative vectors remain the f32 embeddings in SQLite.

use crate::db;
use crate::error::MemoryError;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

/// One authoritative embedding row included in a derived candidate snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingSnapshotRow {
    pub item_id: String,
    pub source_type: String,
    pub embedding: Vec<f32>,
}

/// Stable deterministic digest envelope for authoritative f32 embedding rows.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EmbeddingSnapshotV1 {
    pub embedding_snapshot_digest: String,
    pub source_digest: String,
    pub vector_dim: usize,
    pub rows: Vec<EmbeddingSnapshotRow>,
}

fn digest_hex(
    domain: &str,
    vector_dim: usize,
    rows: &[EmbeddingSnapshotRow],
    include_embeddings: bool,
) -> String {
    let mut sorted = rows.to_vec();
    sorted.sort_by(|a, b| {
        (a.source_type.as_str(), a.item_id.as_str())
            .cmp(&(b.source_type.as_str(), b.item_id.as_str()))
    });

    let mut hasher = blake3::Hasher::new();
    hasher.update(domain.as_bytes());
    hasher.update(&[0]);
    hasher.update(&(vector_dim as u64).to_le_bytes());
    hasher.update(&[0]);
    for row in &sorted {
        hasher.update(row.source_type.as_bytes());
        hasher.update(&[0]);
        hasher.update(row.item_id.as_bytes());
        hasher.update(&[0]);
        if include_embeddings {
            for value in &row.embedding {
                hasher.update(&value.to_le_bytes());
            }
            hasher.update(&[0]);
        }
    }
    format!("blake3:{}", hasher.finalize().to_hex())
}

/// Build a stable snapshot from in-memory authoritative f32 rows.
pub fn build_embedding_snapshot(
    rows: Vec<EmbeddingSnapshotRow>,
    vector_dim: usize,
) -> Result<EmbeddingSnapshotV1, MemoryError> {
    for row in &rows {
        if row.item_id.is_empty() || row.source_type.is_empty() {
            return Err(MemoryError::Other(
                "embedding snapshot rows require non-empty item_id and source_type".to_string(),
            ));
        }
        if row.embedding.len() != vector_dim {
            return Err(MemoryError::DimensionMismatch {
                expected: vector_dim,
                actual: row.embedding.len(),
            });
        }
    }
    Ok(EmbeddingSnapshotV1 {
        embedding_snapshot_digest: digest_hex(
            "semantic-memory.embedding_snapshot.v1",
            vector_dim,
            &rows,
            true,
        ),
        source_digest: digest_hex(
            "semantic-memory.embedding_source.v1",
            vector_dim,
            &rows,
            false,
        ),
        vector_dim,
        rows,
    })
}

/// Load all authoritative embeddings from SQLite and build a deterministic snapshot.
pub fn load_embedding_snapshot_from_db(
    conn: &Connection,
    vector_dim: usize,
) -> Result<EmbeddingSnapshotV1, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, 'fact', embedding FROM facts WHERE embedding IS NOT NULL
         UNION ALL
         SELECT id, 'chunk', embedding FROM chunks WHERE embedding IS NOT NULL
         UNION ALL
         SELECT CAST(id AS TEXT), 'message', embedding FROM messages WHERE embedding IS NOT NULL
         UNION ALL
         SELECT episode_id, 'episode', embedding FROM episodes WHERE embedding IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Vec<u8>>(2)?,
        ))
    })?;

    let mut snapshot_rows = Vec::new();
    for row in rows {
        let (item_id, source_type, blob) = row?;
        snapshot_rows.push(EmbeddingSnapshotRow {
            item_id,
            source_type,
            embedding: db::decode_f32_le(&blob, vector_dim)?,
        });
    }
    build_embedding_snapshot(snapshot_rows, vector_dim)
}

/// Stable digest for a single authoritative embedding row.
pub fn embedding_row_digest(
    row: &EmbeddingSnapshotRow,
    vector_dim: usize,
) -> Result<String, MemoryError> {
    Ok(build_embedding_snapshot(vec![row.clone()], vector_dim)?.embedding_snapshot_digest)
}
