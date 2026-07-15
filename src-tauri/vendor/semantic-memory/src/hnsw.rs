//! HNSW approximate nearest-neighbor index wrapper.
//!
//! SQLite remains the source of truth. The on-disk HNSW files are a recoverable
//! acceleration sidecar that can be rebuilt from SQLite whenever needed.
//!
//! ## Backend selection (2026-06-02)
//!
//! This file is the **hnsw_rs 0.3 backend** implementation of the
//! [`VectorBackend`] trait (see `vector_backend.rs`). It is the default
//! backend as of 2026-06-02 but is being phased out in favor of the
//! usearch 2.25 backend (see `usearch_backend.rs`). Both backends implement
//! the same trait so that callers don't need to change.

use crate::db;
use crate::error::MemoryError;
use crate::vector_backend::{VectorBackend, VectorHit, VectorIndexConfig};
use hnsw_rs::prelude::*;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use stack_ids::ContentDigest;
use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

const HNSW_DATA_MAGIC: u32 = 0x534d_4844; // "SMHD"
const HNSW_GRAPH_MAGIC: u32 = 0x534d_4847; // "SMHG"
const HNSW_SIDECAR_VERSION: u16 = 1;
const HNSW_SIDECAR_HEADER_LEN: u16 = 24;
const HNSW_MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Configuration for the HNSW index. Type alias to the canonical
/// `VectorIndexConfig` (in `vector_backend.rs`) so the same config can be
/// used regardless of which backend is active. Field access is preserved.
pub type HnswConfig = VectorIndexConfig;

/// A single hit from HNSW search. Type alias to the canonical
/// `VectorHit`. Field access is preserved.
pub type HnswHit = VectorHit;

// Note: the previous HnswConfig struct + impl Default block lived here.
// It has been moved to `vector_backend.rs` as the canonical
// `VectorIndexConfig` + `impl Default for VectorIndexConfig`.
// Existing call sites continue to work via the `HnswConfig` type alias.

struct HnswIndexInner {
    graph: Hnsw<'static, f32, DistCosine>,
    keymap: RwLock<KeyMapState>,
    next_id: AtomicUsize,
    keymap_dirty: AtomicBool,
    last_flush_epoch: AtomicU64,
    config: HnswConfig,
}

