//! RL-trained retrieval routing on receipt replay data.
//!
//! Uses the system's receipt-driven replay as training signal for a
//! simple tabular/linear routing policy. The policy maps query profile
//! features to pipeline stage selection probabilities, learning from
//! past search outcomes.
//!
//! Degrades gracefully to heuristic routing when untrained.
//!
//! Behind `#[cfg(feature = "rl-routing")]` which depends on `routing`.

#![cfg(feature = "rl-routing")]

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::routing::{QueryProfile, RetrievalRouter, RoutingDecision};

// ─── Policy ─────────────────────────────────────────────────────────────

/// A simple tabular routing policy trained on receipt replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingPolicy {
    /// Maps feature name to weight.
    pub weights: HashMap<String, f64>,
    /// Learning rate for policy updates.
    pub learning_rate: f64,
    /// Baseline outcome score (exponential moving average).
    pub baseline: f64,
    /// Number of training examples seen.
    pub trained_examples: usize,
    /// RFC 3339 timestamp of the most recent training update.
    #[serde(default)]
    pub last_updated: Option<String>,
}

impl Default for RoutingPolicy {
    fn default() -> Self {
        let mut weights = HashMap::new();
        weights.insert("bm25_coarse".to_string(), 1.0);
        weights.insert("vector_medium".to_string(), 1.0);
        weights.insert("rerank_fine".to_string(), 0.5);
        weights.insert("graph_expansion".to_string(), 0.3);
        weights.insert("decoder".to_string(), 0.2);
        weights.insert("discord".to_string(), 0.2);
        Self {
            weights,
            learning_rate: 0.01,
            baseline: 0.5,
            trained_examples: 0,
            last_updated: None,
        }
    }
}

// ─── Training example ───────────────────────────────────────────────────

/// A training example extracted from a search receipt.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrainingExample {
    pub query_profile: QueryProfile,
    pub decision: RoutingDecision,
    pub outcome_score: f64,
}

/// Extract a training example from a search outcome.
pub fn extract_training_example(
    profile: &QueryProfile,
    decision: &RoutingDecision,
    outcome_score: f64,
) -> TrainingExample {
    TrainingExample {
        query_profile: profile.clone(),
        decision: decision.clone(),
        outcome_score,
    }
}

// ─── Policy update ──────────────────────────────────────────────────────

/// Update the policy from a batch of training examples.
///
/// For each example:
/// - reward = outcome_score - baseline
/// - If reward > 0, increase weights for stages that were enabled
/// - If reward < 0, decrease weights for stages that were enabled
/// - Update baseline as exponential moving average
pub fn update_policy(policy: &mut RoutingPolicy, examples: &[TrainingExample]) {
    if examples.is_empty() {
        return;
    }

    let lr = policy.learning_rate;
    let alpha = 0.1; // EMA smoothing

    for ex in examples {
        let reward = ex.outcome_score - policy.baseline;

        // Update weights for each stage.
        let stages = [
            ("bm25_coarse", ex.decision.bm25_coarse),
            ("vector_medium", ex.decision.vector_medium),
            ("rerank_fine", ex.decision.rerank_fine),
            ("graph_expansion", ex.decision.graph_expansion),
            ("decoder", ex.decision.decoder),
            ("discord", ex.decision.discord),
        ];

        for (name, enabled) in stages {
            if enabled {
                let w = policy.weights.entry(name.to_string()).or_insert(0.5);
                *w += lr * reward;
                *w = w.clamp(0.0, 2.0);
            }
        }

        // Update baseline.
        policy.baseline = alpha * ex.outcome_score + (1.0 - alpha) * policy.baseline;
        policy.trained_examples += 1;
    }
}

// ─── Routing with RL ────────────────────────────────────────────────────

/// Route a query using learned policy weights.
///
/// Callers should use [`is_trained`] before selecting this production path.
/// Heuristic fallback belongs at the routing dispatch boundary, where the
/// caller's configured [`RetrievalRouter`] is available.
pub fn route_with_policy(policy: &RoutingPolicy, profile: &QueryProfile) -> RoutingDecision {
    // Compute stage scores from learned weights.
    let bm25_w = *policy.weights.get("bm25_coarse").unwrap_or(&1.0);
    let vector_w = *policy.weights.get("vector_medium").unwrap_or(&1.0);
    let rerank_w = *policy.weights.get("rerank_fine").unwrap_or(&0.5);
    let graph_w = *policy.weights.get("graph_expansion").unwrap_or(&0.3);
    let decoder_w = *policy.weights.get("decoder").unwrap_or(&0.2);
    let discord_w = *policy.weights.get("discord").unwrap_or(&0.2);

    // Enable stages with weight > 0.5.
    let bm25_coarse = bm25_w > 0.5;
    let vector_medium = vector_w > 0.5 && profile.specificity >= 0.15;
    let rerank_fine = rerank_w > 0.5;
    let graph_expansion = graph_w > 0.5 && profile.has_entities;
    let decoder = decoder_w > 0.5 && profile.contradiction_risk;
    let discord = discord_w > 0.5 && profile.has_entities;

    let no_retrieval = !bm25_coarse && !vector_medium && profile.token_count < 3;

    RoutingDecision {
        bm25_coarse,
        vector_medium,
        rerank_fine,
        graph_expansion,
        decoder,
        discord,
        no_retrieval,
        reasoning: format!(
            "RL policy (trained={}): bm25_w={:.2}, vec_w={:.2}, rerank_w={:.2}",
            policy.trained_examples, bm25_w, vector_w, rerank_w
        ),
    }
}

