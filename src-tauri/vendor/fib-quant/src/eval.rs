//! Benchmark and evaluation harness for FibQuant.
//!
//! Provides a typed benchmark receipt (`FibBenchmarkReceiptV1`) capturing
//! recall@K, nDCG@K, compression ratio, cosine similarity, MSE, and timing
//! for a corpus of vectors queried against a FibQuant-quantized database.

use std::collections::HashSet;
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::{metrics, FibCodeV1, FibQuantizer, Result};

/// Schema version stamp for benchmark receipts.
pub const BENCHMARK_SCHEMA: &str = "fib_benchmark_v1";

/// A corpus of database vectors, queries, and ground-truth top-K labels.
///
/// The `ground_truth_topk` must have the same length as `queries`, and each
/// entry must contain the indices of the `k` most similar database vectors
/// (by exact cosine similarity) for the corresponding query.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FibBenchmarkCorpus {
    /// Database vectors to be encoded.
    pub vectors: Vec<Vec<f32>>,
    /// Query vectors.
    pub queries: Vec<Vec<f32>>,
    /// For each query, the indices of the true top-K most similar database vectors.
    pub ground_truth_topk: Vec<Vec<usize>>,
    /// K value for recall@K.
    pub k: usize,
    /// Human-readable corpus identifier.
    pub label: String,
}

impl FibBenchmarkCorpus {
    /// Compute exact cosine similarity between a query and all database vectors,
    /// returning the top-K indices sorted by descending similarity.
    pub fn exact_topk(&self, query: &[f32], k: usize) -> Result<Vec<usize>> {
        let mut sims: Vec<(usize, f64)> = Vec::with_capacity(self.vectors.len());
        for (idx, v) in self.vectors.iter().enumerate() {
            let sim = metrics::cosine_similarity(query, v)?;
            sims.push((idx, sim));
        }
        // Sort by descending similarity, tie-break by index for determinism.
        sims.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        Ok(sims.into_iter().take(k).map(|(idx, _)| idx).collect())
    }
}

/// Typed benchmark receipt produced by [`run_benchmark`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FibBenchmarkReceiptV1 {
    /// Schema version stamp.
    pub schema_version: String,
    /// Corpus label.
    pub corpus_label: String,
    /// Profile digest (BLAKE3 hex).
    pub profile_digest: String,
    /// Codebook digest (BLAKE3 hex).
    pub codebook_digest: String,
    /// Rotation digest (BLAKE3 hex).
    pub rotation_digest: String,
    /// Number of database vectors encoded.
    pub vector_count: usize,
    /// Number of queries evaluated.
    pub query_count: usize,
    /// K value used for recall@K and nDCG@K.
    pub k: usize,
    /// Compression ratio (total original bytes / total encoded bytes).
    pub compression_ratio: f64,
    /// Mean cosine similarity between original and reconstructed vectors.
    pub mean_cosine_similarity: f64,
    /// Mean reconstruction MSE across all database vectors.
    pub mean_mse: f64,
    /// Recall@K averaged over all queries.
    pub recall_at_k: f64,
    /// nDCG@K averaged over all queries.
    pub ndcg_at_k: f64,
    /// Total encode time in microseconds.
    pub encode_elapsed_micros: u128,
    /// Total decode time in microseconds.
    pub decode_elapsed_micros: u128,
    /// Free-form notes about the benchmark run.
    pub notes: Vec<String>,
}

/// Recall@K: fraction of true top-K items found in the approximate top-K.
///
/// Returns 0.0 if `k` is 0 or either slice is empty.
/// Returns the ratio of intersection size to K.
pub fn recall_at_k(exact_topk: &[usize], approx_topk: &[usize], k: usize) -> f64 {
    if k == 0 || exact_topk.is_empty() || approx_topk.is_empty() {
        return 0.0;
    }
    let exact_set: HashSet<usize> = exact_topk.iter().take(k).copied().collect();
    let hits = approx_topk
        .iter()
        .take(k)
        .filter(|idx| exact_set.contains(idx))
        .count();
    let denom = exact_topk.len().min(k);
    if denom == 0 {
        0.0
    } else {
        hits as f64 / denom as f64
    }
}