#[derive(Debug, Default, Clone)]
struct KeyMapState {
    // CONVENTION EXCEPTION: O(1) lookup required for HNSW index
    key_to_id: HashMap<String, usize>,
    // CONVENTION EXCEPTION: O(1) lookup required for HNSW index
    id_to_key: HashMap<usize, String>,
    // CONVENTION EXCEPTION: sidecar persistence needs stable node-id to vector replay
    id_to_vector: HashMap<usize, Vec<f32>>,
    deleted_ids: HashSet<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SidecarHeader {
    magic: u32,
    version: u16,
    header_len: u16,
    dim: u32,
    vector_count: u64,
    flags: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct HnswSidecarManifestV1 {
    schema_version: u32,
    generation_id: String,
    basename: String,
    graph_file_name: String,
    data_file_name: String,
    graph_digest: String,
    data_digest: String,
    dimensions: usize,
    vector_count: u64,
    hnsw_sidecar_format_version: u16,
    source_sqlite_epoch: Option<u64>,
    created_at: String,
}

fn current_epoch_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[derive(Clone)]
pub struct HnswIndex {
    inner: Arc<HnswIndexInner>,
}

impl HnswIndex {
    pub fn new(config: HnswConfig) -> Result<Self, MemoryError> {
        let graph: Hnsw<'static, f32, DistCosine> = Hnsw::new(
            config.m,
            config.max_elements,
            16,
            config.ef_construction,
            DistCosine {},
        );

        Ok(Self {
            inner: Arc::new(HnswIndexInner {
                graph,
                keymap: RwLock::new(KeyMapState::default()),
                next_id: AtomicUsize::new(0),
                keymap_dirty: AtomicBool::new(false),
                last_flush_epoch: AtomicU64::new(current_epoch_secs()),
                config,
            }),
        })
    }

    /// Load a previously flushed HNSW sidecar by replaying the dumped vectors.
    ///
    /// This avoids relying on `hnsw_rs`'s borrowing reload API and keeps the safety
    /// boundary purely in safe Rust. Node IDs are preserved so the SQLite keymap can
    /// be loaded afterward.
    pub fn load(dir: &Path, basename: &str, config: HnswConfig) -> Result<Self, MemoryError> {
        let data_path = dir.join(format!("{}.hnsw.data", basename));
        let graph_path = dir.join(format!("{}.hnsw.graph", basename));
        if !data_path.exists() || !graph_path.exists() {
            return Err(MemoryError::HnswError(format!(
                "missing HNSW sidecar files under {}",
                dir.display()
            )));
        }

        let index = Self::new(config)?;
        let manifest = validate_hnsw_manifest(dir, basename, &index.inner.config)?;
        let graph_header = validate_graph_sidecar(&graph_path)?;
        if let Some(manifest) = &manifest {
            validate_manifest_against_header(manifest, &graph_header)?;
        }
        let max_id = load_vectors_from_sidecar(&index, &data_path, &graph_header)?;
        index
            .inner
            .next_id
            .store(max_id.saturating_add(1), Ordering::SeqCst);
        Ok(index)
    }

    pub fn save(&self, dir: &Path, basename: &str) -> Result<(), MemoryError> {
        std::fs::create_dir_all(dir).map_err(|e| {
            MemoryError::HnswError(format!(
                "failed to create HNSW dir {}: {}",
                dir.display(),
                e
            ))
        })?;
        let tmp_basename = format!(".{}.tmp-{}", basename, std::process::id());

        let vectors = self.sidecar_vectors()?;
        let vector_count = u64::try_from(vectors.len()).map_err(|_| {
            MemoryError::HnswError("HNSW sidecar vector count exceeds u64".to_string())
        })?;

        let graph_tmp = dir.join(format!("{}.hnsw.graph", tmp_basename));
        let data_tmp = dir.join(format!("{}.hnsw.data", tmp_basename));
        let manifest_tmp = dir.join(format!("{}.hnsw.manifest.json", tmp_basename));
        let graph_final = dir.join(format!("{}.hnsw.graph", basename));
        let data_final = dir.join(format!("{}.hnsw.data", basename));
        let manifest_final = hnsw_manifest_path(dir, basename);
        write_graph_sidecar(&graph_tmp, self.inner.config.dimensions, vector_count)?;
        write_data_sidecar(
            &data_tmp,
            self.inner.config.dimensions,
            vector_count,
            &vectors,
        )?;

        let graph_digest = file_digest(&graph_tmp)?;
        let data_digest = file_digest(&data_tmp)?;
        atomically_replace_sidecar(&graph_tmp, &graph_final)?;
        atomically_replace_sidecar(&data_tmp, &data_final)?;
        write_hnsw_manifest(
            &manifest_tmp,
            HnswSidecarManifestV1 {
                schema_version: HNSW_MANIFEST_SCHEMA_VERSION,
                generation_id: uuid::Uuid::new_v4().to_string(),
                basename: basename.to_string(),
                graph_file_name: hnsw_graph_file_name(basename),
                data_file_name: hnsw_data_file_name(basename),
                graph_digest,
                data_digest,
                dimensions: self.inner.config.dimensions,
                vector_count,
                hnsw_sidecar_format_version: HNSW_SIDECAR_VERSION,
                source_sqlite_epoch: Some(current_epoch_secs()),
                created_at: chrono::Utc::now().to_rfc3339(),
            },
        )?;
        atomically_replace_sidecar(&manifest_tmp, &manifest_final)?;
        if let Ok(dir_file) = File::open(dir) {
            let _ = dir_file.sync_all();
        }
        Ok(())
    }

    pub fn insert(&self, key: String, vector: &[f32]) -> Result<(), MemoryError> {
        let id = self.allocate_id()?;
        self.insert_with_id(Some(key), id, vector)
    }

    pub fn delete(&self, key: &str) -> Result<(), MemoryError> {
        let mut keymap = self.inner.keymap.write().unwrap_or_else(|e| e.into_inner());

        if let Some(id) = keymap.key_to_id.remove(key) {
            keymap.id_to_key.remove(&id);
            keymap.deleted_ids.insert(id);
            self.inner.keymap_dirty.store(true, Ordering::SeqCst);
        }
        Ok(())
    }

    pub fn update(&self, key: String, vector: &[f32]) -> Result<(), MemoryError> {
        self.delete(&key)?;
        self.insert(key, vector)
    }

    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<HnswHit>, MemoryError> {
        validate_dimensions(query, self.inner.config.dimensions)?;

        if self.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }

        let keymap_snapshot = self
            .inner
            .keymap
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let total_points = self.inner.graph.get_nb_point();
        let fetch_count = top_k
            .saturating_add(keymap_snapshot.deleted_ids.len())
            .min(total_points);

        let neighbors = self
            .inner
            .graph
            .search(query, fetch_count, self.inner.config.ef_search);

        let mut hits: Vec<HnswHit> = neighbors
            .into_iter()
            .filter(|neighbor| !keymap_snapshot.deleted_ids.contains(&neighbor.d_id))
            .filter_map(|neighbor| {
                keymap_snapshot
                    .id_to_key
                    .get(&neighbor.d_id)
                    .map(|key| HnswHit {
                        key: key.clone(),
                        distance: neighbor.distance,
                    })
            })
            .take(top_k)
            .collect();

        if hits.len() < top_k && keymap_snapshot.key_to_id.len() >= top_k {
            tracing::warn!(
                requested = top_k,
                returned = hits.len(),
                active_keys = keymap_snapshot.key_to_id.len(),
                "HNSW filtered under-return detected; caller should fall back to exact vector search"
            );
            return Ok(Vec::new());
        }

        hits.sort_by(|a, b| {
            a.distance.partial_cmp(&b.distance).unwrap_or_else(|| {
                // LIB-020: NaN distances sort to the end rather than comparing as equal
                if a.distance.is_nan() {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Less
                }
            })
        });
        Ok(hits)
    }

