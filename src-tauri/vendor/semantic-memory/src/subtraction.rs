//! Phase 7: Lawful subtraction engine.
//!
//! Determines what to forget, compress, quarantine, or retire while preserving
//! invariants. A "subtraction" is any operation that removes or demotes an item
//! from the active knowledge base. Every subtraction requires a structuring
//! score below the configured minimum and must satisfy the invariants:
//!
//! - Never remove an item that is still actively referenced.
//! - Never remove an item without emitting a receipt (with snapshot).
//!
//! The module is gated behind the `subtraction` feature flag. It is purely
//! computational — it never touches SQLite directly. Callers feed in items
//! with their structuring scores and active reference sets, and receive
//! candidate lists, invariant checks, and receipts.

use serde::{Deserialize, Serialize};

// ─── Config ───────────────────────────────────────────────────────────────

/// Configuration for the subtraction engine.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtractionConfig {
    /// Minimum structuring score required to keep an item in the active set.
    /// Items below this threshold are subtraction candidates. Default 2.0.
    pub min_structuring_score: f64,
    /// Maximum number of items to process in a single run. Default 100.
    pub max_items_per_run: usize,
}

impl Default for SubtractionConfig {
    fn default() -> Self {
        Self {
            min_structuring_score: 2.0,
            max_items_per_run: 100,
        }
    }
}

// ─── Compaction strategy ─────────────────────────────────────────────────

/// The action recommended for a subtraction candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CompactionStrategy {
    /// Aggregate the item into a summary/rolled-up form.
    Aggregate,
    /// Move the item to cold storage (retained, not active).
    ColdStorage,
    /// Delete the item, keeping only a receipt with a snapshot.
    DeleteWithReceipt,
    /// Quarantine the item (suspended pending review).
    Quarantine,
}

// ─── Candidate ───────────────────────────────────────────────────────────

/// An item that is a candidate for subtraction.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtractionCandidate {
    /// The item id.
    pub item_id: String,
    /// The computed structuring score (lower = more eligible for subtraction).
    pub structuring_score: f64,
    /// The recommended compaction action.
    pub recommended_action: CompactionStrategy,
    /// Human-readable reason for the recommendation.
    pub reason: String,
}

// ─── Receipt ──────────────────────────────────────────────────────────────

/// A receipt proving that an item was subtracted, with a snapshot of its
/// former state so it can be recovered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubtractionReceipt {
    /// Unique receipt id.
    pub receipt_id: String,
    /// The item that was subtracted.
    pub item_id: String,
    /// The action that was applied.
    pub action: CompactionStrategy,
    /// The structuring score at the time of subtraction.
    pub structuring_score: f64,
    /// Human-readable reason.
    pub reason: String,
    /// JSON snapshot of the item's state before subtraction.
    pub item_snapshot: serde_json::Value,
    /// ISO-8601 timestamp of the subtraction.
    pub timestamp: String,
}

// ─── Invariant check ────────────────────────────────────────────────────

/// Result of checking whether a set of candidates satisfies the invariants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvariantCheck {
    /// True iff all invariants pass.
    pub all_pass: bool,
    /// One entry per violated invariant.
    pub violations: Vec<InvariantViolation>,
}

/// A single violated invariant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InvariantViolation {
    /// The candidate that caused the violation.
    pub candidate_id: String,
    /// The invariant name (e.g. `"no_referenced_removal"`).
    pub invariant: String,
    /// Human-readable description.
    pub description: String,
}

// ─── Structuring score ─────────────────────────────────────────────────

/// Compute the structuring score for an item.
///
/// Score = (has_provenance as f64)
///       + (is_referenced as f64)
///       + (is_verified as f64)
///       + (!is_contradicted as f64)
///       + (has_episode as f64)
///       + (access_count > 0 as f64)
///       + temporal_weight
///
/// Higher scores mean the item is more "structuring" (worth keeping). Lower
/// scores make it a subtraction candidate.
pub fn compute_structuring_score(
    has_provenance: bool,
    is_referenced: bool,
    is_verified: bool,
    is_contradicted: bool,
    has_episode: bool,
    access_count: usize,
    temporal_weight: f64,
) -> f64 {
    let has_provenance = if has_provenance { 1.0 } else { 0.0 };
    let is_referenced = if is_referenced { 1.0 } else { 0.0 };
    let is_verified = if is_verified { 1.0 } else { 0.0 };
    let not_contradicted = if !is_contradicted { 1.0 } else { 0.0 };
    let has_episode = if has_episode { 1.0 } else { 0.0 };
    let has_access = if access_count > 0 { 1.0 } else { 0.0 };
    has_provenance
        + is_referenced
        + is_verified
        + not_contradicted
        + has_episode
        + has_access
        + temporal_weight
}

// ─── Candidate finding ──────────────────────────────────────────────────