/// nDCG@K: normalized Discounted Cumulative Gain at K.
///
/// Given the exact relevance scores (sorted by descending relevance) and the
/// approximate ranking (indices into the database), compute nDCG@K using the
/// standard formula:
///
/// ```text
/// DCG@K = sum_{i=1}^{K} rel_i / log2(i + 1)
/// nDCG@K = DCG_approx / DCG_ideal
/// ```
///
/// `exact_scores` should be the true relevance scores of the approximate
/// ranking's items (i.e., the cosine similarities of the approx-ranked items
/// to the query). `approx_ranking` is the list of database indices in the
/// approximate order. `k` caps the evaluation depth.
///
/// Returns 0.0 if `k` is 0 or inputs are empty.
pub fn ndcg_at_k(exact_scores: &[f64], approx_ranking: &[usize], k: usize) -> f64 {
    if k == 0 || exact_scores.is_empty() || approx_ranking.is_empty() {
        return 0.0;
    }
    let cap = k.min(approx_ranking.len()).min(exact_scores.len());
    if cap == 0 {
        return 0.0;
    }

    // Standard ANN evaluation nDCG@K:
    //   exact_scores[i] is the relevance (e.g. cosine similarity) of the
    //   i-th item in the approximate ranking. approx_ranking[i] is the
    //   database index of that item. DCG is computed over the approximate
    //   ordering; ideal DCG is the same scores sorted descending.
    //
    // DCG@K  = sum_{i=0}^{K-1} rel_i / log2(i + 2)
    // nDCG@K = DCG_approx / DCG_ideal

    let dcg_approx: f64 = (0..cap)
        .map(|i| {
            let gain = exact_scores[i].max(0.0);
            gain / ((i as f64 + 2.0).log2())
        })
        .sum();

    let mut ideal_scores: Vec<f64> = exact_scores.to_vec();
    ideal_scores.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));

    let dcg_ideal: f64 = (0..cap)
        .map(|i| {
            let gain = ideal_scores[i].max(0.0);
            gain / ((i as f64 + 2.0).log2())
        })
        .sum();

    if dcg_ideal == 0.0 {
        0.0
    } else {
        dcg_approx / dcg_ideal
    }
}