    pub fn len(&self) -> usize {
        let total = self.inner.graph.get_nb_point();
        let deleted = self
            .inner
            .keymap
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .deleted_ids
            .len();
        total.saturating_sub(deleted)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn deleted_ratio(&self) -> f32 {
        let total = self.inner.graph.get_nb_point();
        if total == 0 {
            return 0.0;
        }
        let deleted = self
            .inner
            .keymap
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .deleted_ids
            .len();
        deleted as f32 / total as f32
    }

    pub fn needs_compaction(&self) -> bool {
        self.deleted_ratio() > self.inner.config.compaction_threshold
    }

    pub fn config(&self) -> &HnswConfig {
        &self.inner.config
    }

    pub fn is_keymap_dirty(&self) -> bool {
        self.inner.keymap_dirty.load(Ordering::SeqCst)
    }

    pub fn should_flush(&self, interval_secs: u64) -> bool {
        let last = self.inner.last_flush_epoch.load(Ordering::SeqCst);
        current_epoch_secs().saturating_sub(last) >= interval_secs
    }

    pub fn update_last_flush_epoch(&self) {
        self.inner
            .last_flush_epoch
            .store(current_epoch_secs(), Ordering::SeqCst);
    }

    pub fn flush_keymap(&self, conn: &rusqlite::Connection) -> Result<(), MemoryError> {
        if !self.is_keymap_dirty() {
            return Ok(());
        }

        let keymap = self.inner.keymap.read().unwrap_or_else(|e| e.into_inner());
        let next_id = self.inner.next_id.load(Ordering::SeqCst);

        db::with_transaction(conn, |tx| {
            tx.execute("DELETE FROM hnsw_keymap", [])?;
            let mut insert_stmt = tx.prepare(
                "INSERT INTO hnsw_keymap (node_id, item_key, deleted) VALUES (?1, ?2, ?3)",
            )?;

            for (key, id) in keymap.key_to_id.iter() {
                insert_stmt.execute(params![*id as i64, key, 0])?;
            }
            for id in keymap.deleted_ids.iter() {
                insert_stmt.execute(params![*id as i64, format!("_deleted:{}", id), 1])?;
            }
            drop(insert_stmt);

            tx.execute(
                "INSERT INTO hnsw_metadata (key, value) VALUES ('next_id', ?1)
                 ON CONFLICT(key) DO UPDATE SET value = excluded.value",
                params![next_id.to_string()],
            )?;
            Ok(())
        })?;

        self.inner.keymap_dirty.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub fn load_keymap(&self, conn: &rusqlite::Connection) -> Result<(), MemoryError> {
        let table_exists: bool = conn.query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='hnsw_keymap'",
            [],
            |row| row.get(0),
        )?;
        if !table_exists {
            return Err(MemoryError::HnswError(
                "hnsw_keymap table missing while HNSW sidecar exists".to_string(),
            ));
        }

        // CONVENTION EXCEPTION: O(1) lookup required for HNSW index
        let mut key_to_id = HashMap::new();
        // CONVENTION EXCEPTION: O(1) lookup required for HNSW index
        let mut id_to_key = HashMap::new();
        let mut deleted_ids = HashSet::new();

        let mut stmt = conn.prepare("SELECT node_id, item_key, deleted FROM hnsw_keymap")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                usize::try_from(row.get::<_, i64>(0)?).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        0,
                        rusqlite::types::Type::Integer,
                        Box::new(e),
                    )
                })?,
                row.get::<_, String>(1)?,
                row.get::<_, bool>(2)?,
            ))
        })?;

        for row in rows {
            let (node_id, item_key, deleted) = row?;
            if !deleted && node_id >= self.inner.next_id.load(Ordering::SeqCst) {
                return Err(MemoryError::HnswError(format!(
                    "hnsw_keymap node_id {node_id} is outside loaded HNSW sidecar bounds"
                )));
            }
            if deleted {
                deleted_ids.insert(node_id);
            } else {
                let has_vector = self
                    .inner
                    .keymap
                    .read()
                    .unwrap_or_else(|e| e.into_inner())
                    .id_to_vector
                    .contains_key(&node_id);
                if !has_vector {
                    return Err(MemoryError::HnswError(format!(
                        "hnsw_keymap node_id {node_id} has no vector in loaded sidecar"
                    )));
                }
                key_to_id.insert(item_key.clone(), node_id);
                id_to_key.insert(node_id, item_key);
            }
        }

        let next_id = match conn.query_row(
            "SELECT value FROM hnsw_metadata WHERE key = 'next_id'",
            [],
            |row| row.get::<_, String>(0),
        ) {
            Ok(value) => value.parse::<usize>().map_err(|e| {
                MemoryError::HnswError(format!("malformed hnsw next_id metadata '{value}': {e}"))
            })?,
            Err(rusqlite::Error::QueryReturnedNoRows) => self.inner.graph.get_nb_point(),
            Err(error) => return Err(error.into()),
        };

        let id_to_vector = self
            .inner
            .keymap
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .id_to_vector
            .clone();
        *self.inner.keymap.write().unwrap_or_else(|e| e.into_inner()) = KeyMapState {
            key_to_id,
            id_to_key,
            id_to_vector,
            deleted_ids,
        };
        self.inner.next_id.store(next_id, Ordering::SeqCst);
        self.inner.keymap_dirty.store(false, Ordering::SeqCst);

        Ok(())
    }

    fn insert_with_id(
        &self,
        key: Option<String>,
        id: usize,
        vector: &[f32],
    ) -> Result<(), MemoryError> {
        validate_dimensions(vector, self.inner.config.dimensions)?;

        if let Some(key) = key {
            self.inner.graph.insert((vector, id));

            let mut keymap = self.inner.keymap.write().unwrap_or_else(|e| e.into_inner());

            if let Some(old_id) = keymap.key_to_id.insert(key.clone(), id) {
                keymap.id_to_key.remove(&old_id);
                keymap.id_to_vector.remove(&old_id);
                keymap.deleted_ids.insert(old_id);
            }
            keymap.id_to_key.insert(id, key);
            keymap.id_to_vector.insert(id, vector.to_vec());
            self.inner.keymap_dirty.store(true, Ordering::SeqCst);
        } else {
            self.inner.graph.insert((vector, id));
            self.inner
                .keymap
                .write()
                .unwrap_or_else(|e| e.into_inner())
                .id_to_vector
                .insert(id, vector.to_vec());
        }
        Ok(())
    }

    fn allocate_id(&self) -> Result<usize, MemoryError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        if id >= self.inner.config.max_elements {
            self.inner.next_id.fetch_sub(1, Ordering::SeqCst);
            return Err(MemoryError::HnswError(format!(
                "HNSW id space exhausted at max_elements={}; compact or rebuild sidecar before inserting more vectors",
                self.inner.config.max_elements
            )));
        }
        Ok(id)
    }

    pub(crate) fn vector_snapshot(&self) -> HashMap<usize, Vec<f32>> {
        self.inner
            .keymap
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .id_to_vector
            .clone()
    }

    fn sidecar_vectors(&self) -> Result<Vec<(usize, Vec<f32>)>, MemoryError> {
        let keymap = self.inner.keymap.read().unwrap_or_else(|e| e.into_inner());
        let mut vectors = Vec::with_capacity(keymap.id_to_key.len());
        for id in keymap.id_to_key.keys() {
            if keymap.deleted_ids.contains(id) {
                continue;
            }
            let vector = keymap.id_to_vector.get(id).ok_or_else(|| {
                MemoryError::HnswError(format!(
                    "HNSW node {id} has an active key but no retained vector for sidecar save"
                ))
            })?;
            db::validate_embedding(vector, self.inner.config.dimensions)?;
            vectors.push((*id, vector.clone()));
        }
        vectors.sort_by_key(|(id, _)| *id);
        Ok(vectors)
    }
}

