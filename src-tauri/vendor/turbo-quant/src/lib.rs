//! # turbo-quant
//!
//! Experimental Rust implementation of **TurboQuant**, **PolarQuant**, and
//! **QJL** profile-defined vector quantization algorithm families.
//!
//! The crate creates derived compressed sidecars for high-dimensional vectors.
//! Quality is workload-dependent and must be measured with exact fallback gates.
//!
//! ## Key Properties
//!
//! - **Data-oblivious**: no k-means training, no codebook, no calibration set.
//!   The rotation is seeded once; retrieval quality still depends on the
//!   deployment distribution and filters.
//! - **Deterministic**: identical `(dim, bits, seed)` always produces the same
//!   quantizer. State can be fully reconstructed from four integers.
//! - **Measured quality**: inner product estimates are approximate and
//!   retrieval deployments still need recall/rank gates.
//! - **Instant indexing**: unlike Product Quantization, there is no offline
//!   training phase. Vectors can be indexed as they arrive.
//!
//! ## Quick Start
//!
//! ```rust
//! use turbo_quant::{TurboQuantizer, PolarQuantizer};
//!
//! // Compress 1536-dimensional embeddings (OpenAI/sentence-transformer size).
//! let dim = 64; // use 1536 in production
//! let q = TurboQuantizer::new(dim, 8, 32, /* seed */ 42).unwrap();
//!
//! let database_vector: Vec<f32> = vec![0.1; dim]; // your embedding here
//! let query_vector: Vec<f32> = vec![0.1; dim];    // your query here
//!
//! // Create a compressed sidecar for the database vector.
//! let code = q.encode(&database_vector).unwrap();
//!
//! // At query time: estimate inner product without decompressing.
//! let score = q.inner_product_estimate(&code, &query_vector).unwrap();
//!
//! // Or just use PolarQuant for a simpler single-stage compressor.
//! let pq = PolarQuantizer::new(dim, 8, 42).unwrap();
//! let polar_code = pq.encode(&database_vector).unwrap();
//! let polar_score = pq.inner_product_estimate(&polar_code, &query_vector).unwrap();
//! ```
//!
//! ## Choosing Parameters
//!
//! | Use case | Recommended bits | Recommended projections |
//! |---|---|---|
//! | Semantic search (recall@10) | 8 | dim / 4 |
//! | KV cache compression | 4–6 | dim / 8 |
//! | Maximum compression | 3 | dim / 16 |
//!
//! ## References
//!
//! - TurboQuant-style two-stage polar plus residual sketch compression.
//! - Polar-coordinate quantization after seeded rotation.
//! - Quantized Johnson-Lindenstrauss sign-projection sketches.

pub mod bitpack;
pub mod codebook;
pub mod error;
pub mod eval;
pub mod kv;
pub mod polar;
pub mod profile;
pub mod qjl;
pub mod rotation;
pub mod turbo;
pub mod wire;

pub use codebook::ScalarCodebook;
pub use error::{Result, TurboQuantError};
pub use eval::{BenchmarkComparisonV1, BenchmarkCorpus, BenchmarkReceiptV1, CompressionEvalV1};
pub use kv::{CompressedToken, KvCacheCompressor, KvCacheConfig, KvQuantPolicy, KvShadowScore};
pub use polar::{PolarCode, PolarProjectedQuery, PolarQuantizer};
pub use profile::{CodecProfileV1, CompressionPolicyV1, CompressionReceiptV1, ValidationState};
pub use qjl::{QjlProjectedQuery, QjlQuantizer, QjlSketch};
pub use rotation::{FastHadamardRotation, Rotation, RotationBackend, RotationKind, StoredRotation};
pub use turbo::{BatchStats, TurboCode, TurboMode, TurboProjectedQuery, TurboQuantizer};
pub use wire::{TurboCodeWireV1, TURBO_CODE_WIRE_MAGIC};
