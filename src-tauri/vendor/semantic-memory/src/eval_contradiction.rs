//! Contradiction-detection evaluation harness (RAMDocs-style).
//!
//! The decoder ([`crate::decoder`]) turns candidate contradiction pairs into
//! structured syndromes and computes the minimal globally-consistent
//! correction. This module measures how well that detection matches ground
//! truth, so the decoder's value can be reported with standard precision /
//! recall / F1 the way RAG-conflict benchmarks (e.g. RAMDocs) expect.
//!
//! Design mirrors [`crate::benchmark`]: the scoring functions ([`prf`],
//! [`detected_conflict_pairs`]) are pure and deterministic, separate from any
//! live database, so they run in CI without embeddings or I/O. A case carries
//! its own `candidate_contradictions` (the pairs fed to the decoder — in
//! production these come from upstream detection; in a fixture they are given
//! so the decoder's structuring/resolution can be scored in isolation) and the
//! `expected_conflicts` ground truth.
//!
//! Behind `#[cfg(feature = "decoder")]`.

use crate::decoder::{detect_syndromes, Syndrome, SyndromeType};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

// ─── Case model ────────────────────────────────────────────────────────────

/// One retrieved item in a contradiction eval case.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalItem {
    /// Stable id (e.g. `fact:<uuid>`) used in syndrome/contradiction pairs.
    pub id: String,
    /// The item's text. Carried for fixtures/readability; the pure scorer does
    /// not need it, but a future store-coupled detector will.
    #[serde(default)]
    pub content: String,
}

/// A graph edge in scope for a case. Mirrors the fields of
/// [`crate::graph_edges::StoredGraphEdge`] the store-coupled path reads; in
/// production these are loaded from the live store for the items in scope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalEdge {
    /// Source item id.
    pub source: String,
    /// Target item id.
    pub target: String,
    /// Edge type string (e.g. `"contradicts"`, `"relates_to"`).
    pub edge_type: String,
}

/// A single contradiction-detection test case (RAMDocs-style): a query, the
/// retrieved items, the candidate contradiction pairs handed to the decoder,
/// and the ground-truth conflicting pairs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionCase {
    /// Stable case identifier.
    pub id: String,
    /// The query that produced these items (informational).
    #[serde(default)]
    pub query: String,
    /// Retrieved items in scope for this case.
    pub items: Vec<EvalItem>,
    /// Candidate contradiction pairs fed to the decoder. Mirrors the
    /// `contradictions` argument of [`detect_syndromes`]. Used by [`run_case`]
    /// to score the decoder in isolation.
    #[serde(default)]
    pub candidate_contradictions: Vec<(String, String)>,
    /// Graph edges in scope. Used by [`run_case_from_edges`] to derive
    /// candidates the way the live system does — from stored contradiction
    /// edges — rather than from pre-supplied pairs.
    #[serde(default)]
    pub graph_edges: Vec<EvalEdge>,
    /// Ground-truth conflicting pairs (order within a pair is ignored).
    pub expected_conflicts: Vec<(String, String)>,
}

// ─── Metrics ────────────────────────────────────────────────────────────────

/// Precision / recall / F1 for contradiction detection over a set of pairs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ContradictionMetrics {
    /// Correctly detected conflict pairs.
    pub true_positives: usize,
    /// Detected pairs that are not real conflicts.
    pub false_positives: usize,
    /// Real conflicts that were missed.
    pub false_negatives: usize,
    /// `tp / (tp + fp)` — 1.0 when no false positives (and any detection).
    pub precision: f64,
    /// `tp / (tp + fn)` — 1.0 when nothing was missed.
    pub recall: f64,
    /// Harmonic mean of precision and recall.
    pub f1: f64,
}

impl ContradictionMetrics {
    /// Build metrics from raw counts, deriving precision/recall/F1. The
    /// degenerate "nothing expected, nothing detected" case scores a perfect
    /// 1.0 (the system correctly found no conflicts).
    pub fn from_counts(tp: usize, fp: usize, fn_: usize) -> Self {
        let precision = if tp + fp == 0 {
            if fn_ == 0 {
                1.0
            } else {
                0.0
            }
        } else {
            tp as f64 / (tp + fp) as f64
        };
        let recall = if tp + fn_ == 0 {
            1.0
        } else {
            tp as f64 / (tp + fn_) as f64
        };
        let f1 = if precision + recall == 0.0 {
            0.0
        } else {
            2.0 * precision * recall / (precision + recall)
        };
        Self {
            true_positives: tp,
            false_positives: fp,
            false_negatives: fn_,
            precision,
            recall,
            f1,
        }
    }
}