// =====================================================================
// VectorBackend trait implementation for the hnsw_rs 0.3 wrapper.
// =====================================================================

impl VectorBackend for HnswIndex {
    fn insert(&self, key: String, vector: &[f32]) -> Result<(), MemoryError> {
        HnswIndex::insert(self, key, vector)
    }

    fn delete(&self, key: &str) -> Result<(), MemoryError> {
        HnswIndex::delete(self, key)
    }

    fn update(&self, key: String, vector: &[f32]) -> Result<(), MemoryError> {
        HnswIndex::update(self, key, vector)
    }

    fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<VectorHit>, MemoryError> {
        // The hnsw_rs search returns `Vec<Neighbour>`; convert to VectorHit.
        HnswIndex::search(self, query, top_k).map(|hits| {
            hits.into_iter()
                .map(|h| VectorHit {
                    key: h.key.clone(),
                    distance: h.distance,
                })
                .collect()
        })
    }

    fn len(&self) -> usize {
        HnswIndex::len(self)
    }

    fn is_empty(&self) -> bool {
        HnswIndex::is_empty(self)
    }

    fn save(&self, dir: &Path, basename: &str) -> Result<(), MemoryError> {
        HnswIndex::save(self, dir, basename)
    }

    fn backend_name(&self) -> &'static str {
        "hnsw_rs 0.3 (with hnswio bincode 1.3.3 — see RUSTSEC-2025-0141)"
    }
}

