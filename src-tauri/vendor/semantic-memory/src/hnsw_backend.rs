//! HnswBackend — re-exports the `HnswIndex` struct from `hnsw.rs` as the
//! `hnsw_rs 0.3` implementation of the [`VectorBackend`] trait.
//!
//! The actual struct, its `new`/`load`/etc. methods, and its `VectorBackend`
//! impl all live in `hnsw.rs` to keep the existing public surface stable
//! (downstream crates like forge-pilot, llm-pipeline, and kernel-conformance
//! import `semantic_memory::hnsw::HnswIndex` directly). This module exists
//! purely as the dispatch target for the `build_active_backend` /
//! `load_active_backend` factories in `vector_backend.rs`.
//!
//! ## Why not just rename `hnsw.rs` → `hnsw_backend.rs`?
//!
//! Renaming would break the public `pub mod hnsw;` declaration in lib.rs,
//! which is a stable API for the workspace. Keeping `hnsw.rs` as the
//! historical module path (with the `HnswIndex` struct and its `impl
//! VectorBackend for HnswIndex` block) preserves that. The
//! `hnsw_backend` name is a logical-only construct used by the backend
//! factory.

pub use crate::hnsw::HnswIndex as HnswBackend;
