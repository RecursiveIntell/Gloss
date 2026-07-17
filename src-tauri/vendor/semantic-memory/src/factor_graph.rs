//! Factor graph unification of heterogeneous graph edges.
//!
//! Models all 4 edge types (semantic, temporal, causal, entity) as factors
//! in a single probabilistic reasoning framework. Belief propagation runs
//! across the combined graph, producing unified confidence scores that
//! reflect ALL relationship types simultaneously.
//!
//! This is the single most novel combination in the semantic-memory system:
//! no existing system unifies heterogeneous edge types into a single factor
//! graph with provenance-weighted belief propagation.
//!
//! ## Architecture
//!
//! Each edge type becomes a factor function:
//! - **Semantic edges** → cosine similarity factors (0.0–1.0)
//! - **Temporal edges** → time-decay factors (recency-weighted)
//! - **Causal edges** → confidence-weighted implication factors
//! - **Entity edges** → structural constraint factors (binary support)
//!
//! Nodes are initialized with provenance confidence values (from the
//! ConfidenceSemiring). Message passing propagates belief across all edge
//! types simultaneously. A node that is semantically similar AND temporally
//! recent AND causally linked AND structurally connected gets a compounded
//! confidence score.
//!
//! ## Relationship to existing decoder
//!
//! The decoder's `ConflictGraph` + `pass_messages()` runs BP on
//! contradiction edges only. This module generalizes that to ALL edge
//! types. The decoder remains the contradiction-detection specialist; the
//! factor graph is the unified reasoning layer.
//!
//! Behind `#[cfg(feature = "integration")]` which requires all constituent
//! features (provenance, temporal, decoder, etc.).

#![cfg(feature = "integration")]

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::decoder::MessagePassingResult;
use crate::types::GraphEdgeType;

// ─── Factor types ───────────────────────────────────────────────────────

/// The kind of factor derived from an edge type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum FactorKind {
    /// Semantic similarity factor (cosine-based).
    Semantic,
    /// Temporal recency factor (time-decay-based).
    Temporal,
    /// Causal implication factor (confidence-weighted).
    Causal,
    /// Entity structural factor (binary support).
    Entity,
}

impl FactorKind {
    /// Map a GraphEdgeType to its factor kind.
    pub fn from_edge_type(edge_type: &GraphEdgeType) -> Self {
        match edge_type {
            GraphEdgeType::Semantic { .. } => FactorKind::Semantic,
            GraphEdgeType::Temporal { .. } => FactorKind::Temporal,
            GraphEdgeType::Causal { .. } => FactorKind::Causal,
            GraphEdgeType::Entity { .. } => FactorKind::Entity,
        }
    }

    /// Default mixing weight for this factor kind (how much it influences
    /// the combined belief). These are starting weights; callers can
    /// override them via `FactorGraphConfig`.
    pub fn default_weight(&self) -> f64 {
        match self {
            FactorKind::Semantic => 0.35,
            FactorKind::Temporal => 0.20,
            FactorKind::Causal => 0.30,
            FactorKind::Entity => 0.15,
        }
    }
}

// ─── Factor ─────────────────────────────────────────────────────────────

/// A factor connecting two nodes, derived from a graph edge.
///
/// The factor function maps the pair (source_belief, target_belief) to
/// a compatibility score in [0, 1]. High compatibility means the two
/// nodes' beliefs reinforce each other; low compatibility means they
/// pull each other apart (contradiction-like).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Factor {
    /// Source node ID.
    pub source: String,
    /// Target node ID.
    pub target: String,
    /// Factor kind.
    pub kind: FactorKind,
    /// Edge weight (from the stored graph edge).
    pub edge_weight: f64,
    /// Optional metadata from the edge (e.g. delta_secs for temporal,
    /// confidence for causal).
    pub metadata: Option<FactorMetadata>,
}

/// Type-specific metadata for a factor.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FactorMetadata {
    /// Semantic: cosine similarity (if known).
    Semantic { cosine_similarity: Option<f64> },
    /// Temporal: time delta in seconds.
    Temporal { delta_secs: i64 },
    /// Causal: confidence value from the causal edge.
    Causal { confidence: f64 },
    /// Entity: relation name (e.g. "depends_on", "parent_of").
    Entity { relation: String },
}

