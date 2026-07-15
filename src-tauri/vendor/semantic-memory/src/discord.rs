//! Discord-structured second-order retrieval.
//!
//! Finds items that are related to the top-K direct search results but are
//! *not* themselves in the direct results, using graph edges. This is a
//! second-order retrieval step: given the anchors (direct hits) and a set of
//! graph edges, surface neighbours of the anchors as "discord" results ranked
//! by edge weight scaled by anchor position.
//!
//! Graceful degradation: an empty graph or empty direct-result set yields an
//! empty result vector — never a crash.

use serde::{Deserialize, Serialize};

use std::collections::HashMap;

/// Configuration for discord-structured retrieval.
#[derive(Debug, Clone)]
pub struct DiscordConfig {
    /// How many of the top direct results to use as anchors. Defaults to 5.
    pub top_k: usize,
    /// Maximum number of discord neighbours to return. Defaults to 10.
    pub max_neighbors: usize,
    /// Minimum discord score to keep a neighbour. Defaults to 0.1.
    pub min_score: f64,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            top_k: 5,
            max_neighbors: 10,
            min_score: 0.1,
        }
    }
}

impl DiscordConfig {
    /// Convenience constructor overriding defaults.
    pub fn new(top_k: usize, max_neighbors: usize, min_score: f64) -> Self {
        Self {
            top_k,
            max_neighbors,
            min_score,
        }
    }
}

/// A lightweight, owned reference to a graph edge used for discord scoring.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphEdgeRef {
    pub source: String,
    pub target: String,
    pub edge_type: String,
    pub weight: f64,
}

impl GraphEdgeRef {
    /// Convenience constructor.
    pub fn new(
        source: impl Into<String>,
        target: impl Into<String>,
        edge_type: impl Into<String>,
        weight: f64,
    ) -> Self {
        Self {
            source: source.into(),
            target: target.into(),
            edge_type: edge_type.into(),
            weight,
        }
    }
}

/// A single discord-structured retrieval result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiscordResult {
    /// The neighbour item id (not present in the direct results).
    pub item_id: String,
    /// Aggregated discord score. Higher is better.
    pub discord_score: f64,
    /// Anchor ids (direct results) that connect to this neighbour.
    pub anchor_ids: Vec<String>,
    /// Relationship/edge types observed connecting the anchors to this item.
    pub relationship_types: Vec<String>,
}

/// Scores discord-structured second-order retrieval results.
pub struct DiscordScorer {
    config: DiscordConfig,
}

impl DiscordScorer {
    /// Create a scorer with the given configuration.
    pub fn new(config: DiscordConfig) -> Self {
        Self { config }
    }

