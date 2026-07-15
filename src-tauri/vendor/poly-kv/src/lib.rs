//! Poly-KV: Shared compressed KV-cache pool for multi-agent context.
//!
//! ## Architecture
//!
//! A two-tier compression strategy derived from empirical benchmarks:
//! - **Shared pool (cold tier):** fib-quant at k=4, N=32, 50× compression, 100% recall
//! - **Agent shells (hot tier):** turbo-quant at 8-bit, 8× compression, 99.9% score retention
//!
//! ## Quick Start
//!
//! ```rust,ignore
//! use poly_kv::{SharedKVPool, KvTensorShape, AttentionType};
//!
//! let shape = KvTensorShape {
//!     attention_type: AttentionType::MHA,
//!     num_layers: 32,
//!     num_heads: 32,
//!     num_kv_heads: 32,
//!     head_dim: 128,
//!     hidden_size: 4096,
//! };
//!
//! let corpus: Vec<(String, Vec<f32>)> = vec![
//!     ("tok_0".into(), vec![0.1; shape.num_layers as usize * 32 * 128 * 2]),
//! ];
//!
//! let (pool, receipt) = SharedKVPool::build(&corpus, &shape, 42).unwrap();
//! let (shell, mat_receipt) = pool.materialize_shell("agent_1", &[], 42).unwrap();
//! ```

pub mod codec;
pub mod digest_compat;
pub mod error;
pub mod fallback;
pub mod ids_compat;
pub mod manifest;
pub mod policy;
pub mod pool;
pub mod receipt;
pub mod replay;
pub mod shape;
pub mod shell;

#[cfg(feature = "temporal-truth")]
pub mod temporal;

// Re-export core types
pub use codec::{create_codec, CompressedBlock, KVecCodec};
pub use digest_compat::Digest;
pub use error::{PolyKvError, Result};
pub use ids_compat::AgentId;
pub use manifest::{PoolManifest, ShellManifest, POOL_MANIFEST_SCHEMA, SHELL_MANIFEST_SCHEMA};
pub use policy::{
    CodecId, CompressionPolicy, FibConfig, TurboConfig, CODEC_EXACT_FALLBACK, CODEC_FIB_K4_N32,
    CODEC_TURBO_8BIT,
};
pub use pool::{
    CacheTarget, CompressedAttentionHit, CompressedAttentionSelection, DecompressedLayer,
    FullyPreparedCompressedIndex, PoolLayer, PrefetchedGramRows, PreparedCompressedIndex,
    SharedKVPool,
};
pub use receipt::{
    BlockInjectionTrace, CompressedAttentionSelectionReceipt, InjectionReceipt, PoolBuildReceipt,
    ShellMaterializeReceipt, COMPRESSED_ATTENTION_SELECTION_RECEIPT_SCHEMA,
    INJECTION_RECEIPT_SCHEMA, POOL_BUILD_RECEIPT_SCHEMA, RECEIPT_SCHEMA,
    SHELL_MATERIALIZE_RECEIPT_SCHEMA,
};
pub use replay::{
    run_captured_model_replay, run_model_replay, CandidateReplayMetrics, CapturedReplayConfig,
    CapturedReplayFixture, CapturedReplayQuery, CapturedReplayReceipt, ModelReplayConfig,
    ModelReplayMetrics, ModelReplayQuery, ModelReplayReceipt, CAPTURED_MODEL_REPLAY_RECEIPT_SCHEMA,
    MODEL_REPLAY_RECEIPT_SCHEMA,
};
pub use shape::{AttentionType, KvTensorShape};
pub use shell::{
    AgentShell, AttentionHit, CompressedShellAttentionHit, CompressedShellAttentionSelection,
    ShellLayer,
};