fn validate_dimensions(vector: &[f32], expected: usize) -> Result<(), MemoryError> {
    if vector.len() != expected {
        return Err(MemoryError::HnswError(format!(
            "expected {} dimensions, got {}",
            expected,
            vector.len()
        )));
    }
    // LIB-LOW-002: Reject NaN/infinity embeddings
    if vector.iter().any(|v| !v.is_finite()) {
        return Err(MemoryError::HnswError(
            "embedding contains NaN or infinity values".into(),
        ));
    }
    Ok(())
}

fn hnsw_graph_file_name(basename: &str) -> String {
    format!("{basename}.hnsw.graph")
}

fn hnsw_data_file_name(basename: &str) -> String {
    format!("{basename}.hnsw.data")
}

fn hnsw_manifest_path(dir: &Path, basename: &str) -> PathBuf {
    dir.join(format!("{basename}.hnsw.manifest.json"))
}

fn file_digest(path: &Path) -> Result<String, MemoryError> {
    let bytes = fs::read(path).map_err(|e| {
        MemoryError::HnswError(format!(
            "failed to read HNSW sidecar {} for digest: {}",
            path.display(),
            e
        ))
    })?;
    Ok(format!("blake3:{}", ContentDigest::compute(&bytes).hex()))
}

fn write_hnsw_manifest(path: &Path, manifest: HnswSidecarManifestV1) -> Result<(), MemoryError> {
    let bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| {
        MemoryError::HnswError(format!("failed to serialize HNSW sidecar manifest: {e}"))
    })?;
    let mut file = File::create(path).map_err(|e| {
        MemoryError::HnswError(format!(
            "failed to create HNSW sidecar manifest {}: {}",
            path.display(),
            e
        ))
    })?;
    file.write_all(&bytes).map_err(|e| {
        MemoryError::HnswError(format!(
            "failed to write HNSW sidecar manifest {}: {}",
            path.display(),
            e
        ))
    })?;
    file.sync_all().map_err(|e| {
        MemoryError::HnswError(format!(
            "failed to fsync HNSW sidecar manifest {}: {}",
            path.display(),
            e
        ))
    })?;
    Ok(())
}

fn validate_hnsw_manifest(
    dir: &Path,
    basename: &str,
    config: &HnswConfig,
) -> Result<Option<HnswSidecarManifestV1>, MemoryError> {
    let manifest_path = hnsw_manifest_path(dir, basename);
    if !manifest_path.exists() {
        tracing::warn!(
            path = %manifest_path.display(),
            "HNSW sidecar manifest missing; legacy graph/data sidecar load is allowed"
        );
        return Ok(None);
    }
    let bytes = fs::read(&manifest_path).map_err(|e| {
        MemoryError::HnswError(format!(
            "failed to read HNSW sidecar manifest {}: {}",
            manifest_path.display(),
            e
        ))
    })?;
    let manifest: HnswSidecarManifestV1 = serde_json::from_slice(&bytes).map_err(|e| {
        MemoryError::HnswError(format!(
            "failed to parse HNSW sidecar manifest {}: {}",
            manifest_path.display(),
            e
        ))
    })?;
    if manifest.schema_version != HNSW_MANIFEST_SCHEMA_VERSION {
        return Err(MemoryError::HnswError(format!(
            "unsupported HNSW sidecar manifest schema {}; supported schema is {}",
            manifest.schema_version, HNSW_MANIFEST_SCHEMA_VERSION
        )));
    }
    if manifest.basename != basename {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar manifest basename mismatch: manifest={}, expected={basename}",
            manifest.basename
        )));
    }
    let expected_graph = hnsw_graph_file_name(basename);
    let expected_data = hnsw_data_file_name(basename);
    if manifest.graph_file_name != expected_graph || manifest.data_file_name != expected_data {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar manifest file mismatch: graph={}, data={}, expected graph={}, data={}",
            manifest.graph_file_name, manifest.data_file_name, expected_graph, expected_data
        )));
    }
    if manifest.dimensions != config.dimensions {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar manifest dimensions {} do not match configured {}",
            manifest.dimensions, config.dimensions
        )));
    }
    if manifest.hnsw_sidecar_format_version != HNSW_SIDECAR_VERSION {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar manifest format version {} does not match supported {}",
            manifest.hnsw_sidecar_format_version, HNSW_SIDECAR_VERSION
        )));
    }

    let graph_path = dir.join(&manifest.graph_file_name);
    let data_path = dir.join(&manifest.data_file_name);
    if !graph_path.exists() || !data_path.exists() {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar manifest points to missing files: graph_exists={}, data_exists={}",
            graph_path.exists(),
            data_path.exists()
        )));
    }
    let graph_digest = file_digest(&graph_path)?;
    let data_digest = file_digest(&data_path)?;
    if graph_digest != manifest.graph_digest {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar graph digest mismatch: manifest={}, actual={graph_digest}",
            manifest.graph_digest
        )));
    }
    if data_digest != manifest.data_digest {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar data digest mismatch: manifest={}, actual={data_digest}",
            manifest.data_digest
        )));
    }
    Ok(Some(manifest))
}

