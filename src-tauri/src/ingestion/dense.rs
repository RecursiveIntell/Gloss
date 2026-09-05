use crate::error::GlossError;
use sha2::{Digest, Sha256};
use std::path::Path;
use usearch::ffi::{IndexOptions, MetricKind, ScalarKind};

/// The single native dense artifact. Every native ingestion/rebuild path must
/// use this identifier rather than inventing a second index file.
pub const NATIVE_DENSE_ARTIFACT_FILENAME: &str = "chunks.usearch";

pub fn native_dense_artifact_path(notebook_dir: &Path) -> std::path::PathBuf {
    notebook_dir
        .join("embeddings")
        .join(NATIVE_DENSE_ARTIFACT_FILENAME)
}

/// HNSW vector index using usearch (C++ via FFI, but only for add/search/save —
/// no model inference here, so heap corruption from ONNX batch embed is isolated).
pub struct HnswIndex {
    index: usearch::Index,
    dims: usize,
    next_label: u64,
    published_digest: std::cell::Cell<Option<[u8; 32]>>,
    dirty: std::cell::Cell<bool>,
}

const INITIAL_HNSW_CAPACITY: usize = 1_024;

impl HnswIndex {
    pub fn new(dims: usize) -> Result<Self, GlossError> {
        let options = IndexOptions {
            metric: MetricKind::IP,
            connectivity: 16,
            dimensions: dims,
            quantization: ScalarKind::F32,
            ..Default::default()
        };
        let index = usearch::Index::new(&options)
            .map_err(|e| GlossError::Embedding(format!("Failed to create HNSW index: {e}")))?;
        index
            .reserve(INITIAL_HNSW_CAPACITY)
            .map_err(|e| GlossError::Embedding(format!("Failed to reserve HNSW index: {e}")))?;
        Ok(Self {
            index,
            dims,
            next_label: 0,
            published_digest: std::cell::Cell::new(None),
            dirty: std::cell::Cell::new(false),
        })
    }

    pub fn load_with_hwm(path: &Path, hwm: i64, dims: usize) -> Result<Self, GlossError> {
        let mut index = Self::new(dims)?;
        if path.exists() {
            // Bind the digest to the exact loaded bytes even if another writer
            // atomically replaces the path while a reader is loading it.
            let bytes = std::fs::read(path)?;
            index
                .index
                .load_from_buffer(&bytes)
                .map_err(|e| GlossError::Embedding(format!("Failed to load HNSW index: {e}")))?;
            index
                .published_digest
                .set(Some(Sha256::digest(&bytes).into()));
            if index.index.dimensions() != dims {
                return Err(GlossError::Embedding(format!(
                    "Stored HNSW dimension mismatch: artifact has {} dims, expected {dims}",
                    index.index.dimensions()
                )));
            }
            index.next_label = u64::try_from(hwm).unwrap_or(0).saturating_add(1);
            let hwm_capacity = usize::try_from(hwm).unwrap_or(0);
            let capacity = hwm_capacity.max(index.index.size());
            index
                .index
                .reserve(capacity)
                .map_err(|e| GlossError::Embedding(format!("Failed to reserve HNSW index: {e}")))?;
        }
        Ok(index)
    }

    pub fn add(&mut self, vector: &[f32]) -> Result<u64, GlossError> {
        if vector.len() != self.dims {
            return Err(GlossError::Embedding(format!(
                "HNSW add dimension mismatch: vector has {} dims, index expects {}",
                vector.len(),
                self.dims
            )));
        }
        let mut label = self.next_label;
        while self.index.contains(label) {
            label = label
                .checked_add(1)
                .ok_or_else(|| GlossError::Embedding("HNSW label overflow".into()))?;
        }
        if label > i64::MAX as u64 {
            return Err(GlossError::Embedding(
                "HNSW label exceeds SQLite integer range".into(),
            ));
        }
        if self.index.size() >= self.index.capacity() {
            let capacity = self
                .index
                .capacity()
                .max(INITIAL_HNSW_CAPACITY)
                .checked_mul(2)
                .ok_or_else(|| GlossError::Embedding("HNSW capacity growth overflow".into()))?;
            self.index.reserve(capacity).map_err(|e| {
                GlossError::Embedding(format!("Failed to grow HNSW index capacity: {e}"))
            })?;
        }
        self.index
            .add(label, vector)
            .map_err(|e| GlossError::Embedding(format!("HNSW add failed: {e}")))?;
        self.next_label = label + 1;
        self.dirty.set(true);
        Ok(label)
    }

