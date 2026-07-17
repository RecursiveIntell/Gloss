//! Phase 3: Temporal field provenance — computed temporal_weight scores.
//!
//! Each item in semantic-memory gets a continuous temporal weight that decays
//! based on age, supersession, contradiction density, and support density.
//! Retrieval results can be boosted by temporal weight. Items in temporal
//! "wells" (dense clusters of mutually-supporting items) get additional boost.
//!
//! temporal_weight is a COMPUTED SCORE, not truth-bearing state. It may be
//! freely UPDATEd by `recompute_temporal_weights()`. This is the only column
//! in the schema where direct UPDATE is permitted.
//!
//! All temporal computations produce receipts.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Config ──────────────────────────────────────────────────────────────

/// Configuration for temporal weight computation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalConfig {
    /// Half-life for age decay (days). Default 365.0 (1 year).
    pub age_half_life_days: f64,
    /// Weight multiplier for superseded items. Default 0.1.
    pub superseded_weight: f64,
    /// Boost per supporting item. Default 0.1 (capped at support_boost_cap).
    pub support_boost_per_item: f64,
    /// Maximum support boost. Default 2.0.
    pub support_boost_cap: f64,
    /// Penalty per contradicting item. Default 0.15.
    pub contradiction_penalty_per_item: f64,
    /// Floor for contradiction factor. Default 0.1.
    pub contradiction_floor: f64,
    /// Threshold for temporal well detection. Items with more than this
    /// number of supporting neighbors are "in a well." Default 3.
    pub well_threshold: usize,
}

impl Default for TemporalConfig {
    fn default() -> Self {
        Self {
            age_half_life_days: 365.0,
            superseded_weight: 0.1,
            support_boost_per_item: 0.1,
            support_boost_cap: 2.0,
            contradiction_penalty_per_item: 0.15,
            contradiction_floor: 0.1,
            well_threshold: 3,
        }
    }
}

// ─── Weight computation ──────────────────────────────────────────────────

/// Compute the temporal weight for a single item.
///
/// weight = base * age_factor * supersession_factor * support_factor * contradiction_factor
///
/// - base: 1.0
/// - age_factor: exp(-age_days / half_life) (exponential decay, very slow)
/// - supersession_factor: superseded_weight if is_superseded, else 1.0
/// - support_factor: min(1.0 + support_boost_per_item * support_count, support_boost_cap)
/// - contradiction_factor: max(contradiction_floor, 1.0 - contradiction_penalty_per_item * contradiction_count)
pub fn compute_temporal_weight(
    recorded_at: DateTime<Utc>,
    is_superseded: bool,
    now: DateTime<Utc>,
    support_count: usize,
    contradiction_count: usize,
    config: &TemporalConfig,
) -> f64 {
    let base = 1.0;

    // Age factor: exponential decay
    let age_duration = now.signed_duration_since(recorded_at);
    let age_days = age_duration.num_seconds() as f64 / 86400.0;
    let age_days = age_days.max(0.0);
    let age_factor = (-age_days / config.age_half_life_days).exp();

    // Supersession factor
    let supersession_factor = if is_superseded {
        config.superseded_weight
    } else {
        1.0
    };

    // Support factor
    let support_factor =
        (1.0 + config.support_boost_per_item * support_count as f64).min(config.support_boost_cap);

    // Contradiction factor
    let contradiction_factor = (1.0
        - config.contradiction_penalty_per_item * contradiction_count as f64)
        .max(config.contradiction_floor);

    base * age_factor * supersession_factor * support_factor * contradiction_factor
}

// ─── Well detection ──────────────────────────────────────────────────────

/// A graph edge reference for well detection.
#[derive(Debug, Clone)]
pub struct GraphEdgeRef {
    pub source: String,
    pub target: String,
    pub edge_type: String,
}