fn validate_manifest_against_header(
    manifest: &HnswSidecarManifestV1,
    graph_header: &SidecarHeader,
) -> Result<(), MemoryError> {
    if graph_header.dim as usize != manifest.dimensions
        || graph_header.vector_count != manifest.vector_count
    {
        return Err(MemoryError::HnswError(format!(
            "HNSW manifest/header mismatch: manifest dim/count={}/{}, graph dim/count={}/{}",
            manifest.dimensions, manifest.vector_count, graph_header.dim, graph_header.vector_count
        )));
    }
    Ok(())
}

fn validate_graph_sidecar(graph_path: &Path) -> Result<SidecarHeader, MemoryError> {
    let header = read_sidecar_header(graph_path, HNSW_GRAPH_MAGIC)?;
    validate_sidecar_header(&header)?;
    Ok(header)
}

fn load_vectors_from_sidecar(
    index: &HnswIndex,
    data_path: &Path,
    graph_header: &SidecarHeader,
) -> Result<usize, MemoryError> {
    let mut file = File::open(data_path).map_err(|e| {
        MemoryError::HnswError(format!("failed to open {}: {}", data_path.display(), e))
    })?;

    let header = read_sidecar_header_from_reader(&mut file, data_path, HNSW_DATA_MAGIC)?;
    validate_sidecar_header(&header)?;
    if graph_header.dim != header.dim || graph_header.vector_count != header.vector_count {
        return Err(MemoryError::HnswError(format!(
            "HNSW graph/data sidecar header mismatch: graph dim/count={}/{}, data dim/count={}/{}",
            graph_header.dim, graph_header.vector_count, header.dim, header.vector_count
        )));
    }
    let dims = usize::try_from(header.dim).map_err(|_| {
        MemoryError::HnswError(format!(
            "HNSW sidecar dimensions {} exceed this platform's usize range",
            header.dim
        ))
    })?;
    if dims != index.inner.config.dimensions {
        return Err(MemoryError::HnswError(format!(
            "HNSW sidecar dimensions {} do not match configured {}",
            dims, index.inner.config.dimensions
        )));
    }

    let mut max_id = 0usize;
    let mut loaded = 0u64;

    while loaded < header.vector_count {
        let mut u64_buf = [0u8; 8];
        file.read_exact(&mut u64_buf).map_err(|e| {
            MemoryError::HnswError(format!("failed to read HNSW sidecar node id: {}", e))
        })?;
        let id_u64 = u64::from_le_bytes(u64_buf);
        let id = usize::try_from(id_u64).map_err(|_| {
            MemoryError::HnswError(format!(
                "HNSW sidecar node id {id_u64} exceeds this platform's usize range"
            ))
        })?;
        if id >= index.inner.config.max_elements {
            return Err(MemoryError::HnswError(format!(
                "HNSW sidecar node id {id} exceeds configured max_elements {}",
                index.inner.config.max_elements
            )));
        }

        file.read_exact(&mut u64_buf).map_err(|e| {
            MemoryError::HnswError(format!("failed to read HNSW sidecar vector length: {}", e))
        })?;
        let byte_len_u64 = u64::from_le_bytes(u64_buf);
        let expected_byte_len = index
            .inner
            .config
            .dimensions
            .checked_mul(4)
            .ok_or_else(|| MemoryError::HnswError("HNSW dimension byte length overflow".into()))?;
        let byte_len = usize::try_from(byte_len_u64).map_err(|_| {
            MemoryError::HnswError(format!(
                "HNSW sidecar vector byte length {byte_len_u64} exceeds this platform's usize range"
            ))
        })?;
        if byte_len != expected_byte_len {
            return Err(MemoryError::HnswError(format!(
                "HNSW sidecar vector byte length {byte_len} does not match configured dimensions {} (expected {expected_byte_len} bytes)",
                index.inner.config.dimensions
            )));
        }
        let mut raw = vec![0u8; byte_len];
        file.read_exact(&mut raw).map_err(|e| {
            MemoryError::HnswError(format!("failed to read HNSW sidecar payload: {}", e))
        })?;

        let vector = db::decode_f32_le(&raw, index.inner.config.dimensions)?;
        index.insert_with_id(None, id, &vector)?;
        max_id = max_id.max(id);
        loaded += 1;
    }

    let mut trailing = [0u8; 1];
    if file.read(&mut trailing).map_err(|e| {
        MemoryError::HnswError(format!("failed to inspect HNSW sidecar trailer: {}", e))
    })? != 0
    {
        return Err(MemoryError::HnswError(
            "HNSW data sidecar has trailing bytes beyond declared vector_count".to_string(),
        ));
    }

    Ok(max_id)
}

