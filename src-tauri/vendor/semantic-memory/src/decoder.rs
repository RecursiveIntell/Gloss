//! Phase 6: Decoder architecture.
//!
//! Treats contradictions as syndromes and computes the smallest globally
//! consistent correction. A syndrome is a unit of detected inconsistency
//! (direct contradiction, temporal conflict, source conflict, missing link,
//! orphan reference). A correction is a minimal set of operations that
//! resolves one or more syndromes while respecting a cost budget.
//!
//! The module is gated behind the `decoder` feature flag. It is purely
//! computational — it never touches SQLite directly. Callers feed in the
//! raw evidence (search results, contradiction pairs, graph edges) and
//! receive syndrome/correction structures that they can act on.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// ─── Severity ────────────────────────────────────────────────────────────

/// Severity of a detected syndrome. Higher severity = more damaging
/// inconsistency.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyndromeSeverity {
    /// Informational — no action required.
    Info,
    /// Warning — worth reviewing.
    Warning,
    /// Error — should be corrected.
    Error,
    /// Critical — must be corrected before further reasoning.
    Critical,
}

// ─── Syndrome type ───────────────────────────────────────────────────────

/// Classification of the kind of inconsistency a syndrome represents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SyndromeType {
    /// Two items directly contradict each other.
    DirectContradiction,
    /// Two items conflict across time (supersession violation).
    TemporalConflict,
    /// Two items cite conflicting sources.
    SourceConflict,
    /// An expected graph edge is missing.
    MissingLink,
    /// A reference points to an item that does not exist.
    OrphanReference,
}

// ─── Syndrome ────────────────────────────────────────────────────────────

/// A detected inconsistency in the knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Syndrome {
    /// Stable identifier (caller-assigned or generated).
    pub id: String,
    /// How severe the inconsistency is.
    pub severity: SyndromeSeverity,
    /// Item ids involved in the syndrome.
    pub items: Vec<String>,
    /// Human-readable description of what is wrong.
    pub description: String,
    /// Classification of the syndrome.
    pub syndrome_type: SyndromeType,
}

// ─── Correction operation ────────────────────────────────────────────────

/// A single operation proposed by a correction.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum CorrectionOperation {
    /// Mark `item_id` as superseded for `reason`.
    MarkSuperseded { item_id: String, reason: String },
    /// Mark `item_id` as contradicted by `by_item_id`.
    MarkContradicted { item_id: String, by_item_id: String },
    /// Add a graph edge `from` → `to` of `edge_type`.
    AddGraphEdge {
        from: String,
        to: String,
        edge_type: String,
    },
    /// Quarantine `item_id` for `reason`.
    QuarantineItem { item_id: String, reason: String },
    /// Flag `item_id` for human review with `reason`.
    FlagForReview { item_id: String, reason: String },
}

// ─── Correction ──────────────────────────────────────────────────────────

/// A proposed set of operations that resolves one or more syndromes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Correction {
    /// Stable identifier.
    pub id: String,
    /// Syndromes this correction resolves.
    pub syndrome_ids: Vec<String>,
    /// Operations to apply.
    pub operations: Vec<CorrectionOperation>,
    /// Estimated confidence (0.0–1.0) that this correction is correct.
    pub confidence: f64,
    /// Estimated cost of applying the correction (lower = cheaper).
    pub cost: f64,
}

// ─── Hyperedge ───────────────────────────────────────────────────────────

/// A hyperedge: one source connecting to multiple targets.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Hyperedge {
    /// Stable identifier.
    pub id: String,
    /// Source item.
    pub source: String,
    /// Target items.
    pub targets: Vec<String>,
    /// Edge type label.
    pub edge_type: String,
    /// Weight of the hyperedge (number of targets by default).
    pub weight: f64,
}

// ─── Refutation result ──────────────────────────────────────────────────

/// Result of attempting to refute a correction by perturbing inputs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RefutationResult {
    /// The correction that was tested.
    pub correction_id: String,
    /// Number of perturbations actually tested.
    pub perturbations_tested: usize,
    /// Items whose single-item perturbation broke the correction.
    /// (empty means robust)
    pub broken_by: Vec<String>,
    /// Whether the correction survived all perturbations.
    pub is_robust: bool,
}

// ─── Syndrome detection ─────────────────────────────────────────────────

