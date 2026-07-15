//! Vector index backend trait.
//!
//! This trait is the abstraction layer over the concrete ANN backend (hnsw_rs
//! or usearch). It exposes a stable interface that the rest of the crate
//! (search.rs, hnsw_ops.rs, config.rs, lib.rs) uses, so that switching the
//! backend is a matter of which impl is wired in at the `hnsw_ops::rebuild_*`
//! and `HnswIndex` factory call sites.
//!
//! ## Design notes
//!
//! - The `VectorBackend` trait is intentionally minimal: just enough surface
//!   to satisfy the 8 call sites that currently use hnsw_rs.
//! - All backends return `Result<_, MemoryError>` so error handling at the
//!   trait boundary doesn't leak backend-specific error types.
//! - `HnswHit` is renamed to `VectorHit` in the trait surface, but a
//!   type alias `pub type HnswHit = VectorHit;` is preserved for source
//!   compatibility with downstream consumers.
//! - `HnswConfig` is kept as the user-facing config name (it's a public
//!   type). The trait receives a `VectorIndexConfig` internally; the
//!   `From<HnswConfig>` impl bridges them.
//!
//! ## Backend implementations
//!
//! - `HnswBackend` (in `hnsw_backend.rs`, gated on `feature = "hnsw"`):
//!   the existing hnsw_rs 0.3 wrapper, behavior-preserving.
//! - `UsearchBackend` (in `usearch_backend.rs`, gated on
//!   `feature = "usearch-backend"`): the new cxx-bridge to usearch 2.25.
//!   This file is the destination of the migration; until it's wired in
//!   it is a stub that returns MemoryError::Unimplemented.
//!
//! ## Backwards compatibility
//!
//! `HnswIndex` is renamed to `VectorIndex` (a thin newtype around
//! `Arc<dyn VectorBackend + Send + Sync>`). The old name is preserved as a
//! deprecated type alias to avoid breaking downstream consumers like
//! forge-pilot, llm-pipeline, and kernel-conformance that import
//! `semantic_memory::hnsw::HnswIndex` directly.

use std::path::Path;
use std::sync::Arc;

use crate::error::MemoryError;

/// User-facing hit from a vector search.
#[derive(Debug, Clone)]
pub struct VectorHit {
    pub key: String,
    pub distance: f32,
}

impl VectorHit {
    pub fn similarity(&self) -> f32 {
        (1.0 - self.distance).max(0.0)
    }

    /// Split the sidecar key into `(domain, identifier)`.
    pub fn parse_key(&self) -> Result<(&str, &str), MemoryError> {
        self.key
            .split_once(':')
            .ok_or_else(|| MemoryError::InvalidKey(self.key.clone()))
    }
}

/// Configuration for the vector index.
///
/// Field names and semantics match the existing `HnswConfig` so that
/// `From<HnswConfig> for VectorIndexConfig` is a no-op. Backend-specific
/// fields (e.g. `simsimd` flags for usearch) are abstracted away — the
/// usearch backend picks its own defaults from these top-level knobs.
#[derive(Debug, Clone)]
pub struct VectorIndexConfig {
    pub m: usize,
    pub ef_construction: usize,
    pub ef_search: usize,
    pub dimensions: usize,
    pub max_elements: usize,
    pub compaction_threshold: f32,
    pub flush_interval_secs: Option<u64>,
}

impl Default for VectorIndexConfig {
    fn default() -> Self {
        Self {
            m: 16,
            ef_construction: 200,
            ef_search: 50,
            dimensions: 768,
            max_elements: 100_000,
            compaction_threshold: 0.3,
            flush_interval_secs: None,
        }
    }
}

/// Core vector index operations. All concrete backends implement this.
///
/// Object-safe: all methods take `&self`, no generic parameters. The
/// factory functions (`new` and `load`) are provided as free `fn` items
/// rather than trait methods, so the trait itself is dyn-compatible.
pub trait VectorBackend: Send + Sync {
    /// Insert a key+vector pair. If the key already exists, the vector is
    /// updated.
    fn insert(&self, key: String, vector: &[f32]) -> Result<(), MemoryError>;

    /// Delete the key (if present). Idempotent.
    fn delete(&self, key: &str) -> Result<(), MemoryError>;

    /// Update the vector for an existing key (or insert if absent).
    fn update(&self, key: String, vector: &[f32]) -> Result<(), MemoryError>;

    /// k-NN search over the index. Returns up to `top_k` hits sorted by
    /// ascending distance.
    fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<VectorHit>, MemoryError>;

    /// Number of live (non-deleted) entries.
    fn len(&self) -> usize;

    /// Whether the index is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Flush the index to a backend-specific sidecar. Implementations may
    /// write additional files (manifest, digests, etc.) in the same dir.
    fn save(&self, dir: &Path, basename: &str) -> Result<(), MemoryError>;

    /// Human-readable backend name (e.g. "hnsw_rs 0.3", "usearch 2.25").
    /// Used in build receipts and `VectorArtifactBuildReceiptV1`.
    fn backend_name(&self) -> &'static str;
}