/// Backward-compatible RL routing entry point.
///
/// New production dispatchers should explicitly select [`route_with_policy`]
/// only for a trained persisted policy and otherwise use their configured
/// heuristic router.
pub fn route_with_rl(policy: &RoutingPolicy, profile: &QueryProfile) -> RoutingDecision {
    if policy.trained_examples == 0 {
        return RetrievalRouter::default().route(profile);
    }
    route_with_policy(policy, profile)
}

/// Check if the policy has been trained enough to use.
pub fn is_trained(policy: &RoutingPolicy) -> bool {
    policy.trained_examples >= 10
}

// ─── Receipt-driven RL routing feedback ─────────────────────────────────

/// The outcome quality of a routing decision, as reported by the caller
/// after observing search results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RoutingOutcome {
    /// The routing decision produced good results.
    Good,
    /// The routing decision produced poor results.
    Bad,
    /// The routing decision produced acceptable results.
    Neutral,
}

impl RoutingOutcome {
    /// Convert the outcome to a numeric score for policy updates.
    fn to_score(self) -> f64 {
        match self {
            RoutingOutcome::Good => 0.9,
            RoutingOutcome::Neutral => 0.5,
            RoutingOutcome::Bad => 0.1,
        }
    }
}

/// Alias for the tabular routing policy used in receipt-driven RL feedback.
/// This is the same `RoutingPolicy` struct, aliased for clarity in the
/// receipt-driven feedback context.
pub type TabularRoutingPolicy = RoutingPolicy;

/// Record a routing outcome and update the tabular routing policy's Q-table.
///
/// This is the receipt-driven RL feedback loop: after a search is performed
/// using a routing decision, the caller reports whether the outcome was good,
/// bad, or neutral. This function converts the outcome to a score and updates
/// the policy weights accordingly.
///
/// Returns the updated policy.
pub fn record_routing_outcome(
    policy: &mut TabularRoutingPolicy,
    profile: &QueryProfile,
    decision: &RoutingDecision,
    outcome: RoutingOutcome,
) -> TabularRoutingPolicy {
    let score = outcome.to_score();
    let example = extract_training_example(profile, decision, score);
    update_policy(policy, &[example]);
    policy.last_updated = Some(chrono::Utc::now().to_rfc3339());
    policy.clone()
}

