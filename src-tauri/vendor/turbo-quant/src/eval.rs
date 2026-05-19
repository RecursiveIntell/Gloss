//! Evaluation and benchmark receipt data structures.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::profile::CodecProfileV1;

/// Summary of an exact-vs-compressed evaluation run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompressionEvalV1 {
    pub schema: String,
    pub recall_at_k: f32,
    pub mean_absolute_error: f32,
    pub queries: usize,
    pub db_size: usize,
    pub top_k: usize,
}

/// Machine-readable benchmark receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BenchmarkReceiptV1 {
    pub schema: String,
    pub profile: CodecProfileV1,
    pub corpus: BenchmarkCorpus,
    pub metrics: CompressionEvalV1,
    pub comparisons: Vec<BenchmarkComparisonV1>,
    pub commands: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct BenchmarkComparisonV1 {
    pub name: String,
    pub profile: CodecProfileV1,
    pub metrics: CompressionEvalV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BenchmarkCorpus {
    pub dim: usize,
    pub db_size: usize,
    pub queries: usize,
    pub seed: u64,
    pub generator: String,
}

pub fn recall_at_k(exact: &[Vec<usize>], estimated: &[Vec<usize>], k: usize) -> f32 {
    if exact.is_empty() || exact.len() != estimated.len() || k == 0 {
        return 0.0;
    }
    let mut hits = 0usize;
    let mut total = 0usize;
    for (exact_row, estimated_row) in exact.iter().zip(estimated.iter()) {
        let exact_top = &exact_row[..exact_row.len().min(k)];
        let estimated_top = &estimated_row[..estimated_row.len().min(k)];
        total += exact_top.len();
        hits += estimated_top
            .iter()
            .filter(|candidate| exact_top.contains(candidate))
            .count();
    }
    if total == 0 {
        0.0
    } else {
        hits as f32 / total as f32
    }
}
