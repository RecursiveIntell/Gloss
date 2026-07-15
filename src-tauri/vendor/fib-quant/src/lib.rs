#![warn(rustdoc::broken_intra_doc_links)]

//! Experimental paper-core FibQuant math crate.
//!
//! This crate implements the normalize, deterministic rotation,
//! spherical-Beta block source, radial-angular codebook, Lloyd-Max refinement,
//! and fixed-rate codec path described in `FibQuant: Universal Vector
//! Quantization for Random-Access KV-Cache Compression`.
//!
//! The `0.1.0-alpha.1` surface is deliberately narrow. It is not a production
//! KV-cache compressor, not a benchmark reproduction package, and not
//! integrated with any parent workspace memory crate. Profiles are validated
//! against explicit alpha resource limits before allocation-heavy paths run.
//!
//! ```
//! use fib_quant::{FibQuantProfileV1, FibQuantizer};
//!
//! # fn main() -> fib_quant::Result<()> {
//! let mut profile = FibQuantProfileV1::paper_default(8, 2, 8, 7)?;
//! profile.training_samples = 128;
//! profile.lloyd_restarts = 1;
//! profile.lloyd_iterations = 2;
//! let quantizer = FibQuantizer::new(profile)?;
//! let input = vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875];
//! let code = quantizer.encode(&input)?;
//! let decoded = quantizer.decode(&code)?;
//! assert_eq!(decoded.len(), input.len());
//! # Ok(())
//! # }
//! ```

pub mod batch_ingest;
pub mod beta_inv;
pub mod bitpack;
pub mod codebook;
pub mod codec;
#[cfg(feature = "compat")]
pub mod compat;
pub mod digest;
pub mod directions;
pub mod error;
pub mod eval;
pub mod ffi;
#[cfg(feature = "kv")]
pub mod kv;
pub mod lattice;
pub mod lloyd;
pub mod metrics;
pub mod persistence;
pub mod profile;
pub mod receipt;
pub mod residual;
pub mod rope;
pub mod rotation;
pub mod scoring;
pub mod sidecar;
pub mod spherical_beta;
pub mod wire;

// Archived Rust implementations of hot paths replaced by C kernels.
// Not compiled — kept for historical reference.
#[allow(unused)]
mod archive;

pub use batch_ingest::{BatchIngestPipeline, IngestReceipt};
pub use codebook::{build_initial_codebook, FibCodebookV1};
pub use codec::{
    CompactFeatureFlags, FibCodeV1, FibQuantizer, GpuStepReport, CODEC_ID, COMPACT_MAGIC,
    COMPACT_V2_MAGIC, COMPACT_V2_VERSION, COMPACT_VERSION,
};
pub use directions::{fibonacci_sphere_3d, fibonacci_spiral_2d, roberts_kronecker};
pub use error::{FibQuantError, Result};
pub use eval::{ndcg_at_k, recall_at_k, run_benchmark, FibBenchmarkCorpus, FibBenchmarkReceiptV1};
pub use lattice::{quantize_a2_pairs, quantize_z1, LatticeKind, LatticeQuantizationResult};
pub use lloyd::{LloydRepairEventV1, LloydReportV1};
pub use persistence::{load_from_file, save_to_file, FibSidecarFileV1, FILE_MAGIC, FILE_VERSION};
#[cfg(feature = "mmap")]
pub use persistence::{load_mmap, MmapSidecarIndex};
pub use profile::{
    DirectionMethod, EmptyCellPolicy, FibQuantProfileV1, LloydMode, NormFormat, RadiusMethod,
    SourceMode, MAX_AMBIENT_DIM, MAX_BLOCK_DIM, MAX_CODEBOOK_SIZE, MAX_CODEBOOK_VALUES,
    MAX_PACKED_INDEX_BITS, MAX_ROTATION_MATRIX_VALUES, MAX_TRAINING_SAMPLES,
};
pub use receipt::FibQuantCompressionReceiptV1;
pub use residual::{
    FibMultiLevelQuantizer, FibResidualCodeV1, FibResidualQuantizer, MultiLevelCode,
    MultiLevelResidualCodebookV1, ResidualCodebookV1,
};
pub use rope::{
    allocate_rope_bits, rope_block_energies, rope_blocks, RopeBitAllocation, RopeBlock,
    RopeBlockEnergy,
};
pub use rotation::{StoredRotation, ROTATION_ALGORITHM_VERSION, ROTATION_SCHEMA};
pub use scoring::{FibPreparedQuery, FibScorer, GramTable, ScoredItem};
pub use sidecar::{
    FibSidecarIndex, IvfCoarseQuantizer, ScoredCandidate, SearchReceiptIvfV1, SearchReceiptV1,
};
pub use spherical_beta::{
    beta_d_k, radius_quantile, radius_quantile_k2_closed_form, sample_reference_projection,
    sample_spherical_beta,
};
pub use wire::{FibCodeWireV1, WireHeader, WIRE_HEADER_SIZE, WIRE_MAGIC, WIRE_VERSION};
