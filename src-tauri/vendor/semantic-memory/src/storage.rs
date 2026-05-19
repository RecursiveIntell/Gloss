//! Storage path management for the memory directory convention.
//!
//! A memory store lives in a directory containing:
//! - `memory.db` — SQLite database (content, metadata, FTS5, f32 embeddings)
//! - `memory.hnsw.graph` — HNSW graph topology (when `hnsw` feature enabled)
//! - `memory.hnsw.data` — HNSW vector data (when `hnsw` feature enabled)

use std::path::{Path, PathBuf};

/// Resolved file paths for all storage files within a memory directory.
#[derive(Debug, Clone)]
pub struct StoragePaths {
    /// The base directory containing all storage files.
    pub base_dir: PathBuf,
    /// Path to the SQLite database file.
    pub sqlite_path: PathBuf,
    /// Directory for HNSW files (same as base_dir, hnsw_rs writes basename.hnsw.graph + basename.hnsw.data).
    pub hnsw_dir: PathBuf,
    /// Base name for HNSW files (e.g., "memory" → memory.hnsw.graph + memory.hnsw.data).
    pub hnsw_basename: String,
}

impl StoragePaths {
    /// Create storage paths from a base directory.
    ///
    /// Given `/path/to/memory`, resolves:
    /// - `/path/to/memory/memory.db`
    /// - `/path/to/memory/memory.hnsw.graph`
    /// - `/path/to/memory/memory.hnsw.data`
    pub fn new(base_dir: impl AsRef<Path>) -> Self {
        let base_dir = base_dir.as_ref().to_path_buf();
        Self {
            sqlite_path: base_dir.join("memory.db"),
            hnsw_dir: base_dir.clone(),
            hnsw_basename: "memory".to_string(),
            base_dir,
        }
    }

    /// Path to the HNSW graph file.
    pub fn hnsw_graph_path(&self) -> PathBuf {
        self.base_dir
            .join(format!("{}.hnsw.graph", self.hnsw_basename))
    }

    /// Path to the HNSW data file.
    pub fn hnsw_data_path(&self) -> PathBuf {
        self.base_dir
            .join(format!("{}.hnsw.data", self.hnsw_basename))
    }

    /// Path to the HNSW sidecar manifest file.
    pub fn hnsw_manifest_path(&self) -> PathBuf {
        self.base_dir
            .join(format!("{}.hnsw.manifest.json", self.hnsw_basename))
    }

    /// Whether both HNSW sidecar files exist on disk.
    pub fn hnsw_files_exist(&self) -> bool {
        self.hnsw_graph_path().exists() && self.hnsw_data_path().exists()
    }
}
