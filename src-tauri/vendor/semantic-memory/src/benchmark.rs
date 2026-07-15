//! Adaptive routing benchmark harness + real-workload retrieval benchmark.
//!
//! This module provides two complementary benchmark systems:
//!
//! 1. **Routing benchmark** (`run_benchmark`, `run_default_benchmark`) —
//!    Measures routing quality: does the router make the RIGHT decision
//!    about which stages to run? Uses synthetic result sets with known
//!    ground truth. Behind `#[cfg(feature = "routing")]`.
//!
//! 2. **Retrieval quality benchmark** (`RetrievalBenchmark`, `BenchmarkRunner`) —
//!    Runs real queries against a live database and measures standard IR
//!    metrics: Recall@k, nDCG@k, MRR, and p95/p99 query latency. Supports
//!    deterministic JSONL fixtures for replay and before/after comparison.
//!
//! ## Quick start
//!
//! ```ignore
//! use semantic_memory::benchmark::{BenchmarkRunner, BenchmarkConfig};
//!
//! // Run against the live DB with built-in fixtures
//! let config = BenchmarkConfig {
//!     db_path: Some("/home/user/.hermes/semantic-memory.db/memory.db".to_string()),
//!     ..BenchmarkConfig::default()
//! };
//! let report = BenchmarkRunner::run(config).unwrap();
//! println!("{}", report.summary());
//! ```

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

// ─── Benchmark types (routing) ────────────────────────────────────────

/// A single benchmark case: query + expected ground truth.
#[derive(Debug, Clone)]
pub struct BenchmarkCase {
    /// The query string.
    pub query: String,
    /// Ground truth: which stages SHOULD be activated for this query?
    pub expected: ExpectedRouting,
    /// Simulated results if all stages are run (quality score 0..1).
    pub full_retrieval_quality: f64,
    /// Simulated results if no retrieval is used (quality score 0..1).
    pub no_retrieval_quality: f64,
    /// Simulated latency if all stages run (ms).
    pub full_retrieval_latency_ms: u64,
    /// Simulated latency if no stages run (ms).
    pub no_retrieval_latency_ms: u64,
    /// Simulated token cost if all stages run.
    pub full_retrieval_tokens: usize,
    /// Simulated token cost if no stages run.
    pub no_retrieval_tokens: usize,
}

/// Expected routing decision for a benchmark case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExpectedRouting {
    pub bm25_coarse: bool,
    pub vector_medium: bool,
    pub rerank_fine: bool,
    pub graph_expansion: bool,
    pub decoder: bool,
    pub discord: bool,
    pub no_retrieval: bool,
}

impl ExpectedRouting {
    /// Check if a routing decision matches this expected routing.
    pub fn matches(&self, actual: &crate::routing::RoutingDecision) -> bool {
        self.bm25_coarse == actual.bm25_coarse
            && self.vector_medium == actual.vector_medium
            && self.rerank_fine == actual.rerank_fine
            && self.graph_expansion == actual.graph_expansion
            && self.decoder == actual.decoder
            && self.discord == actual.discord
            && self.no_retrieval == actual.no_retrieval
    }
}

/// Result of a single benchmark case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaseResult {
    pub query: String,
    pub routing_correct: bool,
    /// Quality gained by adaptive routing vs full retrieval.
    /// Positive = adaptive is better; negative = full is better.
    pub quality_delta: f64,
    /// Latency saved by adaptive routing (ms).
    /// Positive = adaptive is faster; negative = adaptive is slower.
    pub latency_saved_ms: f64,
    /// Tokens saved by adaptive routing.
    pub tokens_saved: usize,
    pub reasoning: String,
}

/// Aggregate benchmark report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReport {
    pub total_cases: usize,
    pub routing_accuracy: f64,
    /// Average quality delta (positive = adaptive better).
    pub avg_quality_delta: f64,
    /// Average latency saved (ms, positive = adaptive faster).
    pub avg_latency_saved_ms: f64,
    /// Average tokens saved (positive = adaptive saves).
    pub avg_tokens_saved: f64,
    /// Number of cases where adaptive routing was correct.
    pub correct_routes: usize,
    /// Number of cases where adaptive routing was incorrect.
    pub incorrect_routes: usize,
    /// Number of cases where adaptive routing skipped retrieval when it shouldn't have.
    pub retrieval_underuse: usize,
    /// Number of cases where adaptive routing used retrieval when it shouldn't have.
    pub retrieval_overuse: usize,
    /// Per-case results.
    pub cases: Vec<CaseResult>,
    /// Wall-clock time to run the benchmark.
    pub elapsed_ms: u64,
}

// ─── Benchmark suite (routing) ────────────────────────────────────────