/// Normalize a pair so `(a,b)` and `(b,a)` compare equal.
fn norm_pair(a: &str, b: &str) -> (String, String) {
    if a <= b {
        (a.to_string(), b.to_string())
    } else {
        (b.to_string(), a.to_string())
    }
}

/// Score detected conflict pairs against the expected (ground-truth) pairs.
/// Both sets are compared as unordered pairs.
pub fn prf(
    detected: &HashSet<(String, String)>,
    expected: &HashSet<(String, String)>,
) -> ContradictionMetrics {
    let tp = detected.intersection(expected).count();
    let fp = detected.len() - tp;
    let fn_ = expected.len() - tp;
    ContradictionMetrics::from_counts(tp, fp, fn_)
}

/// Which syndrome types count as a detected contradiction for scoring.
fn is_conflict(t: SyndromeType) -> bool {
    matches!(
        t,
        SyndromeType::DirectContradiction
            | SyndromeType::TemporalConflict
            | SyndromeType::SourceConflict
    )
}

/// Extract the unordered conflict pairs implied by a set of syndromes. Every
/// distinct pair of items within a conflict-typed syndrome is one detected
/// conflict (a 2-item syndrome yields exactly one pair).
pub fn detected_conflict_pairs(syndromes: &[Syndrome]) -> HashSet<(String, String)> {
    let mut pairs = HashSet::new();
    for s in syndromes {
        if !is_conflict(s.syndrome_type) {
            continue;
        }
        for i in 0..s.items.len() {
            for j in (i + 1)..s.items.len() {
                pairs.insert(norm_pair(&s.items[i], &s.items[j]));
            }
        }
    }
    pairs
}

// ─── Store-coupled candidate derivation ──────────────────────────────────────

/// Edge types the system uses to record a contradiction. Matches the predicate
/// in [`crate::temporal`] that counts contradiction edges.
pub const CONTRADICTION_EDGE_TYPES: [&str; 2] = ["contradicts", "contradicted_by"];

/// Whether an edge type records a contradiction.
pub fn is_contradiction_edge(edge_type: &str) -> bool {
    CONTRADICTION_EDGE_TYPES.contains(&edge_type)
}

/// Derive candidate contradiction pairs the way the live system does: from
/// stored contradiction edges whose *both* endpoints are in scope (the
/// retrieved item set). This is the end-to-end detection signal — unlike
/// [`run_case`]'s pre-supplied pairs, it measures whether the stored graph
/// actually surfaces the conflict among the retrieved items.
pub fn derive_candidates(scope: &HashSet<String>, edges: &[EvalEdge]) -> Vec<(String, String)> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for e in edges {
        if !is_contradiction_edge(&e.edge_type) {
            continue;
        }
        if !scope.contains(&e.source) || !scope.contains(&e.target) {
            continue;
        }
        // De-dupe symmetric edges (contradicts / contradicted_by both ways).
        if seen.insert(norm_pair(&e.source, &e.target)) {
            out.push((e.source.clone(), e.target.clone()));
        }
    }
    out
}

// ─── Runner (deterministic, store-free) ─────────────────────────────────────

/// Run the decoder over one case and score its detected conflicts against
/// ground truth. Deterministic and offline: items become `(id, 1.0)` search
/// results, the case's `candidate_contradictions` are handed to the decoder,
/// and the resulting syndromes are scored.
pub fn run_case(case: &ContradictionCase) -> ContradictionMetrics {
    let results: Vec<(String, f64)> = case.items.iter().map(|i| (i.id.clone(), 1.0)).collect();
    let syndromes = detect_syndromes(&results, &case.candidate_contradictions);
    let detected = detected_conflict_pairs(&syndromes);
    let expected: HashSet<(String, String)> = case
        .expected_conflicts
        .iter()
        .map(|(a, b)| norm_pair(a, b))
        .collect();
    prf(&detected, &expected)
}