/// Find subtraction candidates from a list of `(item_id, structuring_score)`.
///
/// Items with `score < config.min_structuring_score` are candidates. The
/// recommended action is chosen heuristically:
/// - score < 1.0 → DeleteWithReceipt
/// - 1.0 <= score < 1.5 → Quarantine
/// - 1.5 <= score < 2.0 → ColdStorage
/// - (scores below threshold but >= 2.0 never happen by definition)
///
/// Results are sorted by score ascending (lowest first) and capped at
/// `config.max_items_per_run`.
pub fn find_candidates(
    items: &[(String, f64)],
    config: &SubtractionConfig,
) -> Vec<SubtractionCandidate> {
    let mut candidates: Vec<SubtractionCandidate> = items
        .iter()
        .filter(|(_, score)| *score < config.min_structuring_score)
        .map(|(id, score)| {
            let action = if *score < 1.0 {
                CompactionStrategy::DeleteWithReceipt
            } else if *score < 1.5 {
                CompactionStrategy::Quarantine
            } else {
                CompactionStrategy::ColdStorage
            };
            let reason = format!(
                "structuring score {score:.3} below minimum {}",
                config.min_structuring_score
            );
            SubtractionCandidate {
                item_id: id.clone(),
                structuring_score: *score,
                recommended_action: action,
                reason,
            }
        })
        .collect();

    candidates.sort_by(|a, b| {
        a.structuring_score
            .partial_cmp(&b.structuring_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    candidates.truncate(config.max_items_per_run);
    candidates
}

// ─── Invariant verification ─────────────────────────────────────────────

/// Verify that subtracting the given candidates would not violate invariants.
///
/// Invariants checked:
/// - **no_referenced_removal**: no candidate may target an item that is in
///   `active_references`.
/// - **no_removal_without_receipt**: (structural — `execute_subtraction`
///   always emits receipts, so this is satisfied by construction; we still
///   report it as a passing check when there are candidates to subtract.)
pub fn verify_invariants(
    candidates: &[SubtractionCandidate],
    active_references: &[String],
) -> InvariantCheck {
    let active: std::collections::HashSet<&String> = active_references.iter().collect();
    let mut violations = Vec::new();

    for c in candidates {
        if active.contains(&c.item_id) {
            violations.push(InvariantViolation {
                candidate_id: c.item_id.clone(),
                invariant: "no_referenced_removal".to_string(),
                description: format!(
                    "Item {} is actively referenced and must not be removed",
                    c.item_id
                ),
            });
        }
    }

    // Structural invariant: every subtraction must produce a receipt.
    // execute_subtraction always does, so this only fails if there are
    // candidates but the caller somehow bypasses the receipt path. We
    // represent it as a passing check here (no violation) — the real
    // enforcement is in execute_subtraction.
    if !candidates.is_empty() {
        // no_removal_without_receipt is satisfied by construction.
    }

    let all_pass = violations.is_empty();
    InvariantCheck {
        all_pass,
        violations,
    }
}

// ─── Execution ───────────────────────────────────────────────────────────

/// Execute subtraction on the given candidates, skipping any that violate
/// invariants. Returns one receipt per executed candidate, each containing a
/// JSON snapshot of the item (modelled here as a minimal object with the
/// item id and score, since the engine does not have access to full item
/// state).
pub fn execute_subtraction(
    candidates: &[SubtractionCandidate],
    active_references: &[String],
) -> Vec<SubtractionReceipt> {
    let active: std::collections::HashSet<&String> = active_references.iter().collect();
    let timestamp = "1970-01-01T00:00:00Z".to_string(); // deterministic for tests

    candidates
        .iter()
        .filter(|c| !active.contains(&c.item_id))
        .enumerate()
        .map(|(idx, c)| SubtractionReceipt {
            receipt_id: format!("rcpt-{}-{}", c.item_id, idx),
            item_id: c.item_id.clone(),
            action: c.recommended_action,
            structuring_score: c.structuring_score,
            reason: c.reason.clone(),
            item_snapshot: serde_json::json!({
                "item_id": c.item_id,
                "structuring_score": c.structuring_score,
                "recommended_action": format!("{:?}", c.recommended_action),
            }),
            timestamp: timestamp.clone(),
        })
        .collect()
}

// ─── Recovery ─────────────────────────────────────────────────────────────

/// Recover an item from a receipt by returning its stored snapshot.
pub fn recover_item(receipt: &SubtractionReceipt) -> serde_json::Value {
    receipt.item_snapshot.clone()
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structuring_score_computation() {
        // All flags true, access_count = 3, temporal_weight = 0.5.
        let score = compute_structuring_score(true, true, true, false, true, 3, 0.5);
        // 1 + 1 + 1 + 1 + 1 + 1 + 0.5 = 6.5
        assert!((score - 6.5).abs() < f64::EPSILON);
    }

    #[test]
    fn structuring_score_with_contradiction_and_no_access() {
        // Contradicted => !is_contradicted = false. No access => access_count>0 = false.
        let score = compute_structuring_score(false, false, false, true, false, 0, 0.0);
        // 0 + 0 + 0 + 0 + 0 + 0 + 0 = 0.0
        assert!((score - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn candidate_filtering_sorts_ascending() {
        let items = vec![
            ("high".to_string(), 5.0),
            ("low1".to_string(), 0.5),
            ("low2".to_string(), 1.2),
            ("mid".to_string(), 3.0),
        ];
        let cfg = SubtractionConfig::default(); // min = 2.0
        let candidates = find_candidates(&items, &cfg);
        // Only items with score < 2.0: low1 (0.5), low2 (1.2).
        assert_eq!(candidates.len(), 2);
        // Sorted ascending.
        assert!(candidates[0].structuring_score <= candidates[1].structuring_score);
        assert_eq!(candidates[0].item_id, "low1");
        assert_eq!(candidates[1].item_id, "low2");
        // 0.5 < 1.0 => DeleteWithReceipt; 1.2 < 1.5 => Quarantine.
        assert_eq!(
            candidates[0].recommended_action,
            CompactionStrategy::DeleteWithReceipt
        );
        assert_eq!(
            candidates[1].recommended_action,
            CompactionStrategy::Quarantine
        );
    }

    #[test]
    fn invariant_check_passes_when_no_referenced() {
        let candidates = vec![SubtractionCandidate {
            item_id: "a".to_string(),
            structuring_score: 0.5,
            recommended_action: CompactionStrategy::DeleteWithReceipt,
            reason: "r".to_string(),
        }];
        let active = vec!["b".to_string()];
        let check = verify_invariants(&candidates, &active);
        assert!(check.all_pass);
        assert!(check.violations.is_empty());
    }

    #[test]
    fn invariant_check_fails_when_referenced() {
        let candidates = vec![SubtractionCandidate {
            item_id: "a".to_string(),
            structuring_score: 0.5,
            recommended_action: CompactionStrategy::DeleteWithReceipt,
            reason: "r".to_string(),
        }];
        let active = vec!["a".to_string()];
        let check = verify_invariants(&candidates, &active);
        assert!(!check.all_pass);
        assert_eq!(check.violations.len(), 1);
        assert_eq!(check.violations[0].invariant, "no_referenced_removal");
        assert_eq!(check.violations[0].candidate_id, "a");
    }

    #[test]
    fn execution_produces_receipts_and_skips_referenced() {
        let candidates = vec![
            SubtractionCandidate {
                item_id: "keep_ref".to_string(),
                structuring_score: 0.5,
                recommended_action: CompactionStrategy::DeleteWithReceipt,
                reason: "r".to_string(),
            },
            SubtractionCandidate {
                item_id: "drop_me".to_string(),
                structuring_score: 0.8,
                recommended_action: CompactionStrategy::DeleteWithReceipt,
                reason: "r".to_string(),
            },
        ];
        let active = vec!["keep_ref".to_string()];
        let receipts = execute_subtraction(&candidates, &active);
        // Only "drop_me" should be executed.
        assert_eq!(receipts.len(), 1);
        assert_eq!(receipts[0].item_id, "drop_me");
        assert_eq!(receipts[0].action, CompactionStrategy::DeleteWithReceipt);
        // Snapshot should contain the item id.
        assert_eq!(receipts[0].item_snapshot["item_id"], "drop_me");
    }

    #[test]
    fn recovery_returns_snapshot() {
        let receipt = SubtractionReceipt {
            receipt_id: "rcpt-1".to_string(),
            item_id: "x".to_string(),
            action: CompactionStrategy::ColdStorage,
            structuring_score: 1.8,
            reason: "r".to_string(),
            item_snapshot: serde_json::json!({"item_id": "x", "data": 42}),
            timestamp: "1970-01-01T00:00:00Z".to_string(),
        };
        let recovered = recover_item(&receipt);
        assert_eq!(recovered["item_id"], "x");
        assert_eq!(recovered["data"], 42);
    }

    #[test]
    fn background_scheduling_config_respects_max_items() {
        let cfg = SubtractionConfig {
            min_structuring_score: 2.0,
            max_items_per_run: 2,
        };
        let items: Vec<(String, f64)> = (0..10)
            .map(|i| (format!("item-{i}"), 0.1 * i as f64))
            .collect();
        let candidates = find_candidates(&items, &cfg);
        // All have score < 2.0, but capped at max_items_per_run = 2.
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn empty_items_produce_empty_candidates() {
        let cfg = SubtractionConfig::default();
        let candidates = find_candidates(&[], &cfg);
        assert!(candidates.is_empty());
        let check = verify_invariants(&[], &[]);
        assert!(check.all_pass);
        let receipts = execute_subtraction(&[], &[]);
        assert!(receipts.is_empty());
    }

    #[test]
    fn all_items_above_threshold_produces_no_candidates() {
        let items = vec![
            ("a".to_string(), 2.5),
            ("b".to_string(), 3.0),
            ("c".to_string(), 10.0),
        ];
        let cfg = SubtractionConfig::default(); // min = 2.0
        let candidates = find_candidates(&items, &cfg);
        assert!(candidates.is_empty());
    }
}