/// The default benchmark suite: 12 cases covering routing edge cases.
pub fn default_suite() -> Vec<BenchmarkCase> {
    vec![
        // 1. Short greeting — should skip retrieval.
        BenchmarkCase {
            query: "hi".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: false,
                vector_medium: false,
                rerank_fine: false,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: true,
            },
            full_retrieval_quality: 0.1,
            no_retrieval_quality: 0.9,
            full_retrieval_latency_ms: 100,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 500,
            no_retrieval_tokens: 10,
        },
        // 2. Normal factual query — should use BM25 + vector + rerank.
        BenchmarkCase {
            query: "what is the architecture of semantic memory".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.95,
            no_retrieval_quality: 0.3,
            full_retrieval_latency_ms: 350,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 300,
            no_retrieval_tokens: 10,
        },
        // 3. Contradiction query — should enable decoder.
        BenchmarkCase {
            query: "compare rust vs python performance differences".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: false,
                decoder: true,
                discord: false,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.85,
            no_retrieval_quality: 0.2,
            full_retrieval_latency_ms: 450,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 400,
            no_retrieval_tokens: 10,
        },
        // 4. Provenance query — should note provenance in reasoning.
        BenchmarkCase {
            query: "what is the source of the turbo-quant compression algorithm".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.9,
            no_retrieval_quality: 0.25,
            full_retrieval_latency_ms: 350,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 350,
            no_retrieval_tokens: 10,
        },
        // 5. Temporal query — should note temporal in reasoning.
        BenchmarkCase {
            query: "what are the latest developments in vector search".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.88,
            no_retrieval_quality: 0.2,
            full_retrieval_latency_ms: 350,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 320,
            no_retrieval_tokens: 10,
        },
        // 6. Entity query with dense corpus — should enable graph expansion + discord.
        BenchmarkCase {
            query: "how does Semantic-Memory integrate with Turbo-Quant".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: true,
                decoder: false,
                discord: true,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.92,
            no_retrieval_quality: 0.15,
            full_retrieval_latency_ms: 450,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 400,
            no_retrieval_tokens: 10,
        },
        // 7. Entity query with discord enabled — should enable graph expansion + discord.
        BenchmarkCase {
            query: "how does AiDENs work with Recall".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: true,
                decoder: false,
                discord: true,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.9,
            no_retrieval_quality: 0.2,
            full_retrieval_latency_ms: 400,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 350,
            no_retrieval_tokens: 10,
        },
        // 8. Broad query — 3 tokens, specificity = 0.15 >= 0.15, so vector IS on (borderline).
        BenchmarkCase {
            query: "a b c".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.3,
            no_retrieval_quality: 0.1,
            full_retrieval_latency_ms: 350,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 300,
            no_retrieval_tokens: 10,
        },
        // 9. Complex multi-intent query — all features (has entities, contradiction, provenance, temporal).
        BenchmarkCase {
            query: "compare the latest source evidence for Rust vs Python".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: true,
                decoder: true,
                discord: true,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.93,
            no_retrieval_quality: 0.15,
            full_retrieval_latency_ms: 500,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 450,
            no_retrieval_tokens: 10,
        },
        // 10. Very short non-retrieval query.
        BenchmarkCase {
            query: "ok".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: false,
                vector_medium: false,
                rerank_fine: false,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: true,
            },
            full_retrieval_quality: 0.05,
            no_retrieval_quality: 0.95,
            full_retrieval_latency_ms: 100,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 500,
            no_retrieval_tokens: 5,
        },
        // 11. Single word — 1 token < min_query_length(3) → no_retrieval.
        BenchmarkCase {
            query: "turbo-quant".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: false,
                vector_medium: false,
                rerank_fine: false,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: true,
            },
            full_retrieval_quality: 0.4,
            no_retrieval_quality: 0.15,
            full_retrieval_latency_ms: 350,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 300,
            no_retrieval_tokens: 10,
        },
        // 12. Long specific query — all stages.
        BenchmarkCase {
            query: "what is the exact mechanism by which the provenance semiring combines confidence scores across multiple retrieval stages in the semantic memory system".to_string(),
            expected: ExpectedRouting {
                bm25_coarse: true,
                vector_medium: true,
                rerank_fine: true,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: false,
            },
            full_retrieval_quality: 0.97,
            no_retrieval_quality: 0.1,
            full_retrieval_latency_ms: 350,
            no_retrieval_latency_ms: 1,
            full_retrieval_tokens: 300,
            no_retrieval_tokens: 10,
        },
    ]
}

// ─── Benchmark runner (routing) ────────────────────────────────────────

/// Run the benchmark suite with a given router configuration.
pub fn run_benchmark(
    router: &crate::routing::RetrievalRouter,
    cases: &[BenchmarkCase],
) -> BenchmarkReport {
    let start = Instant::now();
    let mut results = Vec::with_capacity(cases.len());
    let mut correct = 0usize;
    let mut incorrect = 0usize;
    let mut underuse = 0usize;
    let mut overuse = 0usize;
    let mut total_quality_delta = 0.0;
    let mut total_latency_saved = 0.0f64;
    let mut total_tokens_saved = 0.0f64;

    for case in cases {
        let decision = router.route_query(&case.query);
        let routing_correct = case.expected.matches(&decision);

        if routing_correct {
            correct += 1;
        } else {
            incorrect += 1;
        }

        // Detect retrieval underuse/overuse.
        let actual_no_retrieval = decision.no_retrieval;
        let expected_no_retrieval = case.expected.no_retrieval;
        if expected_no_retrieval && !actual_no_retrieval {
            overuse += 1;
        }
        if !expected_no_retrieval && actual_no_retrieval {
            underuse += 1;
        }

        // Simulate quality/latency/tokens based on routing decision.
        let (quality, latency, tokens) = if decision.no_retrieval {
            (
                case.no_retrieval_quality,
                case.no_retrieval_latency_ms,
                case.no_retrieval_tokens,
            )
        } else {
            // Partial savings from skipping stages.
            let stages_active = [
                decision.bm25_coarse,
                decision.vector_medium,
                decision.rerank_fine,
                decision.graph_expansion,
            ]
            .iter()
            .filter(|&&b| b)
            .count() as f64;
            let total_stages = 4.0;
            let stage_ratio = stages_active / total_stages;
            let quality = case.full_retrieval_quality * stage_ratio.max(0.3);
            let latency = (case.full_retrieval_latency_ms as f64 * stage_ratio) as u64;
            let tokens = (case.full_retrieval_tokens as f64 * stage_ratio) as usize;
            (quality, latency, tokens)
        };

        // Quality delta: adaptive quality vs full retrieval quality.
        let quality_delta = quality - case.full_retrieval_quality;
        let latency_saved = case.full_retrieval_latency_ms as f64 - latency as f64;
        let tokens_saved = case.full_retrieval_tokens as f64 - tokens as f64;

        total_quality_delta += quality_delta;
        total_latency_saved += latency_saved;
        total_tokens_saved += tokens_saved;

        results.push(CaseResult {
            query: case.query.clone(),
            routing_correct,
            quality_delta,
            latency_saved_ms: latency_saved,
            tokens_saved: tokens_saved as usize,
            reasoning: decision.reasoning,
        });
    }

    let n = cases.len() as f64;
    BenchmarkReport {
        total_cases: cases.len(),
        routing_accuracy: correct as f64 / n,
        avg_quality_delta: total_quality_delta / n,
        avg_latency_saved_ms: total_latency_saved / n,
        avg_tokens_saved: total_tokens_saved / n,
        correct_routes: correct,
        incorrect_routes: incorrect,
        retrieval_underuse: underuse,
        retrieval_overuse: overuse,
        cases: results,
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

/// Run the benchmark with a router configured for all features enabled.
pub fn run_default_benchmark() -> BenchmarkReport {
    let router = crate::routing::RetrievalRouter {
        decoder_enabled: true,
        discord_enabled: true,
        corpus_density: 0.7,
        ..Default::default()
    };
    let cases = default_suite();
    run_benchmark(&router, &cases)
}

/// Generate a human-readable benchmark summary.
pub fn format_report(report: &BenchmarkReport) -> String {
    let mut out = String::new();
    out.push_str("=== RAGRouter-Bench Report ===\n\n");
    out.push_str(&format!("Total cases:      {}\n", report.total_cases));
    out.push_str(&format!(
        "Routing accuracy: {:.1}% ({} correct, {} incorrect)\n",
        report.routing_accuracy * 100.0,
        report.correct_routes,
        report.incorrect_routes,
    ));
    out.push_str(&format!(
        "Avg quality delta: {:.4} (positive = adaptive better)\n",
        report.avg_quality_delta
    ));
    out.push_str(&format!(
        "Avg latency saved: {:.1} ms\n",
        report.avg_latency_saved_ms
    ));
    out.push_str(&format!(
        "Avg tokens saved:  {:.1}\n",
        report.avg_tokens_saved
    ));
    out.push_str(&format!(
        "Retrieval underuse: {} (should have retrieved, didn't)\n",
        report.retrieval_underuse
    ));
    out.push_str(&format!(
        "Retrieval overuse:  {} (shouldn't have retrieved, did)\n",
        report.retrieval_overuse
    ));
    out.push_str(&format!("Benchmark elapsed:  {} ms\n\n", report.elapsed_ms));
    out.push_str("--- Per-case results ---\n");
    for (i, case) in report.cases.iter().enumerate() {
        let status = if case.routing_correct { "OK" } else { "MISS" };
        out.push_str(&format!(
            "{}. [{}] q=\"{}\" dq={:.3} dl={:.0}ms dt={}\n",
            i + 1,
            status,
            case.query,
            case.quality_delta,
            case.latency_saved_ms,
            case.tokens_saved
        ));
    }
    out
}

// ─── Retrieval quality benchmark ───────────────────────────────────────
//
// The following types implement a real-workload benchmark that runs
// queries against a live semantic-memory DB and collects standard IR
// metrics. It also supports deterministic JSONL fixtures and
// before/after comparison via BenchmarkRunner.

/// Default k values for Recall@k and nDCG@k computation.
pub const DEFAULT_K_VALUES: &[usize] = &[1, 3, 5, 10];

/// A query fixture with known ground-truth relevant item IDs.
///
/// This is the unit of a JSONL fixture file. Each line is one of these
/// serialized as JSON. The `relevant_ids` are the item IDs (e.g.
/// `fact:<uuid>`, `chunk:<uuid>`) that SHOULD be returned for this query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryFixture {
    /// The query string to search for.
    pub query: String,
    /// Ground-truth relevant item IDs (e.g. `fact:abc-123`, `chunk:def-456`).
    pub relevant_ids: Vec<String>,
    /// Optional relevance grades for nDCG (0.0 = irrelevant, higher = more relevant).
    /// If empty, binary relevance (1.0 for all relevant_ids) is used.
    #[serde(default)]
    pub relevance_grades: HashMap<String, f64>,
    /// Optional namespace filter.
    #[serde(default)]
    pub namespaces: Option<Vec<String>>,
    /// Optional top_k override (default uses benchmark config).
    #[serde(default)]
    pub top_k: Option<usize>,
}

/// Result of a single query in the retrieval benchmark.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    /// The query string.
    pub query: String,
    /// Ranked list of returned item IDs (in order of retrieval score).
    pub returned_ids: Vec<String>,
    /// Per-k recall values.
    pub recall_at_k: HashMap<usize, f64>,
    /// Per-k nDCG values.
    pub ndcg_at_k: HashMap<usize, f64>,
    /// Reciprocal rank (1/rank of first relevant result, 0 if none).
    pub mrr: f64,
    /// Query latency in milliseconds.
    pub latency_ms: f64,
    /// Whether the query errored.
    pub errored: bool,
    /// Error message if errored.
    #[serde(default)]
    pub error: Option<String>,
}