    pub fn remove(&mut self, label: u64) -> Result<bool, GlossError> {
        let removed = self
            .index
            .remove(label)
            .map_err(|e| GlossError::Embedding(format!("HNSW remove failed: {e}")))?;
        if removed > 0 {
            self.dirty.set(true);
        }
        Ok(removed > 0)
    }

    pub fn search(&self, query: &[f32], count: usize) -> Result<Vec<(u64, f32)>, GlossError> {
        if query.len() != self.dims {
            return Err(GlossError::Embedding(format!(
                "HNSW search dimension mismatch: query has {} dims, index expects {}",
                query.len(),
                self.dims
            )));
        }
        let results = self
            .index
            .search(query, count)
            .map_err(|e| GlossError::Embedding(format!("HNSW search failed: {e}")))?;
        Ok(results.keys.into_iter().zip(results.distances).collect())
    }

    pub fn save(&self, path: &Path) -> Result<(), GlossError> {
        std::fs::create_dir_all(path.parent().unwrap_or(Path::new("")))?;
        self.index
            .save(path.to_str().unwrap_or(""))
            .map_err(|e| GlossError::Embedding(format!("HNSW save failed: {e}")))
    }

    /// Publish a dense artifact only after a separate reload verifies the
    /// written bytes. Metadata must be advanced after this method succeeds.
    pub fn save_atomic_verified(&self, path: &Path, hwm: i64) -> Result<(), GlossError> {
        // Callers hold the notebook's SQLite writer transaction. A cached
        // index must never replace a newer queue/inline publication.
        let current_digest = if path.exists() {
            Some(Self::artifact_digest(path)?)
        } else {
            None
        };
        if current_digest != self.published_digest.get() {
            return Err(GlossError::Embedding(
                "Native dense artifact changed since load; reload before publishing".into(),
            ));
        }
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent)?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(NATIVE_DENSE_ARTIFACT_FILENAME);
        let temporary = parent.join(format!(".{filename}.{}.tmp", uuid::Uuid::new_v4()));

