//! # Compressed Scorer — estimate inner products without decompressing vectors
//!
//! This crate provides a trait-based interface for scoring (estimating inner
//! products, cosine similarity, L2 distance) against compressed vector
//! representations WITHOUT decompressing them first.
//!
//! ## The key insight
//!
//! When you have N compressed vectors and want to find the top-K most similar
//! to a query, the standard approach is:
//! 1. Decompress all N vectors (expensive — N full reads)
//! 2. Compute dot product with query for each (N dot products)
//! 3. Select top-K
//!
//! With compressed-domain scoring:
//! 1. Prepare the query once (rotate + quantize to match the codec)
//! 2. For each compressed vector: O(1) table lookup or cheap estimate
//! 3. Select top-K by approximate score
//! 4. Decompress ONLY the top-K for exact verification (optional)
//!
//! This reduces the work from O(N * dim) to O(N * 1) for the scoring phase,
//! and O(K * dim) for the decompression phase where K << N.
//!
//! ## Supported codecs
//!
//! - **fib-quant**: Gram-table lookup. `G[i,j] = <codeword_i, codeword_j>`.
//!   Precomputed at construction time. O(1) per scored vector.
//! - **turbo-quant**: Polar-coordinate inner product estimate after seeded
//!   rotation. Data-oblivious — no trained codebook needed.
//!
//! ## Usage
//!
//! ```rust,ignore
//! use compressed_scorer::{CompressedScorer, ScoringConfig};
//!
//! // Build a scorer with fib-quant
//! let scorer = CompressedScorer::fib_quant(dim, bits, seed)?;
//!
//! // Prepare the query once
//! let prepared = scorer.prepare_query(&query)?;
//!
//! // Score against compressed vectors (no decompression!)
//! for (idx, code) in compressed_vectors.iter().enumerate() {
//!     let score = scorer.score_prepared(&prepared, code)?;
//!     if score > threshold {
//!         candidates.push((idx, score));
//!     }
//! }
//!
//! // Sort candidates, decompress only top-K
//! candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
//! for (idx, _) in candidates.iter().take(K) {
//!     let decoded = scorer.decode(&compressed_vectors[*idx])?;
//!     // exact verification...
//! }
//! ```

// Note: this crate uses alloc (Vec, format!) but not full std.
// When the `no_std` feature is enabled, callers must provide alloc.
#![cfg_attr(feature = "no_std", no_std)]

#[cfg(feature = "no_std")]
extern crate alloc;

pub mod adaptive_budget;
pub mod attention_cache;
pub mod candidate;
pub mod error;
pub mod per_dim_impl;
pub mod trait_def;
pub mod working_set;

#[cfg(feature = "fib")]
pub mod fib_impl;

#[cfg(feature = "turbo")]
pub mod turbo_impl;

pub use adaptive_budget::{
    allocate_head_budgets, allocate_layer_budgets, default_fragility_256tok,
    default_fragility_512tok, default_fragility_for_seq_len, learn_budgets, BudgetConfig,
    HeadBudgets, HeadFragilityEntry, LayerBudgets, LayerFragilityEntry,
};
pub use attention_cache::{AttentionCache, AttentionOutput};
pub use candidate::{search_topk, CandidateList};
pub use error::{ScorerError, ScorerResult};
pub use per_dim_impl::{PerDimCompressed, PerDimPrepared, PerDimScorer};
pub use trait_def::{
    CompressedScorer, PreparedQuery, ProgressiveCompressedScorer, ProgressiveScoredCandidate,
    ScoreStage, ScoreWithUncertainty, ScoredCandidate,
};
pub use working_set::{
    CacheRuntimePolicy, CompressedPage, CompressedWorkingSet, FallbackPolicy, GuardPolicy,
    PageRole, WorkingSetSelection, WorkingSetSelectionReceipt,
};

#[cfg(test)]
mod integration_tests;
#[cfg(test)]
mod tests;