/// Run the *end-to-end* path: derive candidate contradictions from the case's
/// graph edges (the system's stored-contradiction mechanism), run the decoder,
/// and score against ground truth. Measures detection + resolution together,
/// where [`run_case`] measures resolution alone. Deterministic and offline:
/// `graph_edges` stand in for what the live store would return.
pub fn run_case_from_edges(case: &ContradictionCase) -> ContradictionMetrics {
    let scope: HashSet<String> = case.items.iter().map(|i| i.id.clone()).collect();
    let candidates = derive_candidates(&scope, &case.graph_edges);
    let results: Vec<(String, f64)> = case.items.iter().map(|i| (i.id.clone(), 1.0)).collect();
    let syndromes = detect_syndromes(&results, &candidates);
    let detected = detected_conflict_pairs(&syndromes);
    let expected: HashSet<(String, String)> = case
        .expected_conflicts
        .iter()
        .map(|(a, b)| norm_pair(a, b))
        .collect();
    prf(&detected, &expected)
}

/// Run the *content-based* path: propose contradictions from item text with
/// [`crate::contradiction_detect`] (no asserted edges, no decoder) and score
/// the detector's proposals against ground truth. This is how the lexical
/// detector is measured — the harness's reason for existing.
pub fn run_case_from_content(
    case: &ContradictionCase,
    cfg: &crate::contradiction_detect::DetectorConfig,
) -> ContradictionMetrics {
    let items: Vec<(String, String)> = case
        .items
        .iter()
        .map(|i| (i.id.clone(), i.content.clone()))
        .collect();
    let detected: HashSet<(String, String)> =
        crate::contradiction_detect::detect_contradictions(&items, cfg)
            .iter()
            .map(|p| norm_pair(&p.a, &p.b))
            .collect();
    let expected: HashSet<(String, String)> = case
        .expected_conflicts
        .iter()
        .map(|(a, b)| norm_pair(a, b))
        .collect();
    prf(&detected, &expected)
}

/// Aggregate report over a suite of cases.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContradictionReport {
    /// Per-case `(case_id, metrics)`.
    pub per_case: Vec<(String, ContradictionMetrics)>,
    /// Micro-averaged metrics (pool all pairs across cases, then score once).
    pub micro: ContradictionMetrics,
    /// Macro-averaged F1 (mean of per-case F1).
    pub macro_f1: f64,
}

/// Run a suite of cases and produce both micro- and macro-averaged metrics.
pub fn run_suite(cases: &[ContradictionCase]) -> ContradictionReport {
    let mut per_case = Vec::with_capacity(cases.len());
    let (mut tp, mut fp, mut fn_) = (0usize, 0usize, 0usize);
    let mut f1_sum = 0.0;
    for case in cases {
        let m = run_case(case);
        tp += m.true_positives;
        fp += m.false_positives;
        fn_ += m.false_negatives;
        f1_sum += m.f1;
        per_case.push((case.id.clone(), m));
    }
    let macro_f1 = if cases.is_empty() {
        0.0
    } else {
        f1_sum / cases.len() as f64
    };
    ContradictionReport {
        micro: ContradictionMetrics::from_counts(tp, fp, fn_),
        macro_f1,
        per_case,
    }
}

// ─── Fixtures + (de)serialization ────────────────────────────────────────────