/// Run a full benchmark on a corpus with a FibQuantizer.
///
/// Encodes all corpus vectors, decodes them, measures reconstruction quality,
/// then evaluates recall@K and nDCG@K for each query against the ground truth.
/// Timing covers the encode and decode phases separately.
pub fn run_benchmark(
    corpus: &FibBenchmarkCorpus,
    quantizer: &FibQuantizer,
) -> Result<FibBenchmarkReceiptV1> {
    if corpus.vectors.is_empty() {
        return Err(crate::FibQuantError::CorruptPayload(
            "benchmark corpus has no vectors".into(),
        ));
    }
    if corpus.queries.is_empty() {
        return Err(crate::FibQuantError::CorruptPayload(
            "benchmark corpus has no queries".into(),
        ));
    }
    if corpus.k == 0 {
        return Err(crate::FibQuantError::CorruptPayload(
            "benchmark corpus k must be > 0".into(),
        ));
    }
    if corpus.ground_truth_topk.len() != corpus.queries.len() {
        return Err(crate::FibQuantError::CorruptPayload(format!(
            "ground_truth_topk len {} != queries len {}",
            corpus.ground_truth_topk.len(),
            corpus.queries.len()
        )));
    }

    let dim = quantizer.profile().ambient_dim as usize;
    let original_bytes_per_vec = dim * std::mem::size_of::<f32>();

    // ── Encode phase ──
    let encode_start = Instant::now();
    let codes: Vec<FibCodeV1> = corpus
        .vectors
        .iter()
        .map(|v| quantizer.encode(v))
        .collect::<Result<Vec<_>>>()?;
    let encode_elapsed = encode_start.elapsed();

    // ── Decode phase ──
    let decode_start = Instant::now();
    let decoded_vectors = quantizer.decode_batch_fast(&codes)?;
    let decode_elapsed = decode_start.elapsed();

    // ── Compression ratio ──
    let total_original_bytes = corpus.vectors.len() * original_bytes_per_vec;
    let total_encoded_bytes: usize = codes.iter().map(|c| c.compact_size()).sum();
    let compression_ratio = if total_encoded_bytes == 0 {
        0.0
    } else {
        total_original_bytes as f64 / total_encoded_bytes as f64
    };

    // ── Reconstruction quality ──
    let mut cos_sum = 0.0f64;
    let mut mse_sum = 0.0f64;
    for (orig, recon) in corpus.vectors.iter().zip(decoded_vectors.iter()) {
        cos_sum += metrics::cosine_similarity(orig, recon)?;
        mse_sum += metrics::mse(orig, recon)?;
    }
    let n = corpus.vectors.len() as f64;
    let mean_cosine = cos_sum / n;
    let mean_mse = mse_sum / n;

    // ── Recall@K and nDCG@K ──
    let mut recall_sum = 0.0f64;
    let mut ndcg_sum = 0.0f64;
    for (qi, query) in corpus.queries.iter().enumerate() {
        let gt = &corpus.ground_truth_topk[qi];

        // Approximate top-K: compute cosine similarity between query and
        // all decoded (reconstructed) vectors, then take the top-K.
        let mut approx_sims: Vec<(usize, f64)> = Vec::with_capacity(decoded_vectors.len());
        for (db_idx, recon) in decoded_vectors.iter().enumerate() {
            let sim = metrics::cosine_similarity(query, recon)?;
            approx_sims.push((db_idx, sim));
        }
        approx_sims.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then(a.0.cmp(&b.0))
        });
        let approx_topk: Vec<usize> = approx_sims
            .iter()
            .take(corpus.k)
            .map(|(idx, _)| *idx)
            .collect();

        // Recall@K
        recall_sum += recall_at_k(gt, &approx_topk, corpus.k);

        // nDCG@K: build the relevance scores for the approximate ranking.
        // For each item in approx_topk, its relevance is the exact cosine
        // similarity between the query and the *original* database vector.
        let approx_scores: Vec<f64> = approx_topk
            .iter()
            .map(|&db_idx| {
                metrics::cosine_similarity(query, &corpus.vectors[db_idx]).unwrap_or(0.0)
            })
            .collect();
        ndcg_sum += ndcg_at_k(&approx_scores, &approx_topk, corpus.k);
    }

    let n_queries = corpus.queries.len() as f64;
    let mean_recall = recall_sum / n_queries;
    let mean_ndcg = ndcg_sum / n_queries;

    // ── Digests ──
    let profile_digest = quantizer.profile().digest()?;
    let codebook_digest = quantizer.codebook_digest().to_string();
    let rotation_digest = quantizer.rotation_digest().to_string();

    Ok(FibBenchmarkReceiptV1 {
        schema_version: BENCHMARK_SCHEMA.into(),
        corpus_label: corpus.label.clone(),
        profile_digest,
        codebook_digest,
        rotation_digest,
        vector_count: corpus.vectors.len(),
        query_count: corpus.queries.len(),
        k: corpus.k,
        compression_ratio,
        mean_cosine_similarity: mean_cosine,
        mean_mse,
        recall_at_k: mean_recall,
        ndcg_at_k: mean_ndcg,
        encode_elapsed_micros: encode_elapsed.as_micros(),
        decode_elapsed_micros: decode_elapsed.as_micros(),
        notes: Vec::new(),
    })
}