impl Factor {
    /// Compute the factor function: how compatible are these two beliefs?
    ///
    /// Returns a value in [0, 1] where:
    /// - 1.0 = fully reinforcing (both beliefs agree and are high)
    /// - 0.5 = neutral
    /// - 0.0 = fully contradicting (beliefs disagree)
    ///
    /// The exact computation depends on the factor kind:
    /// - Semantic: weighted by edge_weight * cosine_similarity (or 1.0 if
    ///   cosine unknown).
    /// - Temporal: decays with delta_secs (more recent = stronger factor).
    /// - Causal: weighted by causal confidence.
    /// - Entity: binary structural support (edge_weight only).
    pub fn compatibility(&self, source_belief: f64, target_belief: f64) -> f64 {
        let base = match &self.metadata {
            Some(FactorMetadata::Semantic { cosine_similarity }) => {
                let cos = cosine_similarity.unwrap_or(1.0).abs();
                // If both beliefs are high and cosine is high → reinforcing.
                // If beliefs disagree (one high, one low) and cosine is high → contradicting.
                let agreement = 1.0 - (source_belief - target_belief).abs();
                cos * agreement
            }
            Some(FactorMetadata::Temporal { delta_secs }) => {
                // Temporal decay: factor weakens with time.
                // Half-life of 30 days (2592000 seconds).
                let half_life_secs = 2_592_000.0_f64;
                let decay = (-delta_secs.abs() as f64 / half_life_secs).exp();
                // More recent connections reinforce; old ones fade.
                let agreement = 1.0 - (source_belief - target_belief).abs();
                decay * agreement
            }
            Some(FactorMetadata::Causal { confidence }) => {
                // Causal: the cause's belief propagates to the effect.
                // If cause is confident, effect should be confident too.
                let agreement = 1.0 - (source_belief - target_belief).abs();
                confidence * agreement
            }
            Some(FactorMetadata::Entity { relation: _ }) => {
                // Entity: structural support. Connected items should have
                // compatible beliefs.
                let agreement = 1.0 - (source_belief - target_belief).abs();
                self.edge_weight * agreement
            }
            None => {
                // No metadata: use edge weight only.
                let agreement = 1.0 - (source_belief - target_belief).abs();
                self.edge_weight * agreement
            }
        };
        base.clamp(0.0, 1.0)
    }
}

// ─── Factor graph node ──────────────────────────────────────────────────

/// A node in the factor graph.
#[derive(Debug, Clone)]
pub struct FactorGraphNode {
    /// Node ID (e.g. "fact:<uuid>").
    pub item_id: String,
    /// Initial belief (from provenance confidence or search score).
    pub initial_belief: f64,
    /// Factors connecting this node to others.
    pub factors: Vec<Factor>,
}

/// The factor graph.
#[derive(Debug, Clone)]
pub struct FactorGraph {
    /// All nodes, keyed by item_id.
    pub nodes: HashMap<String, FactorGraphNode>,
    /// Mixing weights for each factor kind.
    pub config: FactorGraphConfig,
}

/// Configuration for factor graph belief propagation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactorGraphConfig {
    /// Weight for semantic factors.
    pub semantic_weight: f64,
    /// Weight for temporal factors.
    pub temporal_weight: f64,
    /// Weight for causal factors.
    pub causal_weight: f64,
    /// Weight for entity factors.
    pub entity_weight: f64,
    /// How much the node's own initial belief matters (vs neighbor influence).
    pub self_influence: f64,
    /// Max iterations for message passing.
    pub max_iterations: usize,
    /// Convergence threshold.
    pub convergence_threshold: f64,
}

impl Default for FactorGraphConfig {
    fn default() -> Self {
        Self {
            semantic_weight: FactorKind::Semantic.default_weight(),
            temporal_weight: FactorKind::Temporal.default_weight(),
            causal_weight: FactorKind::Causal.default_weight(),
            entity_weight: FactorKind::Entity.default_weight(),
            self_influence: 0.6,
            max_iterations: 50,
            convergence_threshold: 0.001,
        }
    }
}