/// Built-in RAMDocs-style cases: a clean case (no conflict), a single-conflict
/// case, and a noisy case where only one of several candidate pairs is a real
/// conflict (tests that the decoder does not over-flag).
pub fn builtin_cases() -> Vec<ContradictionCase> {
    let item = |id: &str, c: &str| EvalItem {
        id: id.to_string(),
        content: c.to_string(),
    };
    let edge = |s: &str, t: &str, et: &str| EvalEdge {
        source: s.to_string(),
        target: t.to_string(),
        edge_type: et.to_string(),
    };
    vec![
        ContradictionCase {
            id: "clean-no-conflict".to_string(),
            query: "what port does the warm server use".to_string(),
            items: vec![
                item(
                    "fact:a1",
                    "The warm server listens on port 1739 by default.",
                ),
                item(
                    "fact:a2",
                    "The warm HTTP server is co-hosted by the MCP server.",
                ),
            ],
            candidate_contradictions: vec![],
            // Only a benign relation edge — no contradiction edge to derive.
            graph_edges: vec![edge("fact:a1", "fact:a2", "relates_to")],
            expected_conflicts: vec![],
        },
        ContradictionCase {
            id: "single-direct-conflict".to_string(),
            query: "how many tools does the server expose".to_string(),
            items: vec![
                item("fact:b1", "The MCP server exposes 33 tools."),
                item("fact:b2", "The MCP server exposes 12 tools."),
                item("fact:b3", "The server is built with the rmcp Rust SDK."),
            ],
            candidate_contradictions: vec![("fact:b1".to_string(), "fact:b2".to_string())],
            graph_edges: vec![
                edge("fact:b1", "fact:b2", "contradicts"),
                edge("fact:b1", "fact:b3", "relates_to"), // distractor
            ],
            expected_conflicts: vec![("fact:b1".to_string(), "fact:b2".to_string())],
        },
        ContradictionCase {
            id: "noisy-one-real-conflict".to_string(),
            query: "which embedder is the default".to_string(),
            items: vec![
                item(
                    "fact:c1",
                    "The default embedder is Candle (in-process, CPU).",
                ),
                item("fact:c2", "The default embedder is Ollama."),
                item("fact:c3", "Candle downloads nomic-embed-text on first use."),
            ],
            // Two candidates offered; only c1/c2 is a real conflict. c1/c3 are
            // compatible (a real detector should not pair them).
            candidate_contradictions: vec![
                ("fact:c1".to_string(), "fact:c2".to_string()),
                ("fact:c1".to_string(), "fact:c3".to_string()),
            ],
            // The edge path is sharper than the candidate path here: c1/c3 is
            // recorded as a benign relation, so derive_candidates excludes it
            // and only the real c1/c2 contradiction is scored.
            graph_edges: vec![
                edge("fact:c1", "fact:c2", "contradicts"),
                edge("fact:c1", "fact:c3", "relates_to"),
            ],
            expected_conflicts: vec![("fact:c1".to_string(), "fact:c2".to_string())],
        },
    ]
}