fn atomically_replace_sidecar(tmp_path: &Path, final_path: &Path) -> Result<(), MemoryError> {
    if let Ok(file) = File::open(tmp_path) {
        file.sync_all().map_err(|e| {
            MemoryError::HnswError(format!(
                "failed to fsync temporary HNSW sidecar {}: {}",
                tmp_path.display(),
                e
            ))
        })?;
    }
    std::fs::rename(tmp_path, final_path).map_err(|e| {
        MemoryError::HnswError(format!(
            "failed to atomically replace HNSW sidecar {}: {}",
            final_path.display(),
            e
        ))
    })?;
    Ok(())
}

fn write_graph_sidecar(
    path: &Path,
    dimensions: usize,
    vector_count: u64,
) -> Result<(), MemoryError> {
    let mut file = File::create(path).map_err(|e| {
        MemoryError::HnswError(format!("failed to create {}: {}", path.display(), e))
    })?;
    write_sidecar_header(&mut file, HNSW_GRAPH_MAGIC, dimensions, vector_count)?;
    Ok(())
}

fn write_data_sidecar(
    path: &Path,
    dimensions: usize,
    vector_count: u64,
    vectors: &[(usize, Vec<f32>)],
) -> Result<(), MemoryError> {
    let mut file = File::create(path).map_err(|e| {
        MemoryError::HnswError(format!("failed to create {}: {}", path.display(), e))
    })?;
    write_sidecar_header(&mut file, HNSW_DATA_MAGIC, dimensions, vector_count)?;
    for (id, vector) in vectors {
        db::validate_embedding(vector, dimensions)?;
        let id = u64::try_from(*id).map_err(|_| {
            MemoryError::HnswError(format!("HNSW node id {id} exceeds u64 sidecar range"))
        })?;
        let bytes = db::encode_f32_le(vector);
        let byte_len = u64::try_from(bytes.len()).map_err(|_| {
            MemoryError::HnswError("HNSW vector byte length exceeds u64 sidecar range".to_string())
        })?;
        file.write_all(&id.to_le_bytes()).map_err(|e| {
            MemoryError::HnswError(format!("failed to write HNSW sidecar node id: {}", e))
        })?;
        file.write_all(&byte_len.to_le_bytes()).map_err(|e| {
            MemoryError::HnswError(format!("failed to write HNSW sidecar vector length: {}", e))
        })?;
        file.write_all(&bytes).map_err(|e| {
            MemoryError::HnswError(format!("failed to write HNSW sidecar vector: {}", e))
        })?;
    }
    Ok(())
}

fn write_sidecar_header<W: Write>(
    writer: &mut W,
    magic: u32,
    dimensions: usize,
    vector_count: u64,
) -> Result<(), MemoryError> {
    let dim = u32::try_from(dimensions).map_err(|_| {
        MemoryError::HnswError(format!(
            "HNSW dimensions {dimensions} exceed u32 sidecar header range"
        ))
    })?;
    writer
        .write_all(&magic.to_le_bytes())
        .and_then(|_| writer.write_all(&HNSW_SIDECAR_VERSION.to_le_bytes()))
        .and_then(|_| writer.write_all(&HNSW_SIDECAR_HEADER_LEN.to_le_bytes()))
        .and_then(|_| writer.write_all(&dim.to_le_bytes()))
        .and_then(|_| writer.write_all(&vector_count.to_le_bytes()))
        .and_then(|_| writer.write_all(&0u32.to_le_bytes()))
        .map_err(|e| MemoryError::HnswError(format!("failed to write HNSW sidecar header: {}", e)))
}