impl FactorGraphConfig {
    /// Get the weight for a factor kind.
    pub fn weight_for(&self, kind: FactorKind) -> f64 {
        match kind {
            FactorKind::Semantic => self.semantic_weight,
            FactorKind::Temporal => self.temporal_weight,
            FactorKind::Causal => self.causal_weight,
            FactorKind::Entity => self.entity_weight,
        }
    }

    /// Normalize weights so they sum to 1.0.
    pub fn normalized_weights(&self) -> (f64, f64, f64, f64) {
        let total =
            self.semantic_weight + self.temporal_weight + self.causal_weight + self.entity_weight;
        if total <= 0.0 {
            return (0.25, 0.25, 0.25, 0.25);
        }
        (
            self.semantic_weight / total,
            self.temporal_weight / total,
            self.causal_weight / total,
            self.entity_weight / total,
        )
    }
}

impl FactorGraph {
    /// Build a factor graph from nodes and factors.
    ///
    /// `nodes` is a list of (item_id, initial_belief). `factors` is a list
    /// of Factor edges connecting nodes.
    pub fn new(nodes: &[(String, f64)], factors: Vec<Factor>, config: FactorGraphConfig) -> Self {
        let mut node_map: HashMap<String, FactorGraphNode> = nodes
            .iter()
            .map(|(id, belief)| {
                (
                    id.clone(),
                    FactorGraphNode {
                        item_id: id.clone(),
                        initial_belief: *belief,
                        factors: Vec::new(),
                    },
                )
            })
            .collect();

        // Attach factors to nodes (both source and target get a copy so
        // propagation works bidirectionally).
        for factor in &factors {
            // Ensure both nodes exist.
            if !node_map.contains_key(&factor.source) {
                node_map.insert(
                    factor.source.clone(),
                    FactorGraphNode {
                        item_id: factor.source.clone(),
                        initial_belief: 0.5,
                        factors: Vec::new(),
                    },
                );
            }
            if !node_map.contains_key(&factor.target) {
                node_map.insert(
                    factor.target.clone(),
                    FactorGraphNode {
                        item_id: factor.target.clone(),
                        initial_belief: 0.5,
                        factors: Vec::new(),
                    },
                );
            }
            if let Some(node) = node_map.get_mut(&factor.source) {
                node.factors.push(factor.clone());
            }
            if let Some(node) = node_map.get_mut(&factor.target) {
                node.factors.push(factor.clone());
            }
        }

        Self {
            nodes: node_map,
            config,
        }
    }