/// Detect temporal "wells" — dense clusters of mutually-supporting items.
///
/// For each item, count the number of other items that:
/// 1. Share an episode or linked episode
/// 2. Are not contradicted (no contradiction edges)
/// 3. Share graph edges (support relationships)
///
/// If the count exceeds the threshold, the item is in a "well" and gets a boost.
///
/// Returns a map of item_id -> well_boost (1.0 = no boost, higher = well boost).
pub fn detect_temporal_wells(
    items: &[String],
    graph_edges: &[GraphEdgeRef],
    episode_links: &HashMap<String, Vec<String>>, // item_id -> linked episode_ids
    config: &TemporalConfig,
) -> HashMap<String, f64> {
    // Build adjacency from graph edges (support edges only)
    let mut support_neighbors: HashMap<String, Vec<String>> = HashMap::new();
    for edge in graph_edges {
        if edge.edge_type == "supports" || edge.edge_type == "supported_by" {
            support_neighbors
                .entry(edge.source.clone())
                .or_default()
                .push(edge.target.clone());
            support_neighbors
                .entry(edge.target.clone())
                .or_default()
                .push(edge.source.clone());
        }
    }

    // Count contradictions per item
    let mut contradiction_count: HashMap<String, usize> = HashMap::new();
    for edge in graph_edges {
        if edge.edge_type == "contradicts" || edge.edge_type == "contradicted_by" {
            *contradiction_count.entry(edge.source.clone()).or_default() += 1;
            *contradiction_count.entry(edge.target.clone()).or_default() += 1;
        }
    }

    let mut wells = HashMap::new();
    for item_id in items {
        let _neighbors = support_neighbors.get(item_id).map(|v| v.len()).unwrap_or(0);
        // Count only non-contradicted neighbors
        let clean_neighbors = support_neighbors
            .get(item_id)
            .map(|v| {
                v.iter()
                    .filter(|n| contradiction_count.get(*n).copied().unwrap_or(0) == 0)
                    .count()
            })
            .unwrap_or(0);

        // Also count items in the same episode
        let episode_item_count = episode_links
            .get(item_id)
            .map(|links| links.len())
            .unwrap_or(0);

        let total_support = clean_neighbors + episode_item_count;

        if total_support > config.well_threshold {
            // Well boost: linear in excess support, capped at 2.0
            let excess = total_support - config.well_threshold;
            let boost = (1.0 + 0.1 * excess as f64).min(2.0);
            wells.insert(item_id.clone(), boost);
        } else {
            wells.insert(item_id.clone(), 1.0);
        }
    }
    wells
}

// ─── Receipt ──────────────────────────────────────────────────────────────

/// Receipt for a temporal weight recomputation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TemporalRecomputationReceipt {
    pub receipt_id: String,
    pub timestamp: String,
    pub items_computed: usize,
    pub items_updated: usize,
    pub duration_ms: u64,
    pub config: TemporalConfig,
}

// ─── Search result boosting ──────────────────────────────────────────────

/// A search result item with a score that can be boosted by temporal weight.
#[derive(Debug, Clone)]
pub struct TemporalSearchItem {
    pub item_id: String,
    pub base_score: f64,
    pub temporal_weight: f64,
    pub well_boost: f64,
}