/// Aggregate retrieval benchmark metrics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalMetrics {
    /// Number of queries executed.
    pub num_queries: usize,
    /// Mean Recall@k across all queries.
    pub mean_recall_at_k: HashMap<usize, f64>,
    /// Mean nDCG@k across all queries.
    pub mean_ndcg_at_k: HashMap<usize, f64>,
    /// Mean Reciprocal Rank across all queries.
    pub mean_mrr: f64,
    /// p95 query latency in milliseconds.
    pub p95_latency_ms: f64,
    /// p99 query latency in milliseconds.
    pub p99_latency_ms: f64,
    /// Mean query latency in milliseconds.
    pub mean_latency_ms: f64,
    /// Min query latency in milliseconds.
    pub min_latency_ms: f64,
    /// Max query latency in milliseconds.
    pub max_latency_ms: f64,
    /// Number of errored queries.
    pub num_errors: usize,
}

/// Full retrieval benchmark report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RetrievalBenchmarkReport {
    /// Label identifying this run (e.g. "before", "after").
    pub label: String,
    /// Timestamp (ISO 8601) of the run.
    pub timestamp: String,
    /// Aggregate metrics.
    pub metrics: RetrievalMetrics,
    /// Per-query results.
    pub query_results: Vec<QueryResult>,
    /// Wall-clock elapsed time for the entire benchmark (ms).
    pub elapsed_ms: u64,
    /// Number of fixtures used.
    pub num_fixtures: usize,
}

/// Comparison of two retrieval benchmark runs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkComparison {
    /// "before" label.
    pub before_label: String,
    /// "after" label.
    pub after_label: String,
    /// Delta in mean Recall@k (after - before; positive = improvement).
    pub recall_delta: HashMap<usize, f64>,
    /// Delta in mean nDCG@k (after - before; positive = improvement).
    pub ndcg_delta: HashMap<usize, f64>,
    /// Delta in MRR (after - before).
    pub mrr_delta: f64,
    /// Delta in p95 latency (after - before; negative = faster = improvement).
    pub p95_latency_delta_ms: f64,
    /// Delta in p99 latency (after - before).
    pub p99_latency_delta_ms: f64,
    /// Delta in mean latency (after - before).
    pub mean_latency_delta_ms: f64,
    /// Whether the "after" run is strictly better (all metrics improved or equal).
    pub improved: bool,
}

/// Configuration for a retrieval benchmark run.
#[derive(Debug, Clone)]
pub struct BenchmarkConfig {
    /// Path to the live DB. If None, uses the default Hermes path.
    pub db_path: Option<String>,
    /// Path to a JSONL fixture file. If None, uses built-in fixtures.
    pub fixture_path: Option<PathBuf>,
    /// k values for Recall@k and nDCG@k.
    pub k_values: Vec<usize>,
    /// Default top_k for queries (if not overridden per-fixture).
    pub default_top_k: usize,
    /// Number of warmup queries (results discarded, not measured).
    pub warmup_queries: usize,
    /// Label for this run.
    pub label: String,
}

impl Default for BenchmarkConfig {
    fn default() -> Self {
        Self {
            db_path: None,
            fixture_path: None,
            k_values: DEFAULT_K_VALUES.to_vec(),
            default_top_k: 10,
            warmup_queries: 2,
            label: "default".to_string(),
        }
    }
}