    /// Run belief propagation across the factor graph.
    ///
    /// This generalizes the decoder's `pass_messages()` from
    /// contradiction-only edges to ALL edge types. Each factor contributes
    /// a weighted message based on its compatibility function.
    ///
    /// Convergence: stop when max delta < threshold OR max_iterations reached.
    pub fn propagate(&self) -> FactorGraphResult {
        let start = std::time::Instant::now();

        // Initialize beliefs.
        let mut current: HashMap<String, f64> = self
            .nodes
            .iter()
            .map(|(id, node)| (id.clone(), node.initial_belief))
            .collect();

        let (sem_w, temp_w, causal_w, ent_w) = self.config.normalized_weights();
        let self_inf = self.config.self_influence;
        let neighbor_inf = 1.0 - self_inf;

        let mut iterations = 0;
        let mut converged = false;

        for iter in 0..self.config.max_iterations {
            iterations = iter + 1;
            let mut max_delta: f64 = 0.0;
            let mut next: HashMap<String, f64> = HashMap::new();

            for (id, node) in &self.nodes {
                let current_belief = *current.get(id).unwrap_or(&node.initial_belief);

                if node.factors.is_empty() {
                    // No factors → keep initial belief.
                    next.insert(id.clone(), node.initial_belief);
                    continue;
                }

                // Gather weighted messages from all factors.
                let mut weighted_sum: f64 = 0.0;
                let mut weight_total: f64 = 0.0;

                for factor in &node.factors {
                    let neighbor_id = if factor.source == *id {
                        &factor.target
                    } else {
                        &factor.source
                    };
                    let neighbor_belief = *current.get(neighbor_id).unwrap_or(&0.5);

                    let compatibility = factor.compatibility(current_belief, neighbor_belief);
                    let kind_weight = match factor.kind {
                        FactorKind::Semantic => sem_w,
                        FactorKind::Temporal => temp_w,
                        FactorKind::Causal => causal_w,
                        FactorKind::Entity => ent_w,
                    };
                    let combined_weight = kind_weight * factor.edge_weight;

                    // The message from this factor: if compatibility is high,
                    // the neighbor's belief reinforces our belief. If low,
                    // it pulls us toward neutral (0.5).
                    let message = if compatibility > 0.5 {
                        // Reinforcing: move toward neighbor's belief.
                        neighbor_belief * (2.0 * compatibility - 1.0)
                            + current_belief * (2.0 - 2.0 * compatibility)
                    } else {
                        // Contradicting: pull toward 0.5 (uncertainty).
                        0.5 * (1.0 - 2.0 * compatibility) + current_belief * 2.0 * compatibility
                    };

                    weighted_sum += message * combined_weight;
                    weight_total += combined_weight;
                }

                let new_belief = if weight_total > 0.0 {
                    let neighbor_avg = weighted_sum / weight_total;
                    self_inf * node.initial_belief + neighbor_inf * neighbor_avg
                } else {
                    node.initial_belief
                };

                let new_belief = new_belief.clamp(0.0, 1.0);
                let delta = (new_belief - current_belief).abs();
                if delta > max_delta {
                    max_delta = delta;
                }
                next.insert(id.clone(), new_belief);
            }

            current = next;

            if max_delta < self.config.convergence_threshold {
                converged = true;
                break;
            }
        }

        // Compute per-kind influence diagnostics. Count unique factors
        // (each factor is attached to both source and target, so we
        // deduplicate by checking source==id).
        let mut kind_counts: HashMap<FactorKind, usize> = HashMap::new();
        for node in self.nodes.values() {
            for factor in &node.factors {
                if factor.source == node.item_id {
                    *kind_counts.entry(factor.kind).or_default() += 1;
                }
            }
        }

        FactorGraphResult {
            node_beliefs: current,
            iterations,
            converged,
            elapsed_ms: start.elapsed().as_millis() as u64,
            factor_counts: FactorCounts {
                semantic: *kind_counts.get(&FactorKind::Semantic).unwrap_or(&0),
                temporal: *kind_counts.get(&FactorKind::Temporal).unwrap_or(&0),
                causal: *kind_counts.get(&FactorKind::Causal).unwrap_or(&0),
                entity: *kind_counts.get(&FactorKind::Entity).unwrap_or(&0),
            },
            config: self.config.clone(),
        }
    }
}

// ─── Result ─────────────────────────────────────────────────────────────

/// Result of factor graph belief propagation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FactorGraphResult {
    /// item_id -> refined belief after convergence.
    pub node_beliefs: HashMap<String, f64>,
    /// Number of iterations run.
    pub iterations: usize,
    /// Whether belief propagation converged.
    pub converged: bool,
    /// Elapsed time in milliseconds.
    pub elapsed_ms: u64,
    /// Number of factors per kind.
    pub factor_counts: FactorCounts,
    /// Configuration used.
    pub config: FactorGraphConfig,
}

/// Count of factors by kind.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FactorCounts {
    pub semantic: usize,
    pub temporal: usize,
    pub causal: usize,
    pub entity: usize,
}

impl FactorCounts {
    pub fn total(&self) -> usize {
        self.semantic + self.temporal + self.causal + self.entity
    }
}

impl FactorGraphResult {
    /// Convert to the decoder's MessagePassingResult format for
    /// compatibility with existing integration wiring (e.g.
    /// `confidence_aware_quantization`).
    pub fn to_message_passing_result(&self) -> MessagePassingResult {
        MessagePassingResult {
            node_confidences: self.node_beliefs.clone(),
            iterations: self.iterations,
            converged: self.converged,
            elapsed_ms: self.elapsed_ms,
        }
    }