        let result = (|| {
            self.save(&temporary)?;
            std::fs::File::open(&temporary)?.sync_all()?;
            // Loading the temporary artifact catches corrupt/partial writes
            // before the canonical path is replaced.
            let verified = Self::load_with_hwm(&temporary, hwm, self.dims)?;
            if verified.dims != self.dims {
                return Err(GlossError::Embedding(
                    "HNSW reload verification changed embedding dimensions".to_string(),
                ));
            }
            drop(verified);
            std::fs::rename(&temporary, path)?;
            #[cfg(unix)]
            std::fs::File::open(parent)?.sync_all()?;
            let published = Self::load_with_hwm(path, hwm, self.dims)?;
            self.published_digest.set(published.published_digest.get());
            self.dirty.set(false);
            drop(published);
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }

    fn artifact_digest(path: &Path) -> Result<[u8; 32], GlossError> {
        let mut file = std::fs::File::open(path)?;
        let mut digest = Sha256::new();
        std::io::copy(&mut file, &mut digest)?;
        Ok(digest.finalize().into())
    }

    pub fn contains(&self, label: u64) -> bool {
        self.index.contains(label)
    }

    pub fn has_pending_changes(&self) -> bool {
        self.dirty.get()
    }

    pub fn is_current(&self, path: &Path) -> Result<bool, GlossError> {
        let digest = if path.exists() {
            Some(Self::artifact_digest(path)?)
        } else {
            None
        };
        Ok(digest == self.published_digest.get())
    }

    #[allow(dead_code)]
    pub fn size(&self) -> usize {
        self.index.size()
    }
}

/// One recoverable native batch: serialize writers, reload the latest artifact,
/// verify every committed label, publish bytes, then commit mappings and status.
/// An interrupted DB commit leaves only orphan vectors; chunk IDs stay pending.
pub fn publish_dense_batch(
    db: &crate::db::notebook_db::NotebookDb,
    path: &Path,
    chunks: &[crate::db::notebook_db::Chunk],
    embeddings: &[Vec<f32>],
    metadata: &crate::db::notebook_db::EmbeddingIndexMetadata,
) -> Result<usize, GlossError> {
    publish_dense_batch_with(
        db,
        path,
        chunks,
        embeddings,
        metadata,
        |index, path, hwm| index.save_atomic_verified(path, hwm),
    )
}

/// Cache acquisition is read-only: publication readiness belongs to the writer.
pub fn load_published_dense_index(
    db: &crate::db::notebook_db::NotebookDb,
    path: &Path,
    expected: &crate::db::notebook_db::EmbeddingIndexMetadata,
) -> Result<HnswIndex, GlossError> {
    let stored = db.embedding_index_metadata(crate::db::notebook_db::NATIVE_HNSW_INDEX_ID)?;
    if !stored
        .as_ref()
        .is_some_and(|stored| stored.identity_matches(expected))
    {
        return Err(GlossError::Embedding(
            "Native dense index is not ready for the configured embedding identity".into(),
        ));
    }
    let dims = expected
        .dimensions
        .ok_or_else(|| GlossError::Embedding("Missing embedding dimensions".into()))?;
    let index = HnswIndex::load_with_hwm(path, db.max_embedding_id()?.unwrap_or(0), dims)?;
    validate_committed_labels(db, &index)?;
    Ok(index)
}

fn validate_committed_labels(
    db: &crate::db::notebook_db::NotebookDb,
    index: &HnswIndex,
) -> Result<(), GlossError> {
    for label in db.native_embedding_ids()? {
        if label < 0 || !index.contains(label as u64) {
            return Err(GlossError::Embedding(format!("Native dense artifact is missing committed label {label}; explicit rebuild required")));
        }
    }
    Ok(())
}

/// Best-effort cleanup may remove only vectors already detached from canonical
/// chunks. It must not promote or otherwise rewrite publication/config status.
pub fn publish_dense_cleanup(
    db: &crate::db::notebook_db::NotebookDb,
    path: &Path,
    index: &HnswIndex,
) -> Result<(), GlossError> {
    db.with_dense_index_transaction(|db| {
        validate_committed_labels(db, index)?;
        index.save_atomic_verified(path, db.max_embedding_id()?.unwrap_or(0))
    })
}

fn publish_dense_batch_with(
    db: &crate::db::notebook_db::NotebookDb,
    path: &Path,
    chunks: &[crate::db::notebook_db::Chunk],
    embeddings: &[Vec<f32>],
    metadata: &crate::db::notebook_db::EmbeddingIndexMetadata,
    publish: impl FnOnce(&HnswIndex, &Path, i64) -> Result<(), GlossError>,
) -> Result<usize, GlossError> {
    use crate::db::notebook_db::{EmbeddingIndexMetadataStatus, NATIVE_HNSW_INDEX_ID};
    if chunks.len() != embeddings.len() {
        return Err(GlossError::Embedding(
            "Embedding batch count does not match chunk count".into(),
        ));
    }
    let dims = metadata
        .dimensions
        .ok_or_else(|| GlossError::Embedding("Missing embedding dimensions".into()))?;
    // Keep the derivation identity durable even if the first publication
    // succeeds but the subsequent mapping transaction aborts or crashes.
    db.with_dense_index_transaction(|db| {
        let stored = db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID)?;
        if stored
            .as_ref()
            .is_some_and(|stored| stored.status == "stale")
        {
            return Err(GlossError::Embedding(
                "Native dense configuration was invalidated; explicit rebuild required".into(),
            ));
        }
        if path.exists() || db.max_embedding_id()?.is_some() {
            if !stored
                .as_ref()
                .is_some_and(|stored| stored.derivation_matches(metadata))
            {
                return Err(GlossError::Embedding(
                    "Native dense identity mismatch; explicit rebuild required".into(),
                ));
            }
        }
        let mut building = metadata.clone();
        building.status = EmbeddingIndexMetadataStatus::Building.as_str().into();
        building.status_reason =
            Some("Verified native batch publication and mapping commit pending".into());
        db.upsert_embedding_index_metadata(&building)
    })?;
    let result = db.with_dense_index_transaction(|db| {
        let stored = db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID)?;
        if !stored
            .as_ref()
            .is_some_and(|stored| stored.status != "stale" && stored.derivation_matches(metadata))
        {
            return Err(GlossError::Embedding(
                "Native dense identity changed before publication; retry required".into(),
            ));
        }
        let hwm = db.max_embedding_id()?.unwrap_or(0);
        let mut index = HnswIndex::load_with_hwm(path, hwm, dims)?;
        validate_committed_labels(db, &index)?;
        let mut mappings = Vec::new();
        for (chunk, vector) in chunks.iter().zip(embeddings) {
            let current = db.get_chunk(&chunk.id)?;
            if current.source_id != chunk.source_id || current.content != chunk.content {
                return Err(GlossError::Embedding(
                    "Chunk changed while embedding; retry required".into(),
                ));
            }
            if current.embedding_id.is_none() {
                mappings.push((chunk.id.as_str(), index.add(vector)? as i64));
            }
        }
        publish(&index, path, hwm)?;
        for (chunk_id, label) in &mappings {
            db.update_chunk_embedding(chunk_id, *label, &metadata.model)?;
        }
        db.upsert_embedding_index_metadata(metadata)?;
        Ok(mappings.len())
    });
    if let Err(error) = &result {
        db.with_dense_index_transaction(|db| {
            let current = db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID)?;
            // Do not erase a settings invalidation or overwrite another
            // writer's successful publication while reporting this failure.
            if current.as_ref().is_some_and(|current| {
                current.status == "building" && current.derivation_matches(metadata)
            }) {
                db.mark_embedding_index_status(
                    NATIVE_HNSW_INDEX_ID,
                    EmbeddingIndexMetadataStatus::Blocked,
                    Some(&error.to_string()),
                )?;
            }
            Ok(())
        })?;
    }
    result
}