// ─────────────────────────── Tests ───────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn make_tiny_corpus() -> FibBenchmarkCorpus {
        // 6 vectors of dim 8
        let vectors: Vec<Vec<f32>> = vec![
            vec![0.25, -0.5, 0.75, 1.0, -1.25, 0.5, 0.125, -0.875],
            vec![-0.3, 0.6, -0.9, -1.1, 1.3, -0.4, 0.2, 0.85],
            vec![0.5, 0.5, 0.5, 0.5, -0.5, -0.5, -0.5, -0.5],
            vec![0.1, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8],
            vec![-0.2, -0.1, 0.0, 0.1, 0.2, 0.3, 0.4, 0.5],
            vec![0.9, -0.3, 0.7, -0.5, 0.2, 0.1, -0.4, 0.6],
        ];
        let queries: Vec<Vec<f32>> = vec![
            vec![0.3, -0.4, 0.6, 0.9, -0.8, 0.3, 0.2, -0.5],
            vec![0.4, 0.4, 0.4, 0.4, -0.4, -0.4, -0.4, -0.4],
        ];
        // Compute exact top-3 for each query
        let k = 3;
        let gt: Vec<Vec<usize>> = queries
            .iter()
            .map(|q| {
                let mut sims: Vec<(usize, f64)> = vectors
                    .iter()
                    .enumerate()
                    .map(|(i, v)| (i, metrics::cosine_similarity(q, v).unwrap()))
                    .collect();
                sims.sort_by(|a, b| {
                    b.1.partial_cmp(&a.1)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then(a.0.cmp(&b.0))
                });
                sims.iter().take(k).map(|(i, _)| *i).collect()
            })
            .collect();
        FibBenchmarkCorpus {
            vectors,
            queries,
            ground_truth_topk: gt,
            k,
            label: "tiny_test".into(),
        }
    }

    fn make_quantizer() -> FibQuantizer {
        let mut profile = crate::FibQuantProfileV1::paper_default(8, 2, 16, 7).unwrap();
        profile.training_samples = 128;
        profile.lloyd_restarts = 1;
        profile.lloyd_iterations = 2;
        FibQuantizer::new(profile).unwrap()
    }

    #[test]
    fn test_recall_at_k_perfect_match() {
        let exact = vec![0usize, 1, 2];
        let approx = vec![0usize, 1, 2];
        assert!((recall_at_k(&exact, &approx, 3) - 1.0).abs() < 1e-12);
        // Order shouldn't matter
        let approx_reordered = vec![2, 0, 1];
        assert!((recall_at_k(&exact, &approx_reordered, 3) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_recall_at_k_no_match() {
        let exact = vec![0usize, 1, 2];
        let approx = vec![3usize, 4, 5];
        assert!((recall_at_k(&exact, &approx, 3) - 0.0).abs() < 1e-12);
    }

    #[test]
    fn test_recall_at_k_partial() {
        let exact = vec![0usize, 1, 2];
        let approx = vec![0usize, 3, 2];
        // 2 out of 3 hits
        assert!((recall_at_k(&exact, &approx, 3) - (2.0 / 3.0)).abs() < 1e-12);
    }

    #[test]
    fn test_recall_at_k_edge_cases() {
        // k=0 → 0
        assert_eq!(recall_at_k(&[0, 1], &[0, 1], 0), 0.0);
        // empty → 0
        assert_eq!(recall_at_k(&[], &[0], 3), 0.0);
        assert_eq!(recall_at_k(&[0], &[], 3), 0.0);
        // k > len → cap at min
        let exact = vec![0usize, 1];
        let approx = vec![0usize, 1];
        assert!((recall_at_k(&exact, &approx, 10) - 1.0).abs() < 1e-12);
    }

    #[test]
    fn test_ndcg_at_k_perfect_order() {
        // If approx ranking matches ideal ordering, nDCG = 1.0
        let exact_scores = vec![0.9, 0.8, 0.7];
        let approx_ranking = vec![0usize, 1, 2];
        let score = ndcg_at_k(&exact_scores, &approx_ranking, 3);
        assert!(
            (score - 1.0).abs() < 1e-9,
            "perfect nDCG should be 1.0, got {}",
            score
        );
    }

    #[test]
    fn test_ndcg_at_k_reversed_order() {
        // Worst case: reversed order. nDCG should be < 1.0 but > 0.
        let exact_scores = vec![0.7, 0.8, 0.9]; // relevance in approx order
        let approx_ranking = vec![2usize, 1, 0]; // but the ranking is reversed
                                                 // Actually, exact_scores[i] is the relevance of item approx_ranking[i].
                                                 // So in the approx order, item 2 has relevance 0.7 (worst),
                                                 // item 1 has 0.8, item 0 has 0.9 (best). The approx ranking puts
                                                 // worst first. DCG_approx uses exact_scores in order: 0.7, 0.8, 0.9.
                                                 // DCG_ideal sorts desc: 0.9, 0.8, 0.7.
        let score = ndcg_at_k(&exact_scores, &approx_ranking, 3);
        assert!(
            score > 0.0 && score < 1.0,
            "reversed nDCG should be in (0,1), got {}",
            score
        );
    }

    #[test]
    fn test_ndcg_at_k_edge_cases() {
        assert_eq!(ndcg_at_k(&[0.5], &[0], 0), 0.0);
        assert_eq!(ndcg_at_k(&[], &[0], 3), 0.0);
        assert_eq!(ndcg_at_k(&[0.5], &[], 3), 0.0);
    }

    #[test]
    fn test_run_benchmark_produces_valid_receipt() {
        let corpus = make_tiny_corpus();
        let quantizer = make_quantizer();
        let receipt = run_benchmark(&corpus, &quantizer).expect("benchmark should succeed");

        // Check basic fields
        assert_eq!(receipt.schema_version, BENCHMARK_SCHEMA);
        assert_eq!(receipt.corpus_label, "tiny_test");
        assert_eq!(receipt.vector_count, 6);
        assert_eq!(receipt.query_count, 2);
        assert_eq!(receipt.k, 3);

        // Receipt fields should be finite
        assert!(
            receipt.compression_ratio.is_finite(),
            "compression_ratio not finite"
        );
        assert!(
            receipt.compression_ratio > 0.0,
            "compression_ratio should be positive, got {}",
            receipt.compression_ratio
        );
        assert!(
            receipt.mean_cosine_similarity.is_finite(),
            "mean_cosine_similarity not finite"
        );
        assert!(receipt.mean_mse.is_finite(), "mean_mse not finite");
        assert!(receipt.mean_mse >= 0.0, "mean_mse should be non-negative");
        assert!(receipt.recall_at_k.is_finite(), "recall_at_k not finite");
        assert!(
            (0.0..=1.0).contains(&receipt.recall_at_k),
            "recall_at_k should be in [0,1], got {}",
            receipt.recall_at_k
        );
        assert!(receipt.ndcg_at_k.is_finite(), "ndcg_at_k not finite");
        assert!(
            (0.0..=1.0).contains(&receipt.ndcg_at_k),
            "ndcg_at_k should be in [0,1], got {}",
            receipt.ndcg_at_k
        );

        // Timing should be non-negative
        assert!(receipt.encode_elapsed_micros > 0 || receipt.decode_elapsed_micros > 0);

        // Digests should be non-empty hex strings
        assert!(!receipt.profile_digest.is_empty());
        assert!(!receipt.codebook_digest.is_empty());
        assert!(!receipt.rotation_digest.is_empty());

        // Notes should be empty by default
        assert!(receipt.notes.is_empty());
    }

    #[test]
    fn test_run_benchmark_receipt_serializable() {
        let corpus = make_tiny_corpus();
        let quantizer = make_quantizer();
        let receipt = run_benchmark(&corpus, &quantizer).unwrap();
        let json = serde_json::to_string(&receipt).expect("should serialize to JSON");
        let restored: FibBenchmarkReceiptV1 =
            serde_json::from_str(&json).expect("should deserialize from JSON");
        assert_eq!(receipt, restored);
    }

    #[test]
    fn test_exact_topk_consistency() {
        let corpus = make_tiny_corpus();
        for (qi, query) in corpus.queries.iter().enumerate() {
            let computed = corpus.exact_topk(query, corpus.k).unwrap();
            let expected = &corpus.ground_truth_topk[qi];
            assert_eq!(
                computed, *expected,
                "exact_topk mismatch for query {}: computed {:?} vs expected {:?}",
                qi, computed, expected
            );
        }
    }
}