    /// Get the top-K items by refined belief.
    pub fn top_k(&self, k: usize) -> Vec<(String, f64)> {
        let mut items: Vec<(String, f64)> = self
            .node_beliefs
            .iter()
            .map(|(id, belief)| (id.clone(), *belief))
            .collect();
        items.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        items.truncate(k);
        items
    }
}

// ─── Builder from stored graph edges ────────────────────────────────────

/// Build a list of Factors from stored graph edges.
///
/// `edges` is a list of (source, target, edge_type, weight, metadata_json).
/// The edge metadata is derived from `GraphEdgeType`, and `metadata_json`
/// can override type-specific fields when provided.
pub fn factors_from_edges(
    edges: &[(String, String, GraphEdgeType, f64, Option<String>)],
) -> Vec<Factor> {
    edges
        .iter()
        .map(|(source, target, edge_type, weight, metadata_json)| {
            let kind = FactorKind::from_edge_type(edge_type);
            let mut metadata = match edge_type {
                GraphEdgeType::Semantic { cosine_similarity } => Some(FactorMetadata::Semantic {
                    cosine_similarity: Some(*cosine_similarity as f64),
                }),
                GraphEdgeType::Temporal { delta_secs } => Some(FactorMetadata::Temporal {
                    delta_secs: *delta_secs as i64,
                }),
                GraphEdgeType::Causal { confidence, .. } => Some(FactorMetadata::Causal {
                    confidence: *confidence as f64,
                }),
                GraphEdgeType::Entity { relation } => Some(FactorMetadata::Entity {
                    relation: relation.clone(),
                }),
            };

            if let Some(json) = metadata_json.as_ref() {
                if let Ok(value) = serde_json::from_str::<serde_json::Value>(json) {
                    if let Some(obj) = value.as_object() {
                        match &mut metadata {
                            Some(FactorMetadata::Semantic { cosine_similarity }) => {
                                if let Some(override_cosine) =
                                    obj.get("cosine_similarity").and_then(|v| v.as_f64())
                                {
                                    *cosine_similarity = Some(override_cosine);
                                }
                            }
                            Some(FactorMetadata::Temporal { delta_secs }) => {
                                if let Some(override_delta) = obj.get("delta_secs").and_then(|v| {
                                    v.as_i64().or_else(|| v.as_u64().map(|v| v as i64))
                                }) {
                                    *delta_secs = override_delta;
                                }
                            }
                            Some(FactorMetadata::Causal { confidence }) => {
                                if let Some(override_confidence) =
                                    obj.get("confidence").and_then(|v| v.as_f64())
                                {
                                    *confidence = override_confidence;
                                }
                            }
                            Some(FactorMetadata::Entity { relation }) => {
                                if let Some(override_relation) =
                                    obj.get("relation").and_then(|v| v.as_str())
                                {
                                    *relation = override_relation.to_string();
                                }
                            }
                            None => {}
                        }
                    }
                }
            }

            Factor {
                source: source.clone(),
                target: target.clone(),
                kind,
                edge_weight: *weight,
                metadata,
            }
        })
        .collect()
}

// ─── Integration with confidence-aware quantization ─────────────────────