#[cfg(test)]
mod audit_dense_tests {
    use super::*;
    use crate::db::notebook_db::{Chunk, EmbeddingIndexMetadata, NotebookDb, NATIVE_HNSW_INDEX_ID};

    fn batch_fixture(db: &NotebookDb, source: &str, count: usize) -> Vec<Chunk> {
        db.conn().execute("INSERT INTO sources (id, source_type, title, status) VALUES (?1, 'text', ?1, 'ready')", [source]).unwrap();
        let chunks = (0..count)
            .map(|i| Chunk {
                id: format!("{source}-{i}"),
                source_id: source.into(),
                chunk_index: i as i32,
                content: format!("chunk {i}"),
                token_count: None,
                start_offset: None,
                end_offset: None,
                metadata: None,
                embedding_id: None,
                embedding_model: None,
            })
            .collect::<Vec<_>>();
        db.insert_chunks(&chunks).unwrap();
        chunks
    }

    fn metadata() -> EmbeddingIndexMetadata {
        EmbeddingIndexMetadata::ready(
            NATIVE_HNSW_INDEX_ID,
            "fixture",
            "model",
            Some("digest".into()),
            3,
        )
    }

    #[test]
    fn cold_cache_load_preserves_ready_and_failed_publication_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let path = native_dense_artifact_path(dir.path());
        let chunks = batch_fixture(&db, "a", 1);
        publish_dense_batch(&db, &path, &chunks, &[vec![1.0, 0.0, 0.0]], &metadata()).unwrap();
        let before = db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap();
        drop(load_published_dense_index(&db, &path, &metadata()).unwrap());
        assert_eq!(
            db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap(),
            before
        );
        for status in [
            crate::db::notebook_db::EmbeddingIndexMetadataStatus::Building,
            crate::db::notebook_db::EmbeddingIndexMetadataStatus::Blocked,
        ] {
            db.mark_embedding_index_status(
                NATIVE_HNSW_INDEX_ID,
                status,
                Some("retryable publication failure"),
            )
            .unwrap();
            let before = db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap();
            assert!(load_published_dense_index(&db, &path, &metadata()).is_err());
            assert_eq!(
                db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap(),
                before
            );
        }
        // Failed reads did not turn retryable state into configuration Stale.
        publish_dense_batch(&db, &path, &chunks, &[vec![1.0, 0.0, 0.0]], &metadata()).unwrap();
    }

    #[test]
    fn configuration_invalidation_cannot_be_erased_by_an_old_embedder() {
        let dir = tempfile::tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let path = native_dense_artifact_path(dir.path());
        let chunks = batch_fixture(&db, "a", 2);
        let vectors = vec![vec![1.0, 0.0, 0.0]; 2];
        publish_dense_batch(&db, &path, &chunks[..1], &vectors[..1], &metadata()).unwrap();
        db.mark_embedding_index_status(
            NATIVE_HNSW_INDEX_ID,
            crate::db::notebook_db::EmbeddingIndexMetadataStatus::Stale,
            Some("embedding-index-stale: setting semantic_memory_embedding_model changed"),
        )
        .unwrap();
        let before = db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert!(publish_dense_batch(&db, &path, &chunks[1..], &vectors[1..], &metadata()).is_err());
        assert_eq!(
            db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap(),
            before
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(db.get_chunks_without_embedding("a").unwrap().len(), 1);
    }

    #[test]
    fn canonical_delete_failure_preserves_vectors_and_cleanup_checks_survivors() {
        let dir = tempfile::tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let path = native_dense_artifact_path(dir.path());
        let chunks = batch_fixture(&db, "a", 1);
        publish_dense_batch(&db, &path, &chunks, &[vec![1.0, 0.0, 0.0]], &metadata()).unwrap();
        let label = db.get_chunk(&chunks[0].id).unwrap().embedding_id.unwrap() as u64;
        let before = std::fs::read(&path).unwrap();
        let metadata_before = db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap();
        db.conn().execute_batch("CREATE TRIGGER reject_delete BEFORE DELETE ON sources BEGIN SELECT RAISE(ABORT, 'delete failure'); END;").unwrap();
        assert!(db
            .delete_source_with_projection_invalidation("nb", "a")
            .is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        assert_mappings_present(&db, &path);
        let mut cache = load_published_dense_index(&db, &path, &metadata()).unwrap();
        cache.remove(label).unwrap();
        assert!(publish_dense_cleanup(&db, &path, &cache).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), before);
        db.conn()
            .execute_batch("DROP TRIGGER reject_delete;")
            .unwrap();
        db.delete_source_with_projection_invalidation("nb", "a")
            .unwrap();
        publish_dense_cleanup(&db, &path, &cache).unwrap();
        assert_eq!(
            db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID).unwrap(),
            metadata_before
        );
        assert_eq!(HnswIndex::load_with_hwm(&path, 0, 3).unwrap().size(), 0);
    }

    #[test]
    fn retry_reset_rolls_back_status_and_mappings_on_chunk_delete_failure() {
        let dir = tempfile::tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let path = native_dense_artifact_path(dir.path());
        let chunks = batch_fixture(&db, "a", 1);
        publish_dense_batch(&db, &path, &chunks, &[vec![1.0, 0.0, 0.0]], &metadata()).unwrap();
        db.conn().execute_batch("CREATE TRIGGER reject_chunk_delete BEFORE DELETE ON chunks BEGIN SELECT RAISE(ABORT, 'reset failure'); END;").unwrap();
        assert!(db.reset_source_for_reingestion("nb", "a").is_err());
        assert_eq!(db.get_source("a").unwrap().status, "ready");
        assert_mappings_present(&db, &path);
        db.conn()
            .execute_batch("DROP TRIGGER reject_chunk_delete;")
            .unwrap();
        assert_eq!(db.reset_source_for_reingestion("nb", "a").unwrap(), "text");
        assert_eq!(db.get_source("a").unwrap().status, "pending");
        assert_eq!(
            db.get_source("a")
                .unwrap()
                .processing_state
                .unwrap()
                .dense_index_status,
            "missing"
        );
    }

    fn assert_mappings_present(db: &NotebookDb, path: &Path) {
        let loaded =
            HnswIndex::load_with_hwm(path, db.max_embedding_id().unwrap().unwrap_or(0), 3).unwrap();
        for label in db.native_embedding_ids().unwrap() {
            assert!(loaded.contains(label as u64));
        }
    }

    #[test]
    fn publication_failure_leaves_chunks_pending_for_restart_retry() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("notebook.db");
        let path = native_dense_artifact_path(dir.path());
        let db = NotebookDb::open(&db_path).unwrap();
        let chunks = batch_fixture(&db, "a", 2);
        let vectors = vec![vec![1.0, 0.0, 0.0]; 2];
        let failed =
            publish_dense_batch_with(&db, &path, &chunks, &vectors, &metadata(), |index, _, _| {
                assert_eq!(index.size(), 2);
                Err(GlossError::Other("injected save/rename failure".into()))
            });
        assert!(failed.is_err());
        assert!(!path.exists());
        assert_eq!(db.get_chunks_without_embedding("a").unwrap().len(), 2);
        assert_eq!(
            db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID)
                .unwrap()
                .unwrap()
                .status,
            "blocked"
        );
        drop(db);
        let db = NotebookDb::connect(&db_path).unwrap();
        assert_eq!(
            publish_dense_batch(&db, &path, &chunks, &vectors, &metadata()).unwrap(),
            2
        );
        assert_mappings_present(&db, &path);
    }

    #[test]
    fn mapping_failure_after_publication_rolls_back_the_entire_batch() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("notebook.db");
        let path = native_dense_artifact_path(dir.path());
        let db = NotebookDb::open(&db_path).unwrap();
        let chunks = batch_fixture(&db, "a", 2);
        let vectors = vec![vec![1.0, 0.0, 0.0]; 2];
        db.conn().execute_batch("CREATE TRIGGER fail_last_mapping AFTER UPDATE OF embedding_id ON chunks WHEN NEW.id = 'a-1' BEGIN SELECT RAISE(ABORT, 'injected last mapping failure'); END;").unwrap();
        assert!(publish_dense_batch(&db, &path, &chunks, &vectors, &metadata()).is_err());
        assert!(path.exists());
        assert_eq!(db.get_chunks_without_embedding("a").unwrap().len(), 2);
        assert_ne!(
            db.get_source("a")
                .unwrap()
                .processing_state
                .unwrap()
                .dense_index_status,
            "indexed"
        );
        db.conn()
            .execute_batch("DROP TRIGGER fail_last_mapping;")
            .unwrap();
        drop(db);
        let db = NotebookDb::connect(&db_path).unwrap();
        let other = batch_fixture(&db, "b", 1);
        publish_dense_batch(&db, &path, &other, &vectors[..1], &metadata()).unwrap();
        assert_eq!(db.get_chunks_without_embedding("a").unwrap().len(), 2);
        publish_dense_batch(&db, &path, &chunks, &vectors, &metadata()).unwrap();
        assert_mappings_present(&db, &path);
        let ids = db.native_embedding_ids().unwrap();
        assert_eq!(ids.len(), 3);
        assert_eq!(
            ids.iter().collect::<std::collections::HashSet<_>>().len(),
            3
        );
    }

    #[test]
    fn multi_batch_retry_preserves_the_committed_prefix_and_source_readiness() {
        let dir = tempfile::tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let path = native_dense_artifact_path(dir.path());
        let chunks = batch_fixture(&db, "a", 3);
        let vectors = vec![vec![1.0, 0.0, 0.0]; 3];
        publish_dense_batch(&db, &path, &chunks[..2], &vectors[..2], &metadata()).unwrap();
        assert_eq!(
            db.get_source("a")
                .unwrap()
                .processing_state
                .unwrap()
                .dense_index_status,
            "building"
        );
        let prefix = db.native_embedding_ids().unwrap();
        assert!(publish_dense_batch_with(
            &db,
            &path,
            &chunks[2..],
            &vectors[2..],
            &metadata(),
            |_, _, _| Err(GlossError::Other("disk failure".into()))
        )
        .is_err());
        assert_eq!(db.native_embedding_ids().unwrap(), prefix);
        assert_mappings_present(&db, &path);
        publish_dense_batch(&db, &path, &chunks[2..], &vectors[2..], &metadata()).unwrap();
        assert_eq!(
            db.get_source("a")
                .unwrap()
                .processing_state
                .unwrap()
                .dense_index_status,
            "indexed"
        );
        assert_eq!(
            publish_dense_batch(&db, &path, &chunks, &vectors, &metadata()).unwrap(),
            0
        );
        assert_mappings_present(&db, &path);
    }

    #[test]
    fn missing_committed_label_cannot_be_promoted_by_a_later_source() {
        let dir = tempfile::tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let path = native_dense_artifact_path(dir.path());
        let a = batch_fixture(&db, "a", 1);
        let b = batch_fixture(&db, "b", 1);
        db.update_chunk_embedding(&a[0].id, 7, "model").unwrap();
        db.upsert_embedding_index_metadata(&metadata()).unwrap();
        assert!(publish_dense_batch(&db, &path, &b, &[vec![1.0, 0.0, 0.0]], &metadata()).is_err());
        assert_eq!(db.get_chunks_without_embedding("b").unwrap().len(), 1);
        assert_eq!(
            db.embedding_index_metadata(NATIVE_HNSW_INDEX_ID)
                .unwrap()
                .unwrap()
                .status,
            "blocked"
        );
    }

    #[test]
    fn stale_cached_writer_cannot_overwrite_a_newer_publication() {
        let dir = tempfile::tempdir().unwrap();
        let path = native_dense_artifact_path(dir.path());
        let mut first = HnswIndex::new(3).unwrap();
        first.add(&[1.0, 0.0, 0.0]).unwrap();
        first.save_atomic_verified(&path, 0).unwrap();
        let mut stale = HnswIndex::load_with_hwm(&path, 0, 3).unwrap();
        first.add(&[0.0, 1.0, 0.0]).unwrap();
        first.save_atomic_verified(&path, 1).unwrap();
        stale.remove(0).unwrap();
        assert!(!stale.is_current(&path).unwrap());
        assert!(stale.save_atomic_verified(&path, 1).is_err());
        let current = HnswIndex::load_with_hwm(&path, 1, 3).unwrap();
        assert!(current.contains(0));
        assert!(current.contains(1));
    }

    #[test]
    fn repeated_verified_publication_releases_both_reload_owners() {
        let dir = tempfile::tempdir().unwrap();
        let path = native_dense_artifact_path(dir.path());
        let mut index = HnswIndex::new(768).unwrap();
        for _ in 0..32 {
            index.add(&[0.125; 768]).unwrap();
            index
                .save_atomic_verified(&path, index.size() as i64)
                .unwrap();
        }
        assert_eq!(HnswIndex::load_with_hwm(&path, 32, 768).unwrap().size(), 32);
    }

    #[test]
    fn native_loaded_indexes_repeatedly_drop_without_leaking_ownership() {
        let dir = tempfile::tempdir().unwrap();
        let path = native_dense_artifact_path(dir.path());
        let mut original = HnswIndex::new(32).unwrap();
        for _ in 0..128 {
            original.add(&[0.25; 32]).unwrap();
        }
        original.save(&path).unwrap();
        drop(original);
        for _ in 0..256 {
            let reloaded = HnswIndex::load_with_hwm(&path, 127, 32).unwrap();
            assert_eq!(reloaded.size(), 128);
            assert!(!reloaded.search(&[0.25; 32], 1).unwrap().is_empty());
            drop(reloaded);
        }
    }

    #[test]
    fn deleting_a_vector_never_reuses_a_live_label() {
        let mut index = HnswIndex::new(3).unwrap();
        assert_eq!(index.add(&[1.0, 0.0, 0.0]).unwrap(), 0);
        assert_eq!(index.add(&[0.0, 1.0, 0.0]).unwrap(), 1);
        assert_eq!(index.add(&[0.0, 0.0, 1.0]).unwrap(), 2);
        assert!(index.remove(0).unwrap());
        assert_eq!(index.add(&[0.5, 0.5, 0.0]).unwrap(), 3);
        assert_eq!(index.size(), 3);
    }

    #[test]
    fn reloading_after_delete_respects_durable_high_water_mark() {
        let dir = tempfile::tempdir().unwrap();
        let path = native_dense_artifact_path(dir.path());
        let mut index = HnswIndex::new(3).unwrap();
        for _ in 0..4 {
            index.add(&[1.0, 0.0, 0.0]).unwrap();
        }
        index.remove(1).unwrap();
        index.save(&path).unwrap();
        let mut reloaded = HnswIndex::load_with_hwm(&path, 3, 3).unwrap();
        assert_eq!(reloaded.add(&[0.0, 1.0, 0.0]).unwrap(), 4);
    }

    #[test]
    fn rejects_artifact_with_different_dimensions() {
        let dir = tempfile::tempdir().unwrap();
        let path = native_dense_artifact_path(dir.path());
        let mut index = HnswIndex::new(3).unwrap();
        index.add(&[1.0, 0.0, 0.0]).unwrap();
        index.save(&path).unwrap();
        assert!(HnswIndex::load_with_hwm(&path, 0, 4).is_err());
    }
}