// ─── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrained_policy_falls_back_to_heuristic() {
        let policy = RoutingPolicy::default();
        assert_eq!(policy.trained_examples, 0);
        let profile = QueryProfile::from_query("what is rust");
        let decision = route_with_rl(&policy, &profile);
        // Heuristic routing should enable BM25 for a 4-token query.
        assert!(decision.bm25_coarse);
    }

    #[test]
    fn positive_outcome_increases_weights() {
        let mut policy = RoutingPolicy::default();
        let profile = QueryProfile::from_query("compare rust vs python");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: true,
            rerank_fine: true,
            graph_expansion: false,
            decoder: true,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        let example = TrainingExample {
            query_profile: profile,
            decision,
            outcome_score: 0.9, // positive
        };
        let initial_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        update_policy(&mut policy, &[example]);
        let updated_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        assert!(
            updated_bm25_w > initial_bm25_w,
            "positive outcome should increase weight: {} -> {}",
            initial_bm25_w,
            updated_bm25_w
        );
    }

    #[test]
    fn negative_outcome_decreases_weights() {
        let mut policy = RoutingPolicy::default();
        let profile = QueryProfile::from_query("compare rust vs python");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: true,
            rerank_fine: true,
            graph_expansion: false,
            decoder: true,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        let example = TrainingExample {
            query_profile: profile,
            decision,
            outcome_score: 0.1, // negative (below baseline 0.5)
        };
        let initial_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        update_policy(&mut policy, &[example]);
        let updated_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        assert!(
            updated_bm25_w < initial_bm25_w,
            "negative outcome should decrease weight: {} -> {}",
            initial_bm25_w,
            updated_bm25_w
        );
    }

    #[test]
    fn baseline_updates_correctly() {
        let mut policy = RoutingPolicy::default();
        let initial_baseline = policy.baseline;
        let profile = QueryProfile::from_query("test");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: false,
            rerank_fine: false,
            graph_expansion: false,
            decoder: false,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        let example = TrainingExample {
            query_profile: profile,
            decision,
            outcome_score: 0.8,
        };
        update_policy(&mut policy, &[example]);
        // Baseline should move toward 0.8 from 0.5.
        assert!(
            policy.baseline > initial_baseline,
            "baseline should increase: {} -> {}",
            initial_baseline,
            policy.baseline
        );
    }

    #[test]
    fn is_trained_returns_false_for_new_policy() {
        let policy = RoutingPolicy::default();
        assert!(!is_trained(&policy));
    }

    #[test]
    fn is_trained_returns_true_after_11_examples() {
        let mut policy = RoutingPolicy::default();
        let profile = QueryProfile::from_query("test query here");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: true,
            rerank_fine: false,
            graph_expansion: false,
            decoder: false,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        for _ in 0..11 {
            let example = TrainingExample {
                query_profile: profile.clone(),
                decision: decision.clone(),
                outcome_score: 0.7,
            };
            update_policy(&mut policy, &[example]);
        }
        assert_eq!(policy.trained_examples, 11);
        assert!(is_trained(&policy));
    }

    #[test]
    fn trained_policy_produces_different_decisions() {
        let mut policy = RoutingPolicy::default();

        // Train with high outcome for decoder-enabled decisions.
        // Use a high learning rate to ensure weights cross the 0.5 threshold.
        policy.learning_rate = 0.1;
        let profile = QueryProfile::from_query("compare rust vs python");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: true,
            rerank_fine: true,
            graph_expansion: false,
            decoder: true,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        for _ in 0..50 {
            let example = TrainingExample {
                query_profile: profile.clone(),
                decision: decision.clone(),
                outcome_score: 0.9,
            };
            update_policy(&mut policy, &[example]);
        }

        // Now route a similar query — decoder should be enabled.
        let test_profile = QueryProfile::from_query("compare go vs rust differences");
        let rl_decision = route_with_policy(&policy, &test_profile);
        assert!(
            rl_decision.decoder,
            "trained policy should enable decoder for contradiction queries (decoder weight: {})",
            policy.weights.get("decoder").unwrap_or(&0.0)
        );
    }

    #[test]
    fn trained_policy_differs_from_heuristic_routing() {
        let profile = QueryProfile::from_query("compare rust vs python performance");
        let heuristic = RetrievalRouter::default().route(&profile);
        assert!(heuristic.rerank_fine);

        let mut policy = RoutingPolicy::default();
        let decision = heuristic.clone();
        for _ in 0..11 {
            record_routing_outcome(&mut policy, &profile, &decision, RoutingOutcome::Bad);
        }

        assert!(is_trained(&policy));
        let learned = route_with_policy(&policy, &profile);
        assert_ne!(learned.rerank_fine, heuristic.rerank_fine);
        assert!(learned.reasoning.starts_with("RL policy"));
    }

    #[test]
    fn empty_examples_does_nothing() {
        let mut policy = RoutingPolicy::default();
        let initial = policy.clone();
        update_policy(&mut policy, &[]);
        assert_eq!(policy.trained_examples, initial.trained_examples);
        assert!((policy.baseline - initial.baseline).abs() < 0.001);
    }

    #[test]
    fn record_routing_outcome_good_increases_weights() {
        let mut policy = RoutingPolicy::default();
        let profile = QueryProfile::from_query("compare rust vs python");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: true,
            rerank_fine: true,
            graph_expansion: false,
            decoder: true,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        let initial_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        record_routing_outcome(&mut policy, &profile, &decision, RoutingOutcome::Good);
        let updated_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        assert!(
            updated_bm25_w > initial_bm25_w,
            "good outcome should increase weight: {} -> {}",
            initial_bm25_w,
            updated_bm25_w
        );
    }

    #[test]
    fn record_routing_outcome_bad_decreases_weights() {
        let mut policy = RoutingPolicy::default();
        let profile = QueryProfile::from_query("compare rust vs python");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: true,
            rerank_fine: true,
            graph_expansion: false,
            decoder: true,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        let initial_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        record_routing_outcome(&mut policy, &profile, &decision, RoutingOutcome::Bad);
        let updated_bm25_w = *policy.weights.get("bm25_coarse").unwrap();
        assert!(
            updated_bm25_w < initial_bm25_w,
            "bad outcome should decrease weight: {} -> {}",
            initial_bm25_w,
            updated_bm25_w
        );
    }

    #[test]
    fn record_routing_outcome_neutral_does_not_change_much() {
        let mut policy = RoutingPolicy::default();
        let profile = QueryProfile::from_query("test query here");
        let decision = RoutingDecision {
            bm25_coarse: true,
            vector_medium: false,
            rerank_fine: false,
            graph_expansion: false,
            decoder: false,
            discord: false,
            no_retrieval: false,
            reasoning: "test".to_string(),
        };
        let initial_trained = policy.trained_examples;
        record_routing_outcome(&mut policy, &profile, &decision, RoutingOutcome::Neutral);
        assert_eq!(policy.trained_examples, initial_trained + 1);
    }
}