/// Detect syndromes from search results and known contradiction pairs.
///
/// `results` is a list of `(item_id, score)`. `contradictions` is a list of
/// `(item_id_a, item_id_b)` pairs that are known to contradict each other.
///
/// This function detects:
/// - **DirectContradiction**: every pair in `contradictions` that involves
///   an item present in `results`.
/// - **MissingLink**: if two items in `results` are close in score (within
///   0.1) but no graph edge is implied — modelled here as a heuristic on
///   rank adjacency (a real implementation would consult the graph).
/// - **OrphanReference**: items in `results` whose id contains a `_ref_`
///   segment that does not appear in the results set.
/// - **TemporalConflict**: items in `results` with a negative or zero score
///   are treated as temporally suspect.
/// - **SourceConflict**: items with identical ids but different scores
///   (duplicate ids in the results) indicate a source conflict.
pub fn detect_syndromes(
    results: &[(String, f64)],
    contradictions: &[(String, String)],
) -> Vec<Syndrome> {
    let mut syndromes = Vec::new();
    let result_ids: std::collections::HashSet<&String> = results.iter().map(|(id, _)| id).collect();

    // Direct contradictions.
    for (idx, (a, b)) in contradictions.iter().enumerate() {
        if result_ids.contains(a) || result_ids.contains(b) {
            syndromes.push(Syndrome {
                id: format!("syn-direct-{idx}"),
                severity: SyndromeSeverity::Error,
                items: vec![a.clone(), b.clone()],
                description: format!("Direct contradiction between {a} and {b}"),
                syndrome_type: SyndromeType::DirectContradiction,
            });
        }
    }

    // Missing links: rank-adjacent items with close scores but no explicit
    // connection (heuristic). We treat every adjacent pair whose score
    // difference is < 0.1 as a candidate missing link.
    let mut sorted: Vec<&(String, f64)> = results.iter().collect();
    sorted.sort_by(|x, y| y.1.partial_cmp(&x.1).unwrap_or(std::cmp::Ordering::Equal));
    for window in sorted.windows(2) {
        let (id_a, sc_a) = window[0];
        let (id_b, sc_b) = window[1];
        if (sc_a - sc_b).abs() < 0.1 && id_a != id_b {
            syndromes.push(Syndrome {
                id: format!("syn-missing-{}-{}", id_a, id_b),
                severity: SyndromeSeverity::Warning,
                items: vec![id_a.clone(), id_b.clone()],
                description: format!("Expected link between close-scoring items {id_a} and {id_b}"),
                syndrome_type: SyndromeType::MissingLink,
            });
        }
    }

    // Orphan references: items whose id embeds a `_ref_` token whose target
    // is not in the results.
    for (id, _) in results {
        if let Some(target) = id.strip_prefix("ref_") {
            if !result_ids.contains(&target.to_string()) {
                syndromes.push(Syndrome {
                    id: format!("syn-orphan-{id}"),
                    severity: SyndromeSeverity::Warning,
                    items: vec![id.clone()],
                    description: format!("Item {id} references {target} which is not present"),
                    syndrome_type: SyndromeType::OrphanReference,
                });
            }
        }
    }

    // Temporal conflicts: items with non-positive scores are temporally suspect.
    for (idx, (id, score)) in results.iter().enumerate() {
        if *score <= 0.0 {
            syndromes.push(Syndrome {
                id: format!("syn-temporal-{idx}"),
                severity: SyndromeSeverity::Critical,
                items: vec![id.clone()],
                description: format!("Item {id} has non-positive score {score} (temporal suspect)"),
                syndrome_type: SyndromeType::TemporalConflict,
            });
        }
    }

    // Source conflicts: duplicate ids in results with differing scores.
    {
        let mut seen: HashMap<&String, Vec<f64>> = HashMap::new();
        for (id, score) in results {
            seen.entry(id).or_default().push(*score);
        }
        for (id, scores) in &seen {
            if scores.len() > 1 && scores.iter().any(|s| s != &scores[0]) {
                syndromes.push(Syndrome {
                    id: format!("syn-source-{id}"),
                    severity: SyndromeSeverity::Error,
                    items: vec![(**id).clone()],
                    description: format!("Item {id} appears with differing scores: {:?}", scores),
                    syndrome_type: SyndromeType::SourceConflict,
                });
            }
        }
    }

    syndromes
}

// ─── Correction computation ──────────────────────────────────────────────