fn read_sidecar_header(path: &Path, expected_magic: u32) -> Result<SidecarHeader, MemoryError> {
    let mut file = File::open(path)
        .map_err(|e| MemoryError::HnswError(format!("failed to open {}: {}", path.display(), e)))?;
    read_sidecar_header_from_reader(&mut file, path, expected_magic)
}

fn read_sidecar_header_from_reader<R: Read>(
    reader: &mut R,
    path: &Path,
    expected_magic: u32,
) -> Result<SidecarHeader, MemoryError> {
    let mut header = [0u8; HNSW_SIDECAR_HEADER_LEN as usize];
    reader.read_exact(&mut header).map_err(|e| {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            MemoryError::HnswError(format!(
                "empty or truncated HNSW sidecar: {}",
                path.display()
            ))
        } else {
            MemoryError::HnswError(format!("failed to read HNSW sidecar header: {}", e))
        }
    })?;
    let parsed = SidecarHeader {
        magic: u32::from_le_bytes([header[0], header[1], header[2], header[3]]),
        version: u16::from_le_bytes([header[4], header[5]]),
        header_len: u16::from_le_bytes([header[6], header[7]]),
        dim: u32::from_le_bytes([header[8], header[9], header[10], header[11]]),
        vector_count: u64::from_le_bytes([
            header[12], header[13], header[14], header[15], header[16], header[17], header[18],
            header[19],
        ]),
        flags: u32::from_le_bytes([header[20], header[21], header[22], header[23]]),
    };
    if parsed.magic != expected_magic {
        return Err(MemoryError::HnswError(format!(
            "unsupported HNSW sidecar magic 0x{:08x}; expected 0x{:08x}",
            parsed.magic, expected_magic
        )));
    }
    Ok(parsed)
}

fn validate_sidecar_header(header: &SidecarHeader) -> Result<(), MemoryError> {
    if header.version != HNSW_SIDECAR_VERSION {
        return Err(MemoryError::HnswError(format!(
            "unsupported HNSW sidecar version {}; supported version is {}",
            header.version, HNSW_SIDECAR_VERSION
        )));
    }
    if header.header_len != HNSW_SIDECAR_HEADER_LEN {
        return Err(MemoryError::HnswError(format!(
            "unsupported HNSW sidecar header length {}; expected {}",
            header.header_len, HNSW_SIDECAR_HEADER_LEN
        )));
    }
    if header.flags != 0 {
        return Err(MemoryError::HnswError(format!(
            "unsupported HNSW sidecar flags 0x{:08x}",
            header.flags
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn test_config(max_elements: usize) -> HnswConfig {
        HnswConfig {
            dimensions: 2,
            max_elements,
            ..HnswConfig::default()
        }
    }

    #[test]
    fn hnsw_keymap_updates_are_single_lock_consistent_under_delete_search_race() {
        let index = Arc::new(HnswIndex::new(test_config(256)).unwrap());
        for i in 0..64 {
            index
                .insert(format!("fact:{i}"), &[1.0, i as f32 / 100.0])
                .unwrap();
        }

        std::thread::scope(|scope| {
            for worker in 0..4 {
                let index = Arc::clone(&index);
                scope.spawn(move || {
                    for i in 0..64 {
                        if i % 4 == worker {
                            index.delete(&format!("fact:{i}")).unwrap();
                        }
                    }
                });
            }
            for _ in 0..4 {
                let index = Arc::clone(&index);
                scope.spawn(move || {
                    for _ in 0..128 {
                        let hits = index.search(&[1.0, 0.0], 16).unwrap();
                        for hit in hits {
                            assert!(hit.key.starts_with("fact:"));
                        }
                    }
                });
            }
        });
    }

    #[test]
    fn dirty_flag_and_flush_epoch_use_seqcst_visible_state() {
        let index = HnswIndex::new(test_config(8)).unwrap();
        assert!(!index.is_keymap_dirty());
        index.insert("fact:a".into(), &[1.0, 0.0]).unwrap();
        assert!(index.is_keymap_dirty());
        assert!(index.should_flush(0));
        index.update_last_flush_epoch();
        assert!(!index.should_flush(u64::MAX));
    }

    #[test]
    fn id_exhaustion_is_explicit_until_compaction_rebuilds_sidecar() {
        let index = HnswIndex::new(test_config(1)).unwrap();
        index.insert("fact:a".into(), &[1.0, 0.0]).unwrap();
        index.delete("fact:a").unwrap();

        let error = index.insert("fact:b".into(), &[0.0, 1.0]).unwrap_err();
        assert!(error.to_string().contains("HNSW id space exhausted"));
        assert!(error.to_string().contains("compact or rebuild"));
    }
}