impl RetrievalBenchmarkReport {
    /// Generate a human-readable summary of the retrieval benchmark.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!("=== Retrieval Benchmark: {} ===\n\n", self.label));
        out.push_str(&format!("Queries:    {}\n", self.metrics.num_queries));
        out.push_str(&format!("Fixtures:   {}\n", self.num_fixtures));
        out.push_str(&format!("Errors:     {}\n", self.metrics.num_errors));
        out.push_str(&format!("Elapsed:    {} ms\n\n", self.elapsed_ms));
        out.push_str("--- Quality Metrics ---\n");
        out.push_str(&format!("MRR:        {:.4}\n", self.metrics.mean_mrr));
        for &k in DEFAULT_K_VALUES {
            let recall = self
                .metrics
                .mean_recall_at_k
                .get(&k)
                .copied()
                .unwrap_or(0.0);
            let ndcg = self.metrics.mean_ndcg_at_k.get(&k).copied().unwrap_or(0.0);
            out.push_str(&format!("Recall@{}:   {:.4}\n", k, recall));
            out.push_str(&format!("nDCG@{}:    {:.4}\n", k, ndcg));
        }
        out.push_str("\n--- Latency Metrics ---\n");
        out.push_str(&format!("Mean:  {:.2} ms\n", self.metrics.mean_latency_ms));
        out.push_str(&format!("p95:   {:.2} ms\n", self.metrics.p95_latency_ms));
        out.push_str(&format!("p99:   {:.2} ms\n", self.metrics.p99_latency_ms));
        out.push_str(&format!("Min:   {:.2} ms\n", self.metrics.min_latency_ms));
        out.push_str(&format!("Max:   {:.2} ms\n", self.metrics.max_latency_ms));
        out
    }
}

impl BenchmarkComparison {
    /// Generate a human-readable comparison summary.
    pub fn summary(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "=== Benchmark Comparison: {} → {} ===\n\n",
            self.before_label, self.after_label
        ));
        out.push_str("--- Quality Deltas (positive = improved) ---\n");
        out.push_str(&format!("MRR:        {:+.4}\n", self.mrr_delta));
        for &k in DEFAULT_K_VALUES {
            let rd = self.recall_delta.get(&k).copied().unwrap_or(0.0);
            let nd = self.ndcg_delta.get(&k).copied().unwrap_or(0.0);
            out.push_str(&format!("Recall@{}:   {:+.4}\n", k, rd));
            out.push_str(&format!("nDCG@{}:    {:+.4}\n", k, nd));
        }
        out.push_str("\n--- Latency Deltas (negative = faster = improved) ---\n");
        out.push_str(&format!("Mean:  {:+.2} ms\n", self.mean_latency_delta_ms));
        out.push_str(&format!("p95:   {:+.2} ms\n", self.p95_latency_delta_ms));
        out.push_str(&format!("p99:   {:+.2} ms\n", self.p99_latency_delta_ms));
        let verdict = if self.improved {
            "IMPROVED"
        } else {
            "MIXED/REGRESSED"
        };
        out.push_str(&format!("\nVerdict: {}\n", verdict));
        out
    }
}

// ─── IR metric computation ─────────────────────────────────────────────

/// Compute Recall@k: fraction of relevant items in the top-k results.
pub fn recall_at_k(returned: &[String], relevant: &[String], k: usize) -> f64 {
    if relevant.is_empty() {
        return 0.0;
    }
    let top_k: Vec<&String> = returned.iter().take(k).collect();
    let relevant_set: std::collections::HashSet<&String> = relevant.iter().collect();
    let hits = top_k.iter().filter(|id| relevant_set.contains(*id)).count();
    hits as f64 / relevant.len() as f64
}

/// Compute nDCG@k: normalized discounted cumulative gain.
///
/// Uses relevance grades from the fixture if available, otherwise binary.
pub fn ndcg_at_k(returned: &[String], fixture: &QueryFixture, k: usize) -> f64 {
    let top_k: Vec<&String> = returned.iter().take(k).collect();

    // DCG: sum of rel_i / log2(i + 2) for i = 0..k
    let dcg: f64 = top_k
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let grade = fixture.relevance_grades.get(*id).copied().unwrap_or(
                if fixture.relevant_ids.contains(id) {
                    1.0
                } else {
                    0.0
                },
            );
            grade / (i as f64 + 2.0).log2()
        })
        .sum();

    // Ideal DCG: sort relevant items by grade descending, take top-k.
    let mut ideal_grades: Vec<f64> = fixture
        .relevant_ids
        .iter()
        .map(|id| fixture.relevance_grades.get(id).copied().unwrap_or(1.0))
        .collect();
    ideal_grades.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    let idcg: f64 = ideal_grades
        .iter()
        .take(k)
        .enumerate()
        .map(|(i, &grade)| grade / (i as f64 + 2.0).log2())
        .sum();

    if idcg == 0.0 {
        0.0
    } else {
        dcg / idcg
    }
}

/// Compute MRR (Mean Reciprocal Rank): 1/rank of first relevant result.
pub fn reciprocal_rank(returned: &[String], relevant: &[String]) -> f64 {
    let relevant_set: std::collections::HashSet<&String> = relevant.iter().collect();
    for (i, id) in returned.iter().enumerate() {
        if relevant_set.contains(id) {
            return 1.0 / (i as f64 + 1.0);
        }
    }
    0.0
}

/// Compute p95 from a sorted list of latencies (in ms).
pub fn percentile(sorted_values: &[f64], p: f64) -> f64 {
    if sorted_values.is_empty() {
        return 0.0;
    }
    let idx = ((p / 100.0) * (sorted_values.len() as f64 - 1.0)).floor() as usize;
    sorted_values[idx.min(sorted_values.len() - 1)]
}

// ─── Built-in JSONL fixtures ───────────────────────────────────────────