/// Apply temporal boost to search results.
/// final_score = base_score * temporal_weight * well_boost
/// Returns items sorted by final_score descending.
pub fn apply_temporal_boost(items: Vec<TemporalSearchItem>) -> Vec<TemporalSearchItem> {
    let mut boosted: Vec<TemporalSearchItem> = items
        .into_iter()
        .map(|item| {
            let final_score = item.base_score * item.temporal_weight * item.well_boost;
            TemporalSearchItem {
                item_id: item.item_id,
                base_score: final_score,
                temporal_weight: item.temporal_weight,
                well_boost: item.well_boost,
            }
        })
        .collect();
    boosted.sort_by(|a, b| {
        b.base_score
            .partial_cmp(&a.base_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    boosted
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn test_episode(
        recorded_at: DateTime<Utc>,
        valid_to: Option<DateTime<Utc>>,
    ) -> (DateTime<Utc>, bool) {
        (recorded_at, valid_to.is_some())
    }

    #[test]
    fn test_temporal_weight_active_item() {
        let config = TemporalConfig::default();
        let now = Utc::now();
        let (recorded_at, is_superseded) = test_episode(now, None);
        let weight = compute_temporal_weight(recorded_at, is_superseded, now, 0, 0, &config);
        // Fresh, active, no support, no contradictions → weight ≈ 1.0
        assert!(
            (weight - 1.0).abs() < 0.01,
            "fresh active item should be ~1.0, got {weight}"
        );
    }

    #[test]
    fn test_temporal_weight_superseded() {
        let config = TemporalConfig::default();
        let now = Utc::now();
        let past = now - chrono::Duration::days(1);
        let (recorded_at, is_superseded) = test_episode(past, Some(now));
        let weight = compute_temporal_weight(recorded_at, is_superseded, now, 0, 0, &config);
        // Superseded → weight drops to superseded_weight (0.1) * age_factor
        assert!(
            weight < 0.2,
            "superseded item should be < 0.2, got {weight}"
        );
        assert!(
            weight > 0.05,
            "superseded item should be > 0.05, got {weight}"
        );
    }

    #[test]
    fn test_temporal_weight_aged() {
        let config = TemporalConfig::default();
        let now = Utc::now();
        let old = now - chrono::Duration::days(365 * 2); // 2 years
        let (recorded_at, is_superseded) = test_episode(old, None);
        let weight = compute_temporal_weight(recorded_at, is_superseded, now, 0, 0, &config);
        // 2 half-lives → age_factor ≈ exp(-2) ≈ 0.135
        assert!(
            weight < 0.2,
            "2-year-old item should be < 0.2, got {weight}"
        );
        assert!(
            weight > 0.05,
            "2-year-old item should be > 0.05, got {weight}"
        );
    }

    #[test]
    fn test_temporal_weight_supported() {
        let config = TemporalConfig::default();
        let now = Utc::now();
        let (recorded_at, is_superseded) = test_episode(now, None);
        let weight = compute_temporal_weight(recorded_at, is_superseded, now, 5, 0, &config);
        // 5 supports → support_factor = min(1.0 + 0.1*5, 2.0) = 1.5
        let expected = 1.0 * 1.0 * 1.5 * 1.0; // base * age * support * contradiction
        assert!(
            (weight - expected).abs() < 0.01,
            "supported item: expected {expected}, got {weight}"
        );
    }

    #[test]
    fn test_temporal_weight_support_cap() {
        let config = TemporalConfig::default();
        let now = Utc::now();
        let (recorded_at, is_superseded) = test_episode(now, None);
        let weight = compute_temporal_weight(recorded_at, is_superseded, now, 100, 0, &config);
        // 100 supports → support_factor capped at 2.0
        let expected = 1.0 * 1.0 * 2.0 * 1.0;
        assert!(
            (weight - expected).abs() < 0.01,
            "capped support: expected {expected}, got {weight}"
        );
    }

    #[test]
    fn test_temporal_weight_contradicted() {
        let config = TemporalConfig::default();
        let now = Utc::now();
        let (recorded_at, is_superseded) = test_episode(now, None);
        let weight = compute_temporal_weight(recorded_at, is_superseded, now, 0, 3, &config);
        // 3 contradictions → contradiction_factor = max(0.1, 1.0 - 0.15*3) = max(0.1, 0.55) = 0.55
        let expected = 1.0 * 1.0 * 1.0 * 0.55;
        assert!(
            (weight - expected).abs() < 0.01,
            "contradicted item: expected {expected}, got {weight}"
        );
    }

    #[test]
    fn test_temporal_weight_contradiction_floor() {
        let config = TemporalConfig::default();
        let now = Utc::now();
        let (recorded_at, is_superseded) = test_episode(now, None);
        let weight = compute_temporal_weight(recorded_at, is_superseded, now, 0, 100, &config);
        // 100 contradictions → contradiction_factor = max(0.1, 1.0 - 0.15*100) = max(0.1, -14.0) = 0.1
        let expected = 1.0 * 1.0 * 1.0 * 0.1;
        assert!(
            (weight - expected).abs() < 0.01,
            "floor: expected {expected}, got {weight}"
        );
    }

    #[test]
    fn test_temporal_well_detection() {
        let config = TemporalConfig::default();
        let items = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let graph_edges = vec![
            GraphEdgeRef {
                source: "a".into(),
                target: "b".into(),
                edge_type: "supports".into(),
            },
            GraphEdgeRef {
                source: "a".into(),
                target: "c".into(),
                edge_type: "supports".into(),
            },
            GraphEdgeRef {
                source: "a".into(),
                target: "d".into(),
                edge_type: "supports".into(),
            },
            GraphEdgeRef {
                source: "b".into(),
                target: "c".into(),
                edge_type: "supports".into(),
            },
            GraphEdgeRef {
                source: "b".into(),
                target: "d".into(),
                edge_type: "supports".into(),
            },
        ];
        let mut episode_links = HashMap::new();
        episode_links.insert("a".to_string(), vec!["b".to_string(), "c".to_string()]);

        let wells = detect_temporal_wells(&items, &graph_edges, &episode_links, &config);
        // 'a' has 3 support neighbors + 2 episode links = 5 > threshold 3 → boost
        let a_boost = wells.get("a").copied().unwrap_or(1.0);
        assert!(
            a_boost > 1.0,
            "item 'a' should be in a well, got boost {a_boost}"
        );
        // 'd' has 2 support neighbors + 0 episode links = 2 < threshold 3 → no boost
        let d_boost = wells.get("d").copied().unwrap_or(1.0);
        assert!(
            (d_boost - 1.0).abs() < 0.01,
            "item 'd' should NOT be in a well, got boost {d_boost}"
        );
    }

    #[test]
    fn test_temporal_well_isolated() {
        let config = TemporalConfig::default();
        let items = vec!["x".to_string()];
        let graph_edges: Vec<GraphEdgeRef> = vec![];
        let episode_links = HashMap::new();
        let wells = detect_temporal_wells(&items, &graph_edges, &episode_links, &config);
        let x_boost = wells.get("x").copied().unwrap_or(1.0);
        assert!(
            (x_boost - 1.0).abs() < 0.01,
            "isolated item should have boost 1.0, got {x_boost}"
        );
    }

    #[test]
    fn test_temporal_boost_reorders_results() {
        let items = vec![
            TemporalSearchItem {
                item_id: "low".into(),
                base_score: 0.9,
                temporal_weight: 0.1,
                well_boost: 1.0,
            },
            TemporalSearchItem {
                item_id: "high".into(),
                base_score: 0.5,
                temporal_weight: 2.0,
                well_boost: 1.5,
            },
        ];
        let boosted = apply_temporal_boost(items);
        // low: 0.9 * 0.1 * 1.0 = 0.09
        // high: 0.5 * 2.0 * 1.5 = 1.5
        assert_eq!(boosted[0].item_id, "high");
        assert_eq!(boosted[1].item_id, "low");
    }

    #[test]
    fn test_recompute_receipt_fields() {
        let receipt = TemporalRecomputationReceipt {
            receipt_id: "test".to_string(),
            timestamp: "2026-06-18T00:00:00Z".to_string(),
            items_computed: 100,
            items_updated: 50,
            duration_ms: 42,
            config: TemporalConfig::default(),
        };
        assert_eq!(receipt.items_computed, 100);
        assert_eq!(receipt.items_updated, 50);
        assert_eq!(receipt.config.age_half_life_days, 365.0);
    }
}