/// Compute confidence-aware quantization recommendations from factor graph
/// results. This connects the factor graph to the compression governor.
///
/// Items with high refined belief → F32 (full precision).
/// Items with medium belief → SQ8.
/// Items with low belief → SQ4 or SQ4Marked.
pub fn factor_graph_quantization(
    result: &FactorGraphResult,
) -> Vec<crate::integration::ConfidenceQuantizationRecommendation> {
    crate::integration::confidence_aware_quantization(&result.to_message_passing_result())
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_nodes() -> Vec<(String, f64)> {
        vec![
            ("fact:a".to_string(), 0.9),
            ("fact:b".to_string(), 0.8),
            ("fact:c".to_string(), 0.5),
            ("fact:d".to_string(), 0.3),
        ]
    }

    #[test]
    fn factor_graph_propagates_semantic_reinforcement() {
        // Two high-belief nodes connected by a semantic edge should
        // reinforce each other's beliefs.
        let nodes = vec![("fact:a".to_string(), 0.7), ("fact:b".to_string(), 0.7)];
        let factors = vec![Factor {
            source: "fact:a".to_string(),
            target: "fact:b".to_string(),
            kind: FactorKind::Semantic,
            edge_weight: 1.0,
            metadata: Some(FactorMetadata::Semantic {
                cosine_similarity: Some(0.9),
            }),
        }];
        let graph = FactorGraph::new(&nodes, factors, FactorGraphConfig::default());
        let result = graph.propagate();
        // Both nodes should remain high (reinforcing).
        let a_belief = result.node_beliefs["fact:a"];
        let b_belief = result.node_beliefs["fact:b"];
        assert!(
            a_belief > 0.65,
            "fact:a belief should be reinforced: {}",
            a_belief
        );
        assert!(
            b_belief > 0.65,
            "fact:b belief should be reinforced: {}",
            b_belief
        );
    }

    #[test]
    fn factor_graph_propagates_contradiction_pull() {
        // A high-belief node connected to a low-belief node via a semantic
        // edge should be pulled down (disagreement).
        let nodes = vec![
            ("fact:high".to_string(), 0.9),
            ("fact:low".to_string(), 0.2),
        ];
        let factors = vec![Factor {
            source: "fact:high".to_string(),
            target: "fact:low".to_string(),
            kind: FactorKind::Semantic,
            edge_weight: 1.0,
            metadata: Some(FactorMetadata::Semantic {
                cosine_similarity: Some(0.8),
            }),
        }];
        let graph = FactorGraph::new(&nodes, factors, FactorGraphConfig::default());
        let result = graph.propagate();
        // The high node should be pulled down somewhat.
        let high_belief = result.node_beliefs["fact:high"];
        assert!(
            high_belief < 0.9,
            "fact:high should be pulled down from 0.9: {}",
            high_belief
        );
    }

    #[test]
    fn factor_graph_combines_all_edge_types() {
        // A node connected via all 4 edge types to high-belief neighbors
        // should get a compounded boost.
        let nodes = vec![
            ("fact:center".to_string(), 0.5),
            ("fact:sem".to_string(), 0.9),
            ("fact:temp".to_string(), 0.9),
            ("fact:caus".to_string(), 0.9),
            ("fact:ent".to_string(), 0.9),
        ];
        let factors = vec![
            Factor {
                source: "fact:center".to_string(),
                target: "fact:sem".to_string(),
                kind: FactorKind::Semantic,
                edge_weight: 1.0,
                metadata: Some(FactorMetadata::Semantic {
                    cosine_similarity: Some(0.9),
                }),
            },
            Factor {
                source: "fact:center".to_string(),
                target: "fact:temp".to_string(),
                kind: FactorKind::Temporal,
                edge_weight: 1.0,
                metadata: Some(FactorMetadata::Temporal { delta_secs: 3600 }),
            },
            Factor {
                source: "fact:center".to_string(),
                target: "fact:caus".to_string(),
                kind: FactorKind::Causal,
                edge_weight: 1.0,
                metadata: Some(FactorMetadata::Causal { confidence: 0.8 }),
            },
            Factor {
                source: "fact:center".to_string(),
                target: "fact:ent".to_string(),
                kind: FactorKind::Entity,
                edge_weight: 1.0,
                metadata: Some(FactorMetadata::Entity {
                    relation: "depends_on".to_string(),
                }),
            },
        ];
        let graph = FactorGraph::new(&nodes, factors, FactorGraphConfig::default());
        let result = graph.propagate();
        // The center node should be boosted by all 4 reinforcing factors.
        let center_belief = result.node_beliefs["fact:center"];
        assert!(
            center_belief > 0.5,
            "fact:center should be boosted above 0.5 by 4 reinforcing factors: {}",
            center_belief
        );
        assert!(result.factor_counts.total() >= 4);
        assert_eq!(result.factor_counts.semantic, 1);
        assert_eq!(result.factor_counts.temporal, 1);
        assert_eq!(result.factor_counts.causal, 1);
        assert_eq!(result.factor_counts.entity, 1);
    }

    #[test]
    fn factor_graph_converges() {
        let nodes = sample_nodes();
        let factors = vec![
            Factor {
                source: "fact:a".to_string(),
                target: "fact:b".to_string(),
                kind: FactorKind::Entity,
                edge_weight: 1.0,
                metadata: Some(FactorMetadata::Entity {
                    relation: "sibling".to_string(),
                }),
            },
            Factor {
                source: "fact:b".to_string(),
                target: "fact:c".to_string(),
                kind: FactorKind::Causal,
                edge_weight: 0.8,
                metadata: Some(FactorMetadata::Causal { confidence: 0.7 }),
            },
        ];
        let graph = FactorGraph::new(&nodes, factors, FactorGraphConfig::default());
        let result = graph.propagate();
        assert!(result.converged, "should converge within 50 iterations");
        assert!(result.iterations <= 50);
    }

    #[test]
    fn factor_graph_no_factors_keeps_initial() {
        let nodes = vec![("fact:lonely".to_string(), 0.7)];
        let graph = FactorGraph::new(&nodes, vec![], FactorGraphConfig::default());
        let result = graph.propagate();
        let belief = result.node_beliefs["fact:lonely"];
        assert!(
            (belief - 0.7).abs() < 0.001,
            "node with no factors should keep initial belief: {}",
            belief
        );
    }

    #[test]
    fn factor_graph_temporal_decay() {
        // A recent temporal edge should have more influence than an old one.
        let recent_nodes = vec![
            ("fact:a".to_string(), 0.5),
            ("fact:b_recent".to_string(), 0.9),
        ];
        let recent_factors = vec![Factor {
            source: "fact:a".to_string(),
            target: "fact:b_recent".to_string(),
            kind: FactorKind::Temporal,
            edge_weight: 1.0,
            metadata: Some(FactorMetadata::Temporal { delta_secs: 100 }), // 100 seconds ago
        }];
        let recent_graph =
            FactorGraph::new(&recent_nodes, recent_factors, FactorGraphConfig::default());
        let recent_result = recent_graph.propagate();

        let old_nodes = vec![("fact:a".to_string(), 0.5), ("fact:b_old".to_string(), 0.9)];
        let old_factors = vec![Factor {
            source: "fact:a".to_string(),
            target: "fact:b_old".to_string(),
            kind: FactorKind::Temporal,
            edge_weight: 1.0,
            metadata: Some(FactorMetadata::Temporal {
                delta_secs: 31_536_000,
            }), // 1 year ago
        }];
        let old_graph = FactorGraph::new(&old_nodes, old_factors, FactorGraphConfig::default());
        let old_result = old_graph.propagate();

        // Recent temporal edge should boost fact:a more than old one.
        let recent_boost = recent_result.node_beliefs["fact:a"];
        let old_boost = old_result.node_beliefs["fact:a"];
        assert!(
            recent_boost > old_boost,
            "recent temporal edge should boost more than old: recent={}, old={}",
            recent_boost,
            old_boost
        );
    }

    #[test]
    fn factor_graph_causal_confidence_weighting() {
        // A high-confidence causal edge should influence more than low.
        let high_conf_factors = vec![Factor {
            source: "fact:cause".to_string(),
            target: "fact:effect".to_string(),
            kind: FactorKind::Causal,
            edge_weight: 1.0,
            metadata: Some(FactorMetadata::Causal { confidence: 0.9 }),
        }];
        let low_conf_factors = vec![Factor {
            source: "fact:cause".to_string(),
            target: "fact:effect".to_string(),
            kind: FactorKind::Causal,
            edge_weight: 1.0,
            metadata: Some(FactorMetadata::Causal { confidence: 0.3 }),
        }];
        let nodes = vec![
            ("fact:cause".to_string(), 0.9),
            ("fact:effect".to_string(), 0.5),
        ];
        let high_graph = FactorGraph::new(&nodes, high_conf_factors, FactorGraphConfig::default());
        let low_graph = FactorGraph::new(&nodes, low_conf_factors, FactorGraphConfig::default());
        let high_result = high_graph.propagate();
        let low_result = low_graph.propagate();
        // High confidence causal edge should pull effect toward cause more.
        let high_effect = high_result.node_beliefs["fact:effect"];
        let low_effect = low_result.node_beliefs["fact:effect"];
        assert!(
            high_effect > low_effect,
            "high-confidence causal should pull effect toward cause more: high={}, low={}",
            high_effect,
            low_effect
        );
    }

    #[test]
    fn factors_from_edges_parses_metadata() {
        use crate::types::GraphEdgeType;
        let edges = vec![
            (
                "fact:a".to_string(),
                "fact:b".to_string(),
                GraphEdgeType::Temporal { delta_secs: 0 },
                1.0,
                Some(r#"{"delta_secs": 86400}"#.to_string()),
            ),
            (
                "fact:c".to_string(),
                "fact:d".to_string(),
                GraphEdgeType::Causal {
                    confidence: 0.0,
                    evidence_ids: vec![],
                },
                1.0,
                Some(r#"{"confidence": 0.8}"#.to_string()),
            ),
            (
                "fact:e".to_string(),
                "fact:f".to_string(),
                GraphEdgeType::Entity {
                    relation: "depends_on".to_string(),
                },
                1.0,
                Some(r#"{"relation": "depends_on"}"#.to_string()),
            ),
        ];
        let factors = factors_from_edges(&edges);
        assert_eq!(factors.len(), 3);
        assert!(matches!(
            factors[0].metadata,
            Some(FactorMetadata::Temporal { delta_secs: 86400 })
        ));
        assert!(matches!(
            factors[1].metadata,
            Some(FactorMetadata::Causal { confidence: 0.8 })
        ));
        assert!(matches!(
            factors[2].metadata,
            Some(FactorMetadata::Entity { ref relation }) if relation == "depends_on"
        ));
    }

    #[test]
    fn factor_graph_top_k_returns_highest_beliefs() {
        let nodes = vec![
            ("fact:low".to_string(), 0.2),
            ("fact:high".to_string(), 0.9),
            ("fact:mid".to_string(), 0.5),
        ];
        let graph = FactorGraph::new(&nodes, vec![], FactorGraphConfig::default());
        let result = graph.propagate();
        let top2 = result.top_k(2);
        assert_eq!(top2.len(), 2);
        assert_eq!(top2[0].0, "fact:high");
        assert_eq!(top2[1].0, "fact:mid");
    }

    #[test]
    fn factor_graph_to_message_passing_result() {
        let nodes = vec![("fact:a".to_string(), 0.7)];
        let graph = FactorGraph::new(&nodes, vec![], FactorGraphConfig::default());
        let result = graph.propagate();
        let mp = result.to_message_passing_result();
        assert_eq!(mp.node_confidences["fact:a"], result.node_beliefs["fact:a"]);
        assert_eq!(mp.iterations, result.iterations);
        assert_eq!(mp.converged, result.converged);
    }

    #[test]
    fn factor_graph_quantization_integration() {
        // Factor graph results should produce quantization recommendations.
        let nodes = vec![
            ("fact:high".to_string(), 0.9),
            ("fact:low".to_string(), 0.1),
        ];
        let graph = FactorGraph::new(&nodes, vec![], FactorGraphConfig::default());
        let result = graph.propagate();
        let recs = factor_graph_quantization(&result);
        assert_eq!(recs.len(), 2);
        // High belief → F32, low belief → SQ4Marked.
        let high_rec = recs.iter().find(|r| r.item_id == "fact:high").unwrap();
        assert_eq!(high_rec.recommended_level, "F32");
        let low_rec = recs.iter().find(|r| r.item_id == "fact:low").unwrap();
        assert_eq!(low_rec.recommended_level, "SQ4Marked");
    }

    #[test]
    fn factor_graph_normalized_weights_sum_to_one() {
        let config = FactorGraphConfig::default();
        let (s, t, c, e) = config.normalized_weights();
        assert!(
            (s + t + c + e - 1.0).abs() < 0.001,
            "weights should sum to 1.0"
        );
    }

    #[test]
    fn factor_graph_empty_graph_propagates_without_error() {
        let graph = FactorGraph::new(&[], vec![], FactorGraphConfig::default());
        let result = graph.propagate();
        assert!(result.node_beliefs.is_empty());
        assert!(result.converged);
    }
}