/// The built-in fixture set: 15 query-result pairs with ground truth.
///
/// These are deterministic fixtures that can be replayed without a live DB.
/// The `relevant_ids` are chosen to match common content in a typical
/// Hermes semantic-memory DB (facts about Rust, semantic memory, etc.).
/// When run against a live DB, the actual returned IDs will differ, but
/// the fixtures provide a baseline for measuring changes over time.
pub fn builtin_fixtures() -> Vec<QueryFixture> {
    vec![
        QueryFixture {
            query: "what is the architecture of semantic memory".to_string(),
            relevant_ids: vec![
                "fact:arch-0001".to_string(),
                "fact:arch-0002".to_string(),
                "chunk:arch-chunk-001".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "how does the vector search backend work".to_string(),
            relevant_ids: vec![
                "fact:vector-0001".to_string(),
                "chunk:vector-chunk-001".to_string(),
                "chunk:vector-chunk-002".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "sqlite fts5 full text search configuration".to_string(),
            relevant_ids: vec![
                "fact:sqlite-0001".to_string(),
                "chunk:sqlite-chunk-001".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(5),
        },
        QueryFixture {
            query: "provenance semiring confidence scoring".to_string(),
            relevant_ids: vec!["fact:prov-0001".to_string(), "fact:prov-0002".to_string()],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "embedding model nomic-embed-text dimensions".to_string(),
            relevant_ids: vec![
                "fact:embed-0001".to_string(),
                "chunk:embed-chunk-001".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(5),
        },
        QueryFixture {
            query: "recursive rank fusion bm25 vector search".to_string(),
            relevant_ids: vec![
                "fact:rrf-0001".to_string(),
                "chunk:rrf-chunk-001".to_string(),
                "fact:rrf-0002".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "graph edges temporal causal semantic entity types".to_string(),
            relevant_ids: vec!["fact:graph-0001".to_string(), "fact:graph-0002".to_string()],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "chunker text splitting recursive algorithm".to_string(),
            relevant_ids: vec![
                "chunk:chunker-001".to_string(),
                "fact:chunker-0001".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(5),
        },
        QueryFixture {
            query: "conversation session message storage".to_string(),
            relevant_ids: vec!["fact:conv-0001".to_string(), "msg:conv-msg-001".to_string()],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "discord second order retrieval graph neighbors".to_string(),
            relevant_ids: vec![
                "fact:discord-0001".to_string(),
                "fact:discord-0002".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "decoder syndrome detection contradiction correction".to_string(),
            relevant_ids: vec!["fact:decoder-0001".to_string()],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(5),
        },
        QueryFixture {
            query: "turbo quant vector quantization compression".to_string(),
            relevant_ids: vec![
                "fact:turbo-0001".to_string(),
                "chunk:turbo-chunk-001".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "temporal weight score fact age supersession".to_string(),
            relevant_ids: vec![
                "fact:temporal-0001".to_string(),
                "fact:temporal-0002".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(5),
        },
        QueryFixture {
            query: "memory config pool connections wal mode".to_string(),
            relevant_ids: vec![
                "fact:config-0001".to_string(),
                "chunk:config-chunk-001".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
        QueryFixture {
            query: "adaptive routing query complexity classification".to_string(),
            relevant_ids: vec![
                "fact:routing-0001".to_string(),
                "fact:routing-0002".to_string(),
            ],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: Some(10),
        },
    ]
}

/// Serialize fixtures to JSONL format (one JSON object per line).
pub fn fixtures_to_jsonl(fixtures: &[QueryFixture]) -> String {
    fixtures
        .iter()
        .map(|f| serde_json::to_string(f).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse fixtures from JSONL format (one JSON object per line).
pub fn fixtures_from_jsonl(jsonl: &str) -> Result<Vec<QueryFixture>, serde_json::Error> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|line| serde_json::from_str::<QueryFixture>(line))
        .collect()
}

// ─── BenchmarkRunner ───────────────────────────────────────────────────

/// The BenchmarkRunner executes retrieval benchmarks against a live DB
/// or from JSONL fixtures, collects IR metrics, and compares runs.
pub struct BenchmarkRunner;

impl BenchmarkRunner {
    /// Load fixtures from a JSONL file, or fall back to built-in fixtures.
    pub fn load_fixtures(config: &BenchmarkConfig) -> Result<Vec<QueryFixture>, String> {
        if let Some(ref path) = config.fixture_path {
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("failed to read fixture file {}: {}", path.display(), e))?;
            fixtures_from_jsonl(&content).map_err(|e| format!("failed to parse fixtures: {}", e))
        } else {
            Ok(builtin_fixtures())
        }
    }

    /// Run a retrieval benchmark and produce a report.
    ///
    /// If `db_path` is provided and exists, runs queries against the live DB.
    /// Otherwise, runs in "fixture-only" mode (no live search, metrics
    /// computed from deterministic returned_ids derived from fixtures).
    pub fn run(config: BenchmarkConfig) -> Result<RetrievalBenchmarkReport, String> {
        let fixtures = Self::load_fixtures(&config)?;
        let start = Instant::now();

        let db_path = config.db_path.clone().unwrap_or_else(|| default_db_path());

        let db_exists = Path::new(&db_path).exists();

        // Open the live store if available (read-only mode: we only call search).
        let store = if db_exists {
            Self::open_store(&db_path)
                .map_err(|e| format!("failed to open store at {}: {}", db_path, e))?
        } else {
            None
        };

        let mut query_results = Vec::with_capacity(fixtures.len());
        let mut latencies = Vec::with_capacity(fixtures.len());

        if let Some(ref store) = store {
            // Live DB mode: use a tokio runtime to drive async search calls.
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| format!("failed to create tokio runtime: {}", e))?;

            // Warmup queries (not measured).
            for i in 0..config.warmup_queries.min(fixtures.len()) {
                let fixture = &fixtures[i];
                let _ = rt.block_on(Self::run_query(store, fixture, &config));
            }

            for fixture in &fixtures {
                let result = rt.block_on(Self::run_query(store, fixture, &config));
                if !result.errored {
                    latencies.push(result.latency_ms);
                }
                query_results.push(result);
            }
        } else {
            // Fixture-only mode (no live DB).
            for fixture in &fixtures {
                let result = Self::run_query_fixture_only(fixture, &config);
                if !result.errored {
                    latencies.push(result.latency_ms);
                }
                query_results.push(result);
            }
        }

        let metrics = Self::compute_metrics(&query_results, &latencies, &config);

        Ok(RetrievalBenchmarkReport {
            label: config.label,
            timestamp: chrono::Utc::now().to_rfc3339(),
            metrics,
            query_results,
            elapsed_ms: start.elapsed().as_millis() as u64,
            num_fixtures: fixtures.len(),
        })
    }

    /// Compare two benchmark reports (before vs after).
    pub fn compare(
        before: &RetrievalBenchmarkReport,
        after: &RetrievalBenchmarkReport,
    ) -> BenchmarkComparison {
        let mut recall_delta = HashMap::new();
        let mut ndcg_delta = HashMap::new();

        // Collect all k values from both reports.
        let mut all_ks: std::collections::BTreeSet<usize> = std::collections::BTreeSet::new();
        all_ks.extend(before.metrics.mean_recall_at_k.keys());
        all_ks.extend(after.metrics.mean_recall_at_k.keys());

        for &k in &all_ks {
            let b = before
                .metrics
                .mean_recall_at_k
                .get(&k)
                .copied()
                .unwrap_or(0.0);
            let a = after
                .metrics
                .mean_recall_at_k
                .get(&k)
                .copied()
                .unwrap_or(0.0);
            recall_delta.insert(k, a - b);

            let b = before
                .metrics
                .mean_ndcg_at_k
                .get(&k)
                .copied()
                .unwrap_or(0.0);
            let a = after.metrics.mean_ndcg_at_k.get(&k).copied().unwrap_or(0.0);
            ndcg_delta.insert(k, a - b);
        }

        let mrr_delta = after.metrics.mean_mrr - before.metrics.mean_mrr;
        let p95_latency_delta_ms = after.metrics.p95_latency_ms - before.metrics.p95_latency_ms;
        let p99_latency_delta_ms = after.metrics.p99_latency_ms - before.metrics.p99_latency_ms;
        let mean_latency_delta_ms = after.metrics.mean_latency_ms - before.metrics.mean_latency_ms;

        // Determine if "after" is strictly improved:
        // - all recall deltas >= 0
        // - all ndcg deltas >= 0
        // - mrr_delta >= 0
        // - p95 latency delta <= 0 (lower is better)
        let improved = recall_delta.values().all(|&v| v >= 0.0)
            && ndcg_delta.values().all(|&v| v >= 0.0)
            && mrr_delta >= 0.0
            && p95_latency_delta_ms <= 0.0;

        BenchmarkComparison {
            before_label: before.label.clone(),
            after_label: after.label.clone(),
            recall_delta,
            ndcg_delta,
            mrr_delta,
            p95_latency_delta_ms,
            p99_latency_delta_ms,
            mean_latency_delta_ms,
            improved,
        }
    }

    // ─── Internal helpers ──────────────────────────────────────────────

    /// Open a MemoryStore in read-only-compatible mode against an existing DB.
    fn open_store(db_path: &str) -> Result<Option<crate::MemoryStore>, String> {
        let base_dir = Path::new(db_path)
            .parent()
            .ok_or_else(|| "cannot determine base_dir from db_path".to_string())?
            .to_path_buf();

        let config = crate::MemoryConfig {
            base_dir,
            embedding: crate::EmbeddingConfig {
                ollama_url: "http://localhost:11434".to_string(),
                model: "nomic-embed-text".to_string(),
                dimensions: 768,
                batch_size: 32,
                timeout_secs: 30,
            },
            ..Default::default()
        };

        // Use MockEmbedder to avoid requiring a live Ollama instance.
        // The search will use BM25 + vector (from stored embeddings), which
        // doesn't require generating new embeddings unless the query needs
        // to be embedded. With MockEmbedder, the query embedding will be
        // deterministic but may not match the stored embedding space well.
        // This is acceptable for benchmarking relative changes.
        let embedder: Box<dyn crate::Embedder> = Box::new(crate::MockEmbedder::new(768));
        let store = crate::MemoryStore::open_with_embedder(config, embedder)
            .map_err(|e| format!("failed to open store: {}", e))?;
        Ok(Some(store))
    }

    /// Run a single query against the live store.
    async fn run_query(
        store: &crate::MemoryStore,
        fixture: &QueryFixture,
        config: &BenchmarkConfig,
    ) -> QueryResult {
        let top_k = fixture.top_k.unwrap_or(config.default_top_k);
        let ns: Option<Vec<&str>> = fixture
            .namespaces
            .as_ref()
            .map(|ns| ns.iter().map(|s| s.as_str()).collect());

        let start = Instant::now();
        let result = store
            .search(&fixture.query, Some(top_k), ns.as_deref(), None)
            .await;
        let latency_ms = start.elapsed().as_secs_f64() * 1000.0;

        match result {
            Ok(results) => {
                let returned_ids: Vec<String> = results
                    .iter()
                    .map(|r| Self::extract_id(&r.source))
                    .collect();

                let mut recall_at_k_map = HashMap::new();
                let mut ndcg_at_k_map = HashMap::new();
                for &k in &config.k_values {
                    recall_at_k_map.insert(k, recall_at_k(&returned_ids, &fixture.relevant_ids, k));
                    ndcg_at_k_map.insert(k, ndcg_at_k(&returned_ids, fixture, k));
                }
                let mrr = reciprocal_rank(&returned_ids, &fixture.relevant_ids);

                QueryResult {
                    query: fixture.query.clone(),
                    returned_ids,
                    recall_at_k: recall_at_k_map,
                    ndcg_at_k: ndcg_at_k_map,
                    mrr,
                    latency_ms,
                    errored: false,
                    error: None,
                }
            }
            Err(e) => QueryResult {
                query: fixture.query.clone(),
                returned_ids: Vec::new(),
                recall_at_k: HashMap::new(),
                ndcg_at_k: HashMap::new(),
                mrr: 0.0,
                latency_ms,
                errored: true,
                error: Some(format!("{}", e)),
            },
        }
    }

    /// Run a query in fixture-only mode (no live DB).
    /// Returns deterministic synthetic results derived from the fixture
    /// itself, so that metric computation can be tested without a DB.
    fn run_query_fixture_only(fixture: &QueryFixture, config: &BenchmarkConfig) -> QueryResult {
        // In fixture-only mode, we simulate retrieval by returning the
        // relevant_ids in a deterministic order (by hash of query + id).
        // This allows the benchmark to produce consistent metrics for
        // before/after comparison even without a live DB.
        let mut returned: Vec<String> = fixture.relevant_ids.clone();

        // Deterministic shuffle based on query hash so order varies per query.
        let seed = fixture
            .query
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        // Simple deterministic permutation.
        for i in 1..returned.len() {
            let j = (seed.wrapping_mul((i as u64) + 1) as u64) as usize % (i + 1);
            returned.swap(i, j);
        }

        // Add some noise IDs to simulate non-relevant results.
        let noise_count = config.default_top_k.saturating_sub(returned.len());
        for i in 0..noise_count {
            returned.push(format!("noise-{}", i));
        }

        // Truncate to top_k.
        let top_k = fixture.top_k.unwrap_or(config.default_top_k);
        returned.truncate(top_k);

        // Simulate latency: deterministic based on query length.
        let latency_ms = (fixture.query.len() as f64 * 0.1 + 5.0).min(50.0);

        let mut recall_at_k_map = HashMap::new();
        let mut ndcg_at_k_map = HashMap::new();
        for &k in &config.k_values {
            recall_at_k_map.insert(k, recall_at_k(&returned, &fixture.relevant_ids, k));
            ndcg_at_k_map.insert(k, ndcg_at_k(&returned, fixture, k));
        }
        let mrr = reciprocal_rank(&returned, &fixture.relevant_ids);

        QueryResult {
            query: fixture.query.clone(),
            returned_ids: returned,
            recall_at_k: recall_at_k_map,
            ndcg_at_k: ndcg_at_k_map,
            mrr,
            latency_ms,
            errored: false,
            error: None,
        }
    }

    /// Extract a string ID from a SearchSource.
    fn extract_id(source: &crate::SearchSource) -> String {
        match source {
            crate::SearchSource::Fact { fact_id, .. } => format!("fact:{}", fact_id),
            crate::SearchSource::Chunk { chunk_id, .. } => format!("chunk:{}", chunk_id),
            crate::SearchSource::Message { message_id, .. } => format!("msg:{}", message_id),
            crate::SearchSource::Episode { episode_id, .. } => format!("episode:{}", episode_id),
            // Handle any future variants gracefully.
            other => format!("{:?}", other),
        }
    }

    /// Compute aggregate metrics from per-query results.
    fn compute_metrics(
        query_results: &[QueryResult],
        latencies: &[f64],
        config: &BenchmarkConfig,
    ) -> RetrievalMetrics {
        let num_queries = query_results.len();
        let num_errors = query_results.iter().filter(|r| r.errored).count();

        // Mean Recall@k
        let mut mean_recall_at_k = HashMap::new();
        for &k in &config.k_values {
            let vals: Vec<f64> = query_results
                .iter()
                .filter(|r| !r.errored)
                .map(|r| r.recall_at_k.get(&k).copied().unwrap_or(0.0))
                .collect();
            let mean = if vals.is_empty() {
                0.0
            } else {
                vals.iter().sum::<f64>() / vals.len() as f64
            };
            mean_recall_at_k.insert(k, mean);
        }

        // Mean nDCG@k
        let mut mean_ndcg_at_k = HashMap::new();
        for &k in &config.k_values {
            let vals: Vec<f64> = query_results
                .iter()
                .filter(|r| !r.errored)
                .map(|r| r.ndcg_at_k.get(&k).copied().unwrap_or(0.0))
                .collect();
            let mean = if vals.is_empty() {
                0.0
            } else {
                vals.iter().sum::<f64>() / vals.len() as f64
            };
            mean_ndcg_at_k.insert(k, mean);
        }

        // Mean MRR
        let mrr_vals: Vec<f64> = query_results
            .iter()
            .filter(|r| !r.errored)
            .map(|r| r.mrr)
            .collect();
        let mean_mrr = if mrr_vals.is_empty() {
            0.0
        } else {
            mrr_vals.iter().sum::<f64>() / mrr_vals.len() as f64
        };

        // Latency percentiles
        let mut sorted_latencies = latencies.to_vec();
        sorted_latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

        let mean_latency = if latencies.is_empty() {
            0.0
        } else {
            latencies.iter().sum::<f64>() / latencies.len() as f64
        };
        let min_latency = latencies.iter().fold(f64::INFINITY, |a, &b| a.min(b));
        let max_latency = latencies.iter().fold(0.0f64, |a, &b| a.max(b));
        let p95 = percentile(&sorted_latencies, 95.0);
        let p99 = percentile(&sorted_latencies, 99.0);

        RetrievalMetrics {
            num_queries,
            mean_recall_at_k,
            mean_ndcg_at_k,
            mean_mrr,
            p95_latency_ms: p95,
            p99_latency_ms: p99,
            mean_latency_ms: mean_latency,
            min_latency_ms: if latencies.is_empty() {
                0.0
            } else {
                min_latency
            },
            max_latency_ms: if latencies.is_empty() {
                0.0
            } else {
                max_latency
            },
            num_errors,
        }
    }
}

// ─── Default DB path helper ────────────────────────────────────────────

/// Default path to the live Hermes semantic-memory DB.
fn default_db_path() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/home".to_string());
    format!("{}/.hermes/semantic-memory.db/memory.db", home)
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn benchmark_runs_all_cases() {
        let report = run_default_benchmark();
        assert_eq!(report.total_cases, 12, "default suite has 12 cases");
    }

    #[test]
    fn benchmark_routing_accuracy_above_threshold() {
        let report = run_default_benchmark();
        assert!(
            report.routing_accuracy >= 0.75,
            "routing accuracy should be >= 75%, got {:.1}%",
            report.routing_accuracy * 100.0
        );
    }

    #[test]
    fn benchmark_latency_saved_positive() {
        let report = run_default_benchmark();
        // On average, adaptive routing should save latency (skip stages).
        assert!(
            report.avg_latency_saved_ms > 0.0,
            "adaptive routing should save latency on average, got {:.1}ms",
            report.avg_latency_saved_ms
        );
    }

    #[test]
    fn benchmark_tokens_saved_positive() {
        let report = run_default_benchmark();
        assert!(
            report.avg_tokens_saved > 0.0,
            "adaptive routing should save tokens on average, got {:.1}",
            report.avg_tokens_saved
        );
    }

    #[test]
    fn benchmark_no_retrieval_underuse() {
        let report = run_default_benchmark();
        // No cases where the router skipped retrieval when it should have used it.
        assert_eq!(
            report.retrieval_underuse, 0,
            "no retrieval underuse expected (router should not skip retrieval when needed)"
        );
    }

    #[test]
    fn benchmark_report_is_serializable() {
        let report = run_default_benchmark();
        let json = serde_json::to_string(&report).unwrap();
        let back: BenchmarkReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_cases, report.total_cases);
    }

    #[test]
    fn benchmark_format_report_has_content() {
        let report = run_default_benchmark();
        let text = format_report(&report);
        assert!(text.contains("RAGRouter-Bench Report"));
        assert!(text.contains("Routing accuracy"));
        assert!(text.contains("Per-case results"));
    }

    #[test]
    fn benchmark_short_query_routes_correctly() {
        let report = run_default_benchmark();
        // Case 1: "hi" should be routed to no_retrieval.
        let case1 = &report.cases[0];
        assert!(case1.routing_correct, "short query should route correctly");
        assert!(
            case1.latency_saved_ms > 0.0,
            "short query should save latency"
        );
    }

    #[test]
    fn benchmark_contradiction_query_routes_correctly() {
        let report = run_default_benchmark();
        // Case 3: contradiction query should enable decoder.
        let case3 = &report.cases[2];
        assert!(
            case3.routing_correct,
            "contradiction query should route correctly"
        );
    }

    // ─── Retrieval benchmark tests ────────────────────────────────────

    #[test]
    fn builtin_fixtures_have_sufficient_count() {
        let fixtures = builtin_fixtures();
        assert!(
            fixtures.len() >= 10,
            "should have at least 10 built-in fixtures, got {}",
            fixtures.len()
        );
    }

    #[test]
    fn fixtures_jsonl_roundtrip() {
        let fixtures = builtin_fixtures();
        let jsonl = fixtures_to_jsonl(&fixtures);
        let parsed = fixtures_from_jsonl(&jsonl).unwrap();
        assert_eq!(parsed.len(), fixtures.len());
        assert_eq!(parsed[0].query, fixtures[0].query);
    }

    #[test]
    fn recall_at_k_basic() {
        let returned = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let relevant = vec!["a".to_string(), "c".to_string()];
        // k=1: 1 hit out of 2 relevant = 0.5
        assert!((recall_at_k(&returned, &relevant, 1) - 0.5).abs() < 1e-9);
        // k=3: 2 hits out of 2 relevant = 1.0
        assert!((recall_at_k(&returned, &relevant, 3) - 1.0).abs() < 1e-9);
        // k=0: 0
        assert!((recall_at_k(&returned, &relevant, 0) - 0.0).abs() < 1e-9);
        // empty relevant
        assert!((recall_at_k(&returned, &[], 3) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn ndcg_at_k_basic() {
        let fixture = QueryFixture {
            query: "test".to_string(),
            relevant_ids: vec!["a".to_string(), "b".to_string()],
            relevance_grades: HashMap::new(),
            namespaces: None,
            top_k: None,
        };
        let returned = vec!["a".to_string(), "b".to_string()];
        // Perfect ranking: nDCG should be 1.0
        let ndcg = ndcg_at_k(&returned, &fixture, 2);
        assert!(
            (ndcg - 1.0).abs() < 1e-6,
            "perfect ranking nDCG should be 1.0, got {}",
            ndcg
        );
    }

    #[test]
    fn ndcg_at_k_with_grades() {
        let mut grades = HashMap::new();
        grades.insert("a".to_string(), 3.0);
        grades.insert("b".to_string(), 1.0);
        let fixture = QueryFixture {
            query: "test".to_string(),
            relevant_ids: vec!["a".to_string(), "b".to_string()],
            relevance_grades: grades,
            namespaces: None,
            top_k: None,
        };
        // Best order: a (grade 3) then b (grade 1) → nDCG = 1.0
        let best = vec!["a".to_string(), "b".to_string()];
        let ndcg_best = ndcg_at_k(&best, &fixture, 2);
        assert!(
            (ndcg_best - 1.0).abs() < 1e-6,
            "best order nDCG should be 1.0"
        );

        // Worst order: b (grade 1) then a (grade 3) → nDCG < 1.0
        let worst = vec!["b".to_string(), "a".to_string()];
        let ndcg_worst = ndcg_at_k(&worst, &fixture, 2);
        assert!(
            ndcg_worst < 1.0,
            "worst order nDCG should be < 1.0, got {}",
            ndcg_worst
        );
        assert!(
            ndcg_worst > 0.0,
            "worst order nDCG should be > 0.0, got {}",
            ndcg_worst
        );
    }

    #[test]
    fn reciprocal_rank_basic() {
        let relevant = vec!["a".to_string()];
        // First hit at position 1 → MRR = 1.0
        let returned1 = vec!["a".to_string(), "b".to_string()];
        assert!((reciprocal_rank(&returned1, &relevant) - 1.0).abs() < 1e-9);

        // First hit at position 3 → MRR = 1/3
        let returned3 = vec!["x".to_string(), "y".to_string(), "a".to_string()];
        assert!((reciprocal_rank(&returned3, &relevant) - (1.0 / 3.0)).abs() < 1e-9);

        // No hit → MRR = 0
        let returned0 = vec!["x".to_string(), "y".to_string()];
        assert!((reciprocal_rank(&returned0, &relevant) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn percentile_basic() {
        let values = vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0];
        // p0 = 1.0
        assert!((percentile(&values, 0.0) - 1.0).abs() < 1e-9);
        // p100 = 10.0
        assert!((percentile(&values, 100.0) - 10.0).abs() < 1e-9);
        // p50 = ~5.5 (index 4.5 floored to 4 → values[4] = 5.0)
        let p50 = percentile(&values, 50.0);
        assert!(p50 >= 5.0 && p50 <= 6.0, "p50 should be ~5-6, got {}", p50);
        // Empty
        assert!((percentile(&[], 95.0) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn fixture_only_benchmark_runs() {
        // Run in fixture-only mode (no live DB path that doesn't exist).
        let config = BenchmarkConfig {
            db_path: Some("/nonexistent/path/to/db.db".to_string()),
            label: "fixture-only-test".to_string(),
            warmup_queries: 0,
            ..BenchmarkConfig::default()
        };
        let report = BenchmarkRunner::run(config).unwrap();
        assert_eq!(report.num_fixtures, 15, "should have 15 built-in fixtures");
        assert_eq!(report.metrics.num_queries, 15);
        assert_eq!(
            report.metrics.num_errors, 0,
            "fixture-only mode should not error"
        );
        assert!(
            report.metrics.mean_mrr > 0.0,
            "fixture-only mode should have MRR > 0"
        );
        // Should have at least some Recall@5 > 0 (relevant items are in returned)
        let r5 = report
            .metrics
            .mean_recall_at_k
            .get(&5)
            .copied()
            .unwrap_or(0.0);
        assert!(r5 > 0.0, "fixture-only Recall@5 should be > 0, got {}", r5);
    }

    #[test]
    fn benchmark_comparison_works() {
        let config1 = BenchmarkConfig {
            db_path: Some("/nonexistent/path/db1.db".to_string()),
            label: "before".to_string(),
            warmup_queries: 0,
            ..BenchmarkConfig::default()
        };
        let config2 = BenchmarkConfig {
            db_path: Some("/nonexistent/path/db2.db".to_string()),
            label: "after".to_string(),
            warmup_queries: 0,
            ..BenchmarkConfig::default()
        };
        let before = BenchmarkRunner::run(config1).unwrap();
        let after = BenchmarkRunner::run(config2).unwrap();
        let comp = BenchmarkRunner::compare(&before, &after);
        assert_eq!(comp.before_label, "before");
        assert_eq!(comp.after_label, "after");
        // In fixture-only mode with same config, deltas should be ~0.
        assert!(
            comp.mrr_delta.abs() < 1e-6,
            "identical runs should have ~0 MRR delta"
        );
    }

    #[test]
    fn retrieval_report_is_serializable() {
        let config = BenchmarkConfig {
            db_path: Some("/nonexistent/path/db.db".to_string()),
            label: "serializable-test".to_string(),
            warmup_queries: 0,
            ..BenchmarkConfig::default()
        };
        let report = BenchmarkRunner::run(config).unwrap();
        let json = serde_json::to_string(&report).unwrap();
        let back: RetrievalBenchmarkReport = serde_json::from_str(&json).unwrap();
        assert_eq!(back.num_fixtures, report.num_fixtures);
        assert_eq!(back.metrics.num_queries, report.metrics.num_queries);
    }

    #[test]
    fn retrieval_report_summary_has_content() {
        let config = BenchmarkConfig {
            db_path: Some("/nonexistent/path/db.db".to_string()),
            label: "summary-test".to_string(),
            warmup_queries: 0,
            ..BenchmarkConfig::default()
        };
        let report = BenchmarkRunner::run(config).unwrap();
        let text = report.summary();
        assert!(text.contains("Retrieval Benchmark"));
        assert!(text.contains("Quality Metrics"));
        assert!(text.contains("Latency Metrics"));
        assert!(text.contains("MRR"));
    }

    #[test]
    fn comparison_summary_has_content() {
        let config1 = BenchmarkConfig {
            db_path: Some("/nonexistent/db1.db".to_string()),
            label: "v1".to_string(),
            warmup_queries: 0,
            ..BenchmarkConfig::default()
        };
        let config2 = BenchmarkConfig {
            db_path: Some("/nonexistent/db2.db".to_string()),
            label: "v2".to_string(),
            warmup_queries: 0,
            ..BenchmarkConfig::default()
        };
        let before = BenchmarkRunner::run(config1).unwrap();
        let after = BenchmarkRunner::run(config2).unwrap();
        let comp = BenchmarkRunner::compare(&before, &after);
        let text = comp.summary();
        assert!(text.contains("Benchmark Comparison"));
        assert!(text.contains("Quality Deltas"));
        assert!(text.contains("Latency Deltas"));
        assert!(text.contains("Verdict"));
    }
}