/// Thread-safe handle to a vector index.
#[derive(Clone)]
pub struct VectorIndex {
    inner: Arc<dyn VectorBackend>,
}

impl VectorIndex {
    /// Construct a new index from a config. Dispatches to the active
    /// backend (selected at compile time via feature flag).
    pub fn new(config: VectorIndexConfig) -> Result<Self, MemoryError> {
        let backend = build_active_backend(config)?;
        Ok(Self { inner: backend })
    }

    /// Load a previously saved index. Dispatches to the active backend.
    pub fn load(
        dir: &Path,
        basename: &str,
        config: VectorIndexConfig,
    ) -> Result<Self, MemoryError> {
        let backend = load_active_backend(dir, basename, config)?;
        Ok(Self { inner: backend })
    }

    pub fn insert(&self, key: String, vector: &[f32]) -> Result<(), MemoryError> {
        self.inner.insert(key, vector)
    }

    pub fn delete(&self, key: &str) -> Result<(), MemoryError> {
        self.inner.delete(key)
    }

    pub fn update(&self, key: String, vector: &[f32]) -> Result<(), MemoryError> {
        self.inner.update(key, vector)
    }

    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<VectorHit>, MemoryError> {
        self.inner.search(query, top_k)
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn save(&self, dir: &Path, basename: &str) -> Result<(), MemoryError> {
        self.inner.save(dir, basename)
    }

    pub fn backend_name(&self) -> &'static str {
        self.inner.backend_name()
    }

    /// Note: downcasting to a concrete backend type is not supported
    /// through the trait. Tests that need backend-specific introspection
    /// should use the `backend_name()` method or read the sidecar
    /// manifest. This is intentional — keeping the trait free of `Any`
    /// avoids the vtable overhead and keeps the public surface minimal.
    pub fn _placeholder(&self) {}
}

/// Factory: build a new backend using the active backend.
///
/// This is the single dispatch point that the rest of the crate uses to
/// select between hnsw_rs and usearch at compile time. The dispatch is
/// gated by `#[cfg(feature = ...)]` so the unused backend's code is not
/// compiled.
fn build_active_backend(config: VectorIndexConfig) -> Result<Arc<dyn VectorBackend>, MemoryError> {
    #[cfg(feature = "hnsw")]
    {
        return Ok(Arc::new(super::hnsw_backend::HnswBackend::new(config)?));
    }
    #[cfg(feature = "usearch-backend")]
    {
        return Ok(Arc::new(super::usearch_backend::UsearchBackend::new(
            config,
        )?));
    }
    // If neither feature is enabled, fall through to a stub that returns
    // an explicit error. The lib.rs compile_error! guard should prevent
    // this from being reached in practice.
    #[allow(unreachable_code)]
    {
        let _ = config;
        Err(MemoryError::NotImplemented(
            "no vector backend feature enabled (need `hnsw` or `usearch-backend`)".to_string(),
        ))
    }
}

fn load_active_backend(
    dir: &Path,
    basename: &str,
    config: VectorIndexConfig,
) -> Result<Arc<dyn VectorBackend>, MemoryError> {
    #[cfg(feature = "hnsw")]
    {
        return Ok(Arc::new(super::hnsw_backend::HnswBackend::load(
            dir, basename, config,
        )?));
    }
    #[cfg(feature = "usearch-backend")]
    {
        return Ok(Arc::new(super::usearch_backend::UsearchBackend::load(
            dir, basename, config,
        )?));
    }
    #[allow(unreachable_code)]
    {
        let _ = (dir, basename, config);
        Err(MemoryError::NotImplemented(
            "no vector backend feature enabled (need `hnsw` or `usearch-backend`)".to_string(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vector_hit_similarity_below_zero_clamps_to_zero() {
        let h = VectorHit {
            key: "fact:1".to_string(),
            distance: 2.0,
        };
        assert_eq!(h.similarity(), 0.0);
    }

    #[test]
    fn vector_hit_similarity_normal() {
        let h = VectorHit {
            key: "fact:1".to_string(),
            distance: 0.3,
        };
        assert!((h.similarity() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn vector_hit_parse_key_valid() {
        let h = VectorHit {
            key: "chunk:abc-123".to_string(),
            distance: 0.0,
        };
        let (domain, id) = h.parse_key().unwrap();
        assert_eq!(domain, "chunk");
        assert_eq!(id, "abc-123");
    }

    #[test]
    fn vector_hit_parse_key_invalid() {
        let h = VectorHit {
            key: "no_colon".to_string(),
            distance: 0.0,
        };
        assert!(h.parse_key().is_err());
    }

    #[test]
    fn config_default_matches_hnsw_default() {
        let c = VectorIndexConfig::default();
        assert_eq!(c.m, 16);
        assert_eq!(c.ef_construction, 200);
        assert_eq!(c.ef_search, 50);
        assert_eq!(c.dimensions, 768);
        assert_eq!(c.max_elements, 100_000);
    }
}