/// Serialize cases to JSONL (one case per line) for fixture storage / replay.
pub fn cases_to_jsonl(cases: &[ContradictionCase]) -> String {
    cases
        .iter()
        .map(|c| serde_json::to_string(c).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("\n")
}

/// Parse cases from JSONL (blank lines ignored).
pub fn cases_from_jsonl(jsonl: &str) -> Result<Vec<ContradictionCase>, serde_json::Error> {
    jsonl
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(serde_json::from_str)
        .collect()
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn set(pairs: &[(&str, &str)]) -> HashSet<(String, String)> {
        pairs.iter().map(|(a, b)| norm_pair(a, b)).collect()
    }

    #[test]
    fn prf_perfect_match() {
        let d = set(&[("a", "b"), ("c", "d")]);
        let e = set(&[("b", "a"), ("d", "c")]); // reversed order, same pairs
        let m = prf(&d, &e);
        assert_eq!(m.true_positives, 2);
        assert_eq!(m.false_positives, 0);
        assert_eq!(m.false_negatives, 0);
        assert!((m.f1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn prf_partial() {
        let d = set(&[("a", "b"), ("x", "y")]); // one right, one wrong
        let e = set(&[("a", "b"), ("c", "d")]); // one hit, one missed
        let m = prf(&d, &e);
        assert_eq!(m.true_positives, 1);
        assert_eq!(m.false_positives, 1);
        assert_eq!(m.false_negatives, 1);
        assert!((m.precision - 0.5).abs() < 1e-9);
        assert!((m.recall - 0.5).abs() < 1e-9);
        assert!((m.f1 - 0.5).abs() < 1e-9);
    }

    #[test]
    fn prf_empty_is_perfect() {
        let m = prf(&HashSet::new(), &HashSet::new());
        assert!((m.f1 - 1.0).abs() < 1e-9);
        assert!((m.precision - 1.0).abs() < 1e-9);
        assert!((m.recall - 1.0).abs() < 1e-9);
    }

    #[test]
    fn clean_case_scores_perfect() {
        let cases = builtin_cases();
        let clean = cases.iter().find(|c| c.id == "clean-no-conflict").unwrap();
        let m = run_case(clean);
        // No candidates, no expected → perfect, no false positives.
        assert_eq!(m.false_positives, 0);
        assert!((m.f1 - 1.0).abs() < 1e-9);
    }

    #[test]
    fn single_conflict_detected() {
        let cases = builtin_cases();
        let c = cases
            .iter()
            .find(|c| c.id == "single-direct-conflict")
            .unwrap();
        let m = run_case(c);
        assert_eq!(m.true_positives, 1);
        assert_eq!(m.false_negatives, 0);
    }

    #[test]
    fn suite_aggregates() {
        let report = run_suite(&builtin_cases());
        assert_eq!(report.per_case.len(), 3);
        // Micro recall: every real conflict in the suite should be found,
        // since each real conflict is offered as a candidate.
        assert_eq!(report.micro.false_negatives, 0);
        assert!(report.macro_f1 > 0.0);
    }

    #[test]
    fn jsonl_roundtrip() {
        let cases = builtin_cases();
        let jsonl = cases_to_jsonl(&cases);
        let back = cases_from_jsonl(&jsonl).unwrap();
        assert_eq!(back.len(), cases.len());
        assert_eq!(back[1].id, cases[1].id);
        assert_eq!(back[1].expected_conflicts, cases[1].expected_conflicts);
    }

    #[test]
    fn derive_candidates_filters_type_and_scope() {
        let scope: HashSet<String> = ["x", "y", "z"].iter().map(|s| s.to_string()).collect();
        let mk = |s: &str, t: &str, et: &str| EvalEdge {
            source: s.to_string(),
            target: t.to_string(),
            edge_type: et.to_string(),
        };
        let edges = vec![
            mk("x", "y", "contradicts"),     // kept
            mk("x", "z", "relates_to"),      // wrong type → dropped
            mk("x", "w", "contradicts"),     // w out of scope → dropped
            mk("y", "x", "contradicted_by"), // symmetric dup of x/y → dropped
        ];
        let cands = derive_candidates(&scope, &edges);
        assert_eq!(cands.len(), 1, "only the in-scope contradiction, de-duped");
        assert_eq!(norm_pair(&cands[0].0, &cands[0].1), norm_pair("x", "y"));
    }

    #[test]
    fn edge_path_scores_single_conflict() {
        let cases = builtin_cases();
        let c = cases
            .iter()
            .find(|c| c.id == "single-direct-conflict")
            .unwrap();
        let m = run_case_from_edges(c);
        assert_eq!(m.true_positives, 1);
        assert_eq!(
            m.false_positives, 0,
            "the relates_to distractor must not count"
        );
        assert_eq!(m.false_negatives, 0);
    }

    #[test]
    fn content_detector_scores_suite() {
        use crate::contradiction_detect::DetectorConfig;
        let cfg = DetectorConfig::default();
        let cases = builtin_cases();
        let (mut tp, mut fp, mut fn_) = (0usize, 0usize, 0usize);
        for c in &cases {
            let m = run_case_from_content(c, &cfg);
            tp += m.true_positives;
            fp += m.false_positives;
            fn_ += m.false_negatives;
        }
        // Numeric (b1/b2) and value (c1/c2) conflicts found; no false positives
        // on the clean case or the benign Candle/distractor pairs.
        assert_eq!(tp, 2, "both real conflicts detected from content");
        assert_eq!(fp, 0, "no benign pair flagged");
        assert_eq!(fn_, 0, "nothing missed");
    }

    #[test]
    fn edge_path_is_cleaner_than_candidate_path_on_noisy_case() {
        let cases = builtin_cases();
        let c = cases
            .iter()
            .find(|c| c.id == "noisy-one-real-conflict")
            .unwrap();
        // The candidate path is fed c1/c3 and would over-flag if the decoder
        // structured it; the edge path derives only the real c1/c2 contradiction.
        let edge_m = run_case_from_edges(c);
        assert_eq!(
            edge_m.false_positives, 0,
            "edge predicate drops the benign c1/c3"
        );
        assert!((edge_m.f1 - 1.0).abs() < 1e-9);
    }
}