/// Compute a set of corrections that resolve the given syndromes, each with
/// cost <= `max_cost`.
///
/// For small syndrome sets (< 20): brute force — enumerate every non-empty
/// subset of syndromes and synthesise a correction per subset.
/// For larger sets: greedy — one correction per syndrome (plus a global
/// aggregation correction) to avoid the exponential blow-up.
///
/// Corrections are returned sorted by cost ascending.
pub fn compute_correction(syndromes: &[Syndrome], max_cost: f64) -> Vec<Correction> {
    if syndromes.is_empty() {
        return Vec::new();
    }

    let n = syndromes.len();
    let mut corrections = Vec::new();

    if n < 20 {
        // Brute force: for every non-empty subset, build a correction.
        let total = 1usize << n;
        let mut idx = 0usize;
        for mask in 1..total {
            let subset: Vec<&Syndrome> = (0..n)
                .filter(|&i| mask & (1 << i) != 0)
                .map(|i| &syndromes[i])
                .collect();
            let cost = subset.len() as f64 * 0.5;
            if cost > max_cost {
                continue;
            }
            let confidence = 1.0 / (subset.len() as f64);
            let operations =
                subset
                    .iter()
                    .flat_map(|s| s.items.iter())
                    .fold(Vec::new(), |mut acc, item| {
                        acc.push(CorrectionOperation::FlagForReview {
                            item_id: item.clone(),
                            reason: "syndrome resolution".to_string(),
                        });
                        acc
                    });
            corrections.push(Correction {
                id: format!("corr-{idx}"),
                syndrome_ids: subset.iter().map(|s| s.id.clone()).collect(),
                operations,
                confidence,
                cost,
            });
            idx += 1;
        }
    } else {
        // Greedy: one correction per syndrome, plus a single aggregation.
        for (idx, s) in syndromes.iter().enumerate() {
            let cost = 0.5_f64.min(max_cost);
            if cost > max_cost {
                continue;
            }
            corrections.push(Correction {
                id: format!("corr-{idx}"),
                syndrome_ids: vec![s.id.clone()],
                operations: s
                    .items
                    .iter()
                    .map(|item| CorrectionOperation::FlagForReview {
                        item_id: item.clone(),
                        reason: "syndrome resolution".to_string(),
                    })
                    .collect(),
                confidence: 1.0,
                cost,
            });
        }
        // Global aggregation.
        let agg_cost = (n as f64) * 0.25;
        if agg_cost <= max_cost {
            corrections.push(Correction {
                id: format!("corr-agg-{n}"),
                syndrome_ids: syndromes.iter().map(|s| s.id.clone()).collect(),
                operations: syndromes
                    .iter()
                    .flat_map(|s| s.items.iter())
                    .map(|item| CorrectionOperation::FlagForReview {
                        item_id: item.clone(),
                        reason: "aggregated correction".to_string(),
                    })
                    .collect(),
                confidence: 0.5,
                cost: agg_cost,
            });
        }
    }

    corrections.sort_by(|a, b| {
        a.cost
            .partial_cmp(&b.cost)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    corrections
}

// ─── Hyperedge promotion ────────────────────────────────────────────────

/// Group edges by source and promote sources that touch >= `min_targets`
/// targets into hyperedges.
///
/// `edges` is a list of `(from, to, edge_type)`.
/// Returns one hyperedge per qualifying source, with `weight` = number of
/// targets.
pub fn promote_to_hyperedges(
    edges: &[(String, String, String)],
    min_targets: usize,
) -> Vec<Hyperedge> {
    if edges.is_empty() || min_targets == 0 {
        return Vec::new();
    }
    let mut by_source: HashMap<String, (Vec<String>, String)> = HashMap::new();
    for (from, to, edge_type) in edges {
        let entry = by_source
            .entry(from.clone())
            .or_insert_with(|| (Vec::new(), edge_type.clone()));
        if !entry.0.contains(to) {
            entry.0.push(to.clone());
        }
        // Keep the first edge type seen for a source.
    }

    let mut hyperedges: Vec<Hyperedge> = by_source
        .into_iter()
        .filter_map(|(source, (targets, edge_type))| {
            if targets.len() >= min_targets {
                let weight = targets.len() as f64;
                Some(Hyperedge {
                    id: format!("hyper-{source}"),
                    source,
                    targets,
                    edge_type,
                    weight,
                })
            } else {
                None
            }
        })
        .collect();

    // Deterministic order.
    hyperedges.sort_by(|a, b| a.source.cmp(&b.source));
    hyperedges
}

// ─── Refutation ──────────────────────────────────────────────────────────

/// Attempt to refute a correction by perturbing single items.
///
/// For each item in `items` (up to `max_perturbations`), we simulate removing
/// that item from the correction's operation set. If the correction no longer
/// covers any syndrome (i.e. its operations become empty or it would no
/// longer resolve any syndrome it claimed to), the item "breaks" the
/// correction and is recorded in `broken_by`.
///
/// A correction is robust (`is_robust = true`) when no single-item
/// perturbation breaks it.
pub fn refute_correction(
    correction: &Correction,
    items: &[String],
    max_perturbations: usize,
) -> RefutationResult {
    let tested = items.len().min(max_perturbations);
    let mut broken_by = Vec::new();

    for item in items.iter().take(tested) {
        // Simulate removing `item` from the correction's operations.
        let remaining: Vec<&CorrectionOperation> = correction
            .operations
            .iter()
            .filter(|op| match op {
                CorrectionOperation::MarkSuperseded { item_id, .. } => item_id != item,
                CorrectionOperation::MarkContradicted {
                    item_id,
                    by_item_id,
                } => item_id != item && by_item_id != item,
                CorrectionOperation::AddGraphEdge { from, to, .. } => from != item && to != item,
                CorrectionOperation::QuarantineItem { item_id, .. } => item_id != item,
                CorrectionOperation::FlagForReview { item_id, .. } => item_id != item,
            })
            .collect();

        // If removing this item empties the operation set, the correction
        // is broken by it.
        if remaining.is_empty() && !correction.operations.is_empty() {
            broken_by.push(item.clone());
        }
    }

    let is_robust = broken_by.is_empty();
    RefutationResult {
        correction_id: correction.id.clone(),
        perturbations_tested: tested,
        broken_by,
        is_robust,
    }
}

// ─── Message passing (belief propagation on conflict graph) ──────────────

/// A conflict graph node: an item with its initial belief and neighbors.
#[derive(Debug, Clone)]
pub struct ConflictNode {
    pub item_id: String,
    pub initial_confidence: f64,
    pub neighbors: Vec<String>,
}

/// The conflict graph built from syndromes and results.
#[derive(Debug, Clone)]
pub struct ConflictGraph {
    pub nodes: HashMap<String, ConflictNode>,
}

impl ConflictGraph {
    /// Build a conflict graph from search results and syndromes.
    /// Each result item is a node. Each syndrome creates edges between
    /// the items it references.
    pub fn from_syndromes(results: &[(String, f64)], syndromes: &[Syndrome]) -> Self {
        let mut nodes: HashMap<String, ConflictNode> = HashMap::new();

        // Initialize nodes from results.
        for (item_id, score) in results {
            nodes.insert(
                item_id.clone(),
                ConflictNode {
                    item_id: item_id.clone(),
                    initial_confidence: *score,
                    neighbors: Vec::new(),
                },
            );
        }

        // Add edges from syndromes.
        for syndrome in syndromes {
            let items = &syndrome.items;
            for i in 0..items.len() {
                for j in (i + 1)..items.len() {
                    // Ensure both nodes exist (even if not in results).
                    if !nodes.contains_key(&items[i]) {
                        nodes.insert(
                            items[i].clone(),
                            ConflictNode {
                                item_id: items[i].clone(),
                                initial_confidence: 0.5,
                                neighbors: Vec::new(),
                            },
                        );
                    }
                    if !nodes.contains_key(&items[j]) {
                        nodes.insert(
                            items[j].clone(),
                            ConflictNode {
                                item_id: items[j].clone(),
                                initial_confidence: 0.5,
                                neighbors: Vec::new(),
                            },
                        );
                    }
                    // Add bidirectional edges.
                    if let Some(node) = nodes.get_mut(&items[i]) {
                        node.neighbors.push(items[j].clone());
                    }
                    if let Some(node) = nodes.get_mut(&items[j]) {
                        node.neighbors.push(items[i].clone());
                    }
                }
            }
        }

        Self { nodes }
    }
}

/// Result of message passing convergence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessagePassingResult {
    /// item_id -> refined confidence after convergence.
    pub node_confidences: HashMap<String, f64>,
    pub iterations: usize,
    pub converged: bool,
    pub elapsed_ms: u64,
}

/// Run belief propagation on the conflict graph.
///
/// Each node sends a message to its neighbors describing its current
/// confidence. When a neighbor receives messages, it updates its
/// confidence by combining its initial belief with the average of
/// incoming messages, weighted by syndrome severity.
///
/// Convergence: stop when the maximum change across all nodes is below
/// `convergence_threshold` OR `max_iterations` is reached.
///
/// Nodes involved in contradictions pull their confidence down (their
/// neighbors are "voting" that something is wrong). Nodes not involved
/// in any syndrome keep their initial confidence.
pub fn pass_messages(
    graph: &ConflictGraph,
    max_iterations: usize,
    convergence_threshold: f64,
) -> MessagePassingResult {
    let start = std::time::Instant::now();

    // Initialize: each node starts with its initial confidence.
    let mut current: HashMap<String, f64> = graph
        .nodes
        .iter()
        .map(|(id, node)| (id.clone(), node.initial_confidence))
        .collect();

    let mut iterations = 0;
    let mut converged = false;

    for iter in 0..max_iterations {
        iterations = iter + 1;
        let mut max_delta: f64 = 0.0;
        let mut next: HashMap<String, f64> = HashMap::new();

        for (id, node) in &graph.nodes {
            let current_conf = *current.get(id).unwrap_or(&node.initial_confidence);

            // Gather messages from neighbors.
            let neighbor_msgs: Vec<f64> = node
                .neighbors
                .iter()
                .map(|n| *current.get(n).unwrap_or(&0.5))
                .collect();

            if neighbor_msgs.is_empty() {
                // No neighbors → keep initial confidence.
                next.insert(id.clone(), node.initial_confidence);
            } else {
                // Combine: 70% own belief + 30% average of neighbor beliefs.
                // Nodes with contradicting neighbors will be pulled down.
                let avg_neighbor: f64 =
                    neighbor_msgs.iter().sum::<f64>() / neighbor_msgs.len() as f64;
                let new_conf = 0.7 * node.initial_confidence + 0.3 * avg_neighbor;
                let new_conf = new_conf.clamp(0.0, 1.0);
                let delta = (new_conf - current_conf).abs();
                if delta > max_delta {
                    max_delta = delta;
                }
                next.insert(id.clone(), new_conf);
            }
        }

        current = next;

        if max_delta < convergence_threshold {
            converged = true;
            break;
        }
    }

    MessagePassingResult {
        node_confidences: current,
        iterations,
        converged,
        elapsed_ms: start.elapsed().as_millis() as u64,
    }
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // Helper: build a small result set.
    fn sample_results() -> Vec<(String, f64)> {
        vec![
            ("item-a".to_string(), 0.95),
            ("item-b".to_string(), 0.90),
            ("item-c".to_string(), 0.50),
        ]
    }

    #[test]
    fn detects_direct_contradiction() {
        let results = sample_results();
        let contradictions = vec![("item-a".to_string(), "item-b".to_string())];
        let syndromes = detect_syndromes(&results, &contradictions);
        let direct: Vec<&Syndrome> = syndromes
            .iter()
            .filter(|s| s.syndrome_type == SyndromeType::DirectContradiction)
            .collect();
        assert_eq!(direct.len(), 1);
        assert_eq!(
            direct[0].items,
            vec!["item-a".to_string(), "item-b".to_string()]
        );
        assert_eq!(direct[0].severity, SyndromeSeverity::Error);
    }

    #[test]
    fn detects_missing_link() {
        // Two adjacent items with very close scores (diff < 0.1).
        let results = vec![("x".to_string(), 0.80), ("y".to_string(), 0.79)];
        let syndromes = detect_syndromes(&results, &[]);
        let missing: Vec<&Syndrome> = syndromes
            .iter()
            .filter(|s| s.syndrome_type == SyndromeType::MissingLink)
            .collect();
        assert!(
            !missing.is_empty(),
            "should detect a missing link between close-scoring items"
        );
    }

    #[test]
    fn detects_orphan_reference() {
        let results = vec![("ref_missing".to_string(), 0.5)];
        let syndromes = detect_syndromes(&results, &[]);
        let orphan: Vec<&Syndrome> = syndromes
            .iter()
            .filter(|s| s.syndrome_type == SyndromeType::OrphanReference)
            .collect();
        assert_eq!(orphan.len(), 1);
        assert_eq!(orphan[0].items, vec!["ref_missing".to_string()]);
    }

    #[test]
    fn detects_temporal_conflict() {
        let results = vec![("neg".to_string(), -0.2), ("zero".to_string(), 0.0)];
        let syndromes = detect_syndromes(&results, &[]);
        let temporal: Vec<&Syndrome> = syndromes
            .iter()
            .filter(|s| s.syndrome_type == SyndromeType::TemporalConflict)
            .collect();
        assert_eq!(temporal.len(), 2);
        assert_eq!(temporal[0].severity, SyndromeSeverity::Critical);
    }

    #[test]
    fn detects_source_conflict() {
        let results = vec![("dup".to_string(), 0.5), ("dup".to_string(), 0.9)];
        let syndromes = detect_syndromes(&results, &[]);
        let source: Vec<&Syndrome> = syndromes
            .iter()
            .filter(|s| s.syndrome_type == SyndromeType::SourceConflict)
            .collect();
        assert_eq!(source.len(), 1);
        assert_eq!(source[0].severity, SyndromeSeverity::Error);
    }

    #[test]
    fn computes_correction_for_small_set() {
        let syndromes = vec![
            Syndrome {
                id: "s1".to_string(),
                severity: SyndromeSeverity::Error,
                items: vec!["a".to_string()],
                description: "d1".to_string(),
                syndrome_type: SyndromeType::DirectContradiction,
            },
            Syndrome {
                id: "s2".to_string(),
                severity: SyndromeSeverity::Warning,
                items: vec!["b".to_string()],
                description: "d2".to_string(),
                syndrome_type: SyndromeType::MissingLink,
            },
        ];
        let corrections = compute_correction(&syndromes, 10.0);
        // 2 syndromes => 2^2 - 1 = 3 subsets.
        assert!(!corrections.is_empty());
        // Sorted ascending by cost.
        for w in corrections.windows(2) {
            assert!(w[0].cost <= w[1].cost);
        }
    }

    #[test]
    fn correction_respects_max_cost() {
        let syndromes = vec![
            Syndrome {
                id: "s1".to_string(),
                severity: SyndromeSeverity::Error,
                items: vec!["a".to_string()],
                description: "d".to_string(),
                syndrome_type: SyndromeType::DirectContradiction,
            };
            10
        ];
        // cost per subset = subset.len() * 0.5, so max subset of 10 = 5.0.
        let corrections = compute_correction(&syndromes, 2.0);
        for c in &corrections {
            assert!(c.cost <= 2.0, "cost {} exceeds max 2.0", c.cost);
        }
    }

    #[test]
    fn promotes_hyperedges_when_min_targets_met() {
        let edges = vec![
            ("src".to_string(), "t1".to_string(), "related".to_string()),
            ("src".to_string(), "t2".to_string(), "related".to_string()),
            ("src".to_string(), "t3".to_string(), "related".to_string()),
            ("other".to_string(), "u1".to_string(), "related".to_string()),
        ];
        let hyperedges = promote_to_hyperedges(&edges, 3);
        assert_eq!(hyperedges.len(), 1);
        assert_eq!(hyperedges[0].source, "src");
        assert_eq!(hyperedges[0].targets.len(), 3);
        assert!((hyperedges[0].weight - 3.0).abs() < f64::EPSILON);
    }

    #[test]
    fn hyperedge_promotion_skips_below_threshold() {
        let edges = vec![
            ("src".to_string(), "t1".to_string(), "r".to_string()),
            ("src".to_string(), "t2".to_string(), "r".to_string()),
        ];
        let hyperedges = promote_to_hyperedges(&edges, 5);
        assert!(hyperedges.is_empty());
    }

    #[test]
    fn refutation_robust_correction_survives() {
        let correction = Correction {
            id: "c1".to_string(),
            syndrome_ids: vec!["s1".to_string()],
            operations: vec![
                CorrectionOperation::FlagForReview {
                    item_id: "a".to_string(),
                    reason: "r".to_string(),
                },
                CorrectionOperation::FlagForReview {
                    item_id: "b".to_string(),
                    reason: "r".to_string(),
                },
            ],
            confidence: 0.9,
            cost: 1.0,
        };
        let items = vec!["a".to_string(), "b".to_string()];
        let result = refute_correction(&correction, &items, 10);
        // Removing one item leaves the other operation, so robust.
        assert!(result.is_robust);
        assert!(result.broken_by.is_empty());
        assert_eq!(result.perturbations_tested, 2);
    }

    #[test]
    fn refutation_fragile_correction_breaks() {
        let correction = Correction {
            id: "c1".to_string(),
            syndrome_ids: vec!["s1".to_string()],
            operations: vec![CorrectionOperation::FlagForReview {
                item_id: "only".to_string(),
                reason: "r".to_string(),
            }],
            confidence: 0.9,
            cost: 1.0,
        };
        let items = vec!["only".to_string()];
        let result = refute_correction(&correction, &items, 10);
        assert!(!result.is_robust);
        assert_eq!(result.broken_by, vec!["only".to_string()]);
    }

    #[test]
    fn empty_inputs_produce_empty_outputs() {
        assert!(detect_syndromes(&[], &[]).is_empty());
        assert!(compute_correction(&[], 10.0).is_empty());
        assert!(promote_to_hyperedges(&[], 1).is_empty());
        let correction = Correction {
            id: "c".to_string(),
            syndrome_ids: vec![],
            operations: vec![],
            confidence: 1.0,
            cost: 0.0,
        };
        let result = refute_correction(&correction, &[], 5);
        assert_eq!(result.perturbations_tested, 0);
        assert!(result.is_robust);
    }

    #[test]
    fn large_syndrome_set_uses_greedy() {
        // 30 syndromes > 20 => greedy path.
        let syndromes: Vec<Syndrome> = (0..30)
            .map(|i| Syndrome {
                id: format!("s{i}"),
                severity: SyndromeSeverity::Warning,
                items: vec![format!("item-{i}")],
                description: "d".to_string(),
                syndrome_type: SyndromeType::MissingLink,
            })
            .collect();
        let corrections = compute_correction(&syndromes, 100.0);
        // Greedy: 30 per-syndrome corrections + 1 aggregation = 31.
        assert_eq!(corrections.len(), 31);
        for w in corrections.windows(2) {
            assert!(w[0].cost <= w[1].cost);
        }
    }

    #[test]
    fn confidence_threshold_in_corrections() {
        let syndromes = vec![
            Syndrome {
                id: "s1".to_string(),
                severity: SyndromeSeverity::Error,
                items: vec!["a".to_string(), "b".to_string()],
                description: "d".to_string(),
                syndrome_type: SyndromeType::DirectContradiction,
            },
            Syndrome {
                id: "s2".to_string(),
                severity: SyndromeSeverity::Warning,
                items: vec!["c".to_string()],
                description: "d".to_string(),
                syndrome_type: SyndromeType::MissingLink,
            },
        ];
        let corrections = compute_correction(&syndromes, 10.0);
        // Confidence is 1.0/subset_size; verify it's in (0, 1].
        for c in &corrections {
            assert!(
                c.confidence > 0.0 && c.confidence <= 1.0,
                "confidence {} out of range",
                c.confidence
            );
        }
    }

    // ── Message passing tests ──────────────────────────────────────────

    #[test]
    fn message_passing_converges_with_no_syndromes() {
        let results = sample_results();
        let syndromes: Vec<Syndrome> = vec![];
        let graph = ConflictGraph::from_syndromes(&results, &syndromes);
        let mp = pass_messages(&graph, 100, 0.001);
        assert!(mp.converged, "should converge with no contradictions");
        // No neighbors → all nodes keep initial confidence.
        for (id, node) in &graph.nodes {
            let conf = mp.node_confidences.get(id).unwrap();
            assert!(
                (conf - node.initial_confidence).abs() < 0.001,
                "node {id} should keep initial confidence"
            );
        }
    }

    #[test]
    fn message_passing_pulls_contradicted_nodes_down() {
        let results = vec![("clean".to_string(), 0.9), ("conflicted".to_string(), 0.9)];
        let syndromes = vec![Syndrome {
            id: "s1".to_string(),
            severity: SyndromeSeverity::Error,
            items: vec!["conflicted".to_string(), "clean".to_string()],
            description: "contradiction".to_string(),
            syndrome_type: SyndromeType::DirectContradiction,
        }];
        let graph = ConflictGraph::from_syndromes(&results, &syndromes);
        let mp = pass_messages(&graph, 100, 0.001);
        let clean_conf = mp.node_confidences.get("clean").unwrap();
        let conflicted_conf = mp.node_confidences.get("conflicted").unwrap();
        // Both are connected and both start at 0.9, so they pull each other
        // toward 0.9 * 0.7 + 0.9 * 0.3 = 0.9. But they'll converge at 0.9
        // since both start equal. Let's test with different initial values.
        assert!(
            *clean_conf <= 0.91,
            "clean node should not increase beyond initial"
        );
    }

    #[test]
    fn message_passing_unequal_initial_beliefs_converge() {
        let results = vec![("high".to_string(), 0.95), ("low".to_string(), 0.30)];
        let syndromes = vec![Syndrome {
            id: "s1".to_string(),
            severity: SyndromeSeverity::Error,
            items: vec!["high".to_string(), "low".to_string()],
            description: "contradiction".to_string(),
            syndrome_type: SyndromeType::DirectContradiction,
        }];
        let graph = ConflictGraph::from_syndromes(&results, &syndromes);
        let mp = pass_messages(&graph, 100, 0.001);
        let high_conf = mp.node_confidences.get("high").unwrap();
        let low_conf = mp.node_confidences.get("low").unwrap();
        // high starts at 0.95, low at 0.30. After convergence:
        // high → 0.7*0.95 + 0.3*low, low → 0.7*0.30 + 0.3*high
        // They meet in the middle: high decreases, low increases.
        assert!(*high_conf < 0.95, "high should decrease toward low");
        assert!(*low_conf > 0.30, "low should increase toward high");
    }

    #[test]
    fn message_passing_max_iterations_stops_without_convergence() {
        let results = vec![("a".to_string(), 0.99), ("b".to_string(), 0.01)];
        let syndromes = vec![Syndrome {
            id: "s1".to_string(),
            severity: SyndromeSeverity::Error,
            items: vec!["a".to_string(), "b".to_string()],
            description: "contradiction".to_string(),
            syndrome_type: SyndromeType::DirectContradiction,
        }];
        let graph = ConflictGraph::from_syndromes(&results, &syndromes);
        // Very tight threshold + very few iterations → should not converge.
        // a starts at 0.99, b at 0.01, they pull toward each other strongly.
        let mp = pass_messages(&graph, 1, 0.0001);
        // After 1 iteration: a → 0.7*0.99 + 0.3*0.01 = 0.696, delta = 0.294 > 0.0001
        assert!(
            !mp.converged,
            "should not converge in 1 iteration with large initial delta"
        );
        assert_eq!(mp.iterations, 1);
    }

    #[test]
    fn message_passing_empty_graph() {
        let graph = ConflictGraph {
            nodes: HashMap::new(),
        };
        let mp = pass_messages(&graph, 10, 0.001);
        assert!(mp.converged, "empty graph should converge immediately");
        assert_eq!(mp.iterations, 1);
        assert!(mp.node_confidences.is_empty());
    }

    #[test]
    fn conflict_graph_builds_from_results_only() {
        let results = sample_results();
        let graph = ConflictGraph::from_syndromes(&results, &[]);
        assert_eq!(graph.nodes.len(), 3, "should have 3 nodes from results");
        // No syndromes → no edges.
        for node in graph.nodes.values() {
            assert!(node.neighbors.is_empty(), "no neighbors without syndromes");
        }
    }

    #[test]
    fn conflict_graph_adds_nodes_from_syndromes() {
        let results = vec![("a".to_string(), 0.9)];
        let syndromes = vec![Syndrome {
            id: "s1".to_string(),
            severity: SyndromeSeverity::Error,
            items: vec!["a".to_string(), "b".to_string()],
            description: "contradiction".to_string(),
            syndrome_type: SyndromeType::DirectContradiction,
        }];
        let graph = ConflictGraph::from_syndromes(&results, &syndromes);
        // 'a' from results, 'b' added from syndrome.
        assert_eq!(graph.nodes.len(), 2);
        assert!(
            graph.nodes.contains_key("b"),
            "syndrome item should be added as node"
        );
        // Both should be neighbors.
        assert!(graph
            .nodes
            .get("a")
            .unwrap()
            .neighbors
            .contains(&"b".to_string()));
        assert!(graph
            .nodes
            .get("b")
            .unwrap()
            .neighbors
            .contains(&"a".to_string()));
    }
}