    /// Create a scorer with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(DiscordConfig::default())
    }

    /// Compute discord results for the given direct result ids and graph edges.
    ///
    /// For each direct result (anchor), we look at graph edges whose `source`
    /// or `target` is the anchor and whose *other* endpoint is NOT itself a
    /// direct result. The neighbour's score contribution is
    /// `edge_weight * (1.0 - anchor_position / direct_count)`, where
    /// `anchor_position` is the 0-based index of the anchor in the
    /// direct-result list (earlier/better anchors contribute more).
    ///
    /// Scores are summed across anchors, so a neighbour connected to multiple
    /// anchors accumulates a higher score. Results are sorted best-first and
    /// truncated to `max_neighbors`, dropping anything below `min_score`.
    pub fn score(
        &self,
        direct_result_ids: &[String],
        graph_edges: &[GraphEdgeRef],
    ) -> Vec<DiscordResult> {
        // Graceful degradation.
        if direct_result_ids.is_empty() || graph_edges.is_empty() {
            return Vec::new();
        }

        let anchors: Vec<&String> = direct_result_ids.iter().take(self.config.top_k).collect();
        let direct_set: std::collections::HashSet<&String> = direct_result_ids.iter().collect();
        let anchor_positions: HashMap<&String, usize> = anchors
            .iter()
            .enumerate()
            .map(|(pos, id)| (*id, pos))
            .collect();
        let direct_count = anchors.len() as f64;

        // Accumulate per-neighbour scores and provenance.
        let mut scores: HashMap<String, f64> = HashMap::new();
        let mut anchor_map: HashMap<String, Vec<String>> = HashMap::new();
        let mut rel_map: HashMap<String, Vec<String>> = HashMap::new();

        for edge in graph_edges {
            // Determine which endpoint is an anchor and which is the candidate
            // neighbour. We consider both directions of the edge.
            let neighbours: Vec<(&String, &String)> = if anchor_positions.contains_key(&edge.source)
            {
                // source is an anchor → target is a candidate neighbour.
                if direct_set.contains(&edge.target) {
                    // Both endpoints are direct results; skip.
                    Vec::new()
                } else {
                    vec![(&edge.target, &edge.source)]
                }
            } else if anchor_positions.contains_key(&edge.target) {
                if direct_set.contains(&edge.source) {
                    Vec::new()
                } else {
                    vec![(&edge.source, &edge.target)]
                }
            } else {
                Vec::new()
            };

            for (neighbour, anchor) in neighbours {
                let anchor_pos = anchor_positions.get(anchor).copied().unwrap_or(0) as f64;
                let contribution = edge.weight * (1.0 - anchor_pos / direct_count);
                *scores.entry(neighbour.clone()).or_insert(0.0) += contribution;
                let anchor_list = anchor_map.entry(neighbour.clone()).or_default();
                if !anchor_list.contains(anchor) {
                    anchor_list.push(anchor.clone());
                }
                let rel_list = rel_map.entry(neighbour.clone()).or_default();
                if !rel_list.contains(&edge.edge_type) {
                    rel_list.push(edge.edge_type.clone());
                }
            }
        }

        let mut results: Vec<DiscordResult> = scores
            .into_iter()
            .filter(|(_, s)| *s >= self.config.min_score)
            .map(|(item_id, discord_score)| {
                let anchor_ids = anchor_map.remove(&item_id).unwrap_or_default();
                let relationship_types = rel_map.remove(&item_id).unwrap_or_default();
                DiscordResult {
                    item_id,
                    discord_score,
                    anchor_ids,
                    relationship_types,
                }
            })
            .collect();

        // Sort by score descending; break ties by item_id for determinism.
        results.sort_by(|a, b| {
            b.discord_score
                .partial_cmp(&a.discord_score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.item_id.cmp(&b.item_id))
        });

        results.truncate(self.config.max_neighbors);
        results
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edge(source: &str, target: &str, etype: &str, weight: f64) -> GraphEdgeRef {
        GraphEdgeRef::new(source, target, etype, weight)
    }

    #[test]
    fn test_discord_finds_neighbors() {
        // Anchors: a, b. Neighbours: x (via a), y (via b).
        let direct = vec!["a".to_string(), "b".to_string()];
        let edges = vec![
            edge("a", "x", "related", 0.5),
            edge("b", "y", "related", 0.5),
        ];
        let scorer = DiscordScorer::with_defaults();
        let res = scorer.score(&direct, &edges);
        let ids: Vec<&str> = res.iter().map(|r| r.item_id.as_str()).collect();
        assert!(ids.contains(&"x"));
        assert!(ids.contains(&"y"));
    }

    #[test]
    fn test_discord_excludes_direct_results() {
        // An edge between two anchors should not produce a discord result.
        let direct = vec!["a".to_string(), "b".to_string()];
        let edges = vec![
            edge("a", "b", "related", 0.9),
            edge("a", "x", "related", 0.5),
        ];
        let scorer = DiscordScorer::with_defaults();
        let res = scorer.score(&direct, &edges);
        let ids: Vec<&str> = res.iter().map(|r| r.item_id.as_str()).collect();
        assert!(!ids.contains(&"a"));
        assert!(!ids.contains(&"b"));
        assert!(ids.contains(&"x"));
    }

    #[test]
    fn test_discord_min_score_filter() {
        // A neighbour with a tiny contribution should be filtered out.
        let direct = vec!["a".to_string()];
        let edges = vec![
            edge("a", "low", "related", 0.05), // 0.05 * (1 - 0/1) = 0.05 < 0.1
            edge("a", "high", "related", 0.5), // 0.5 >= 0.1
        ];
        let scorer = DiscordScorer::with_defaults(); // min_score = 0.1
        let res = scorer.score(&direct, &edges);
        let ids: Vec<&str> = res.iter().map(|r| r.item_id.as_str()).collect();
        assert!(ids.contains(&"high"));
        assert!(!ids.contains(&"low"));
    }

    #[test]
    fn test_discord_max_neighbors_limit() {
        // Many neighbours, but only max_neighbors returned.
        let direct = vec!["a".to_string()];
        let edges: Vec<GraphEdgeRef> = (0..20)
            .map(|i| edge("a", &format!("n{}", i), "related", 0.5))
            .collect();
        let config = DiscordConfig::new(5, 3, 0.1);
        let scorer = DiscordScorer::new(config);
        let res = scorer.score(&direct, &edges);
        assert!(res.len() <= 3);
        assert_eq!(res.len(), 3);
    }

    #[test]
    fn test_discord_empty_graph() {
        let direct = vec!["a".to_string()];
        let edges: Vec<GraphEdgeRef> = Vec::new();
        let scorer = DiscordScorer::with_defaults();
        let res = scorer.score(&direct, &edges);
        assert!(res.is_empty());
    }

    #[test]
    fn test_discord_empty_direct_results() {
        let direct: Vec<String> = Vec::new();
        let edges = vec![edge("a", "x", "related", 0.5)];
        let scorer = DiscordScorer::with_defaults();
        let res = scorer.score(&direct, &edges);
        assert!(res.is_empty());
    }

    #[test]
    fn test_discord_multiple_anchors() {
        // An item connected to two anchors should score higher than one
        // connected to a single anchor.
        let direct = vec!["a".to_string(), "b".to_string()];
        let edges = vec![
            edge("a", "multi", "related", 0.5),
            edge("b", "multi", "related", 0.5),
            edge("a", "single", "related", 0.5),
        ];
        let scorer = DiscordScorer::with_defaults();
        let res = scorer.score(&direct, &edges);
        let multi = res
            .iter()
            .find(|r| r.item_id == "multi")
            .expect("multi present");
        let single = res
            .iter()
            .find(|r| r.item_id == "single")
            .expect("single present");
        assert!(
            multi.discord_score > single.discord_score,
            "multi ({}) should beat single ({})",
            multi.discord_score,
            single.discord_score
        );
        // multi should be the top result.
        assert_eq!(res[0].item_id, "multi");
        // multi should have both anchors recorded.
        assert_eq!(multi.anchor_ids.len(), 2);
    }
}
