//! Contextual reinstatement building blocks.
//!
//! Reinstatement scoring measures how well a candidate memory chunk (episode,
//! entity, namespace) matches an active retrieval context. The score is purely
//! additive and bounded in [0, 1] — callers use it as a soft boost signal, not
//! a hard gate.

/// Active context that drives reinstatement-biased retrieval.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReinstatementContext {
    /// Episode IDs that are currently active / in-focus.
    pub episode_ids: Vec<String>,
    /// Entity IDs that are currently in scope.
    pub entity_ids: Vec<String>,
    /// Optional namespace constraint.
    pub namespace: Option<String>,
}

/// Per-candidate reinstatement score with per-dimension match flags.
#[derive(Debug, Clone, PartialEq)]
pub struct ReinstatementScore {
    /// Aggregate score in [0, 1].
    pub score: f32,
    /// True when at least one episode in the context matched the candidate.
    pub matched_episode: bool,
    /// True when at least one entity in the context matched the candidate.
    pub matched_entity: bool,
    /// True when the namespace matched.
    pub matched_namespace: bool,
}

const EPISODE_WEIGHT: f32 = 0.4;
const ENTITY_WEIGHT: f32 = 0.4;
const NAMESPACE_WEIGHT: f32 = 0.2;

/// Compute a reinstatement score for one candidate against the active context.
///
/// - Episode match contributes 0.4
/// - Entity match contributes 0.4
/// - Namespace match contributes 0.2
///
/// The returned `score` is always in [0.0, 1.0].
pub fn compute_reinstatement_score(
    ctx: &ReinstatementContext,
    candidate_episode_ids: &[String],
    candidate_entity_ids: &[String],
    candidate_namespace: Option<&str>,
) -> ReinstatementScore {
    let matched_episode = !ctx.episode_ids.is_empty()
        && candidate_episode_ids
            .iter()
            .any(|id| ctx.episode_ids.contains(id));

    let matched_entity = !ctx.entity_ids.is_empty()
        && candidate_entity_ids
            .iter()
            .any(|id| ctx.entity_ids.contains(id));

    let matched_namespace = match (ctx.namespace.as_deref(), candidate_namespace) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };

    let raw = if matched_episode { EPISODE_WEIGHT } else { 0.0 }
        + if matched_entity { ENTITY_WEIGHT } else { 0.0 }
        + if matched_namespace {
            NAMESPACE_WEIGHT
        } else {
            0.0
        };

    ReinstatementScore {
        score: raw.clamp(0.0, 1.0),
        matched_episode,
        matched_entity,
        matched_namespace,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> String {
        v.to_string()
    }

    #[test]
    fn reinstatement_no_match_is_zero() {
        let ctx = ReinstatementContext {
            episode_ids: vec![s("ep-1")],
            entity_ids: vec![s("ent-1")],
            namespace: Some(s("ns-a")),
        };
        let result = compute_reinstatement_score(&ctx, &[], &[], None);
        assert_eq!(result.score, 0.0);
        assert!(!result.matched_episode);
        assert!(!result.matched_entity);
        assert!(!result.matched_namespace);
    }

    #[test]
    fn reinstatement_full_match_is_one() {
        let ctx = ReinstatementContext {
            episode_ids: vec![s("ep-1")],
            entity_ids: vec![s("ent-1")],
            namespace: Some(s("ns-a")),
        };
        let result = compute_reinstatement_score(&ctx, &[s("ep-1")], &[s("ent-1")], Some("ns-a"));
        assert!((result.score - 1.0).abs() < f32::EPSILON);
        assert!(result.matched_episode);
        assert!(result.matched_entity);
        assert!(result.matched_namespace);
    }

    #[test]
    fn reinstatement_episode_only() {
        let ctx = ReinstatementContext {
            episode_ids: vec![s("ep-2")],
            entity_ids: vec![],
            namespace: None,
        };
        let result = compute_reinstatement_score(&ctx, &[s("ep-2")], &[], None);
        assert!((result.score - 0.4).abs() < 1e-6);
        assert!(result.matched_episode);
        assert!(!result.matched_entity);
        assert!(!result.matched_namespace);
    }

    #[test]
    fn reinstatement_entity_only() {
        let ctx = ReinstatementContext {
            episode_ids: vec![],
            entity_ids: vec![s("ent-x")],
            namespace: None,
        };
        let result = compute_reinstatement_score(&ctx, &[], &[s("ent-x")], None);
        assert!((result.score - 0.4).abs() < 1e-6);
        assert!(!result.matched_episode);
        assert!(result.matched_entity);
    }

    #[test]
    fn reinstatement_namespace_only() {
        let ctx = ReinstatementContext {
            episode_ids: vec![],
            entity_ids: vec![],
            namespace: Some(s("ns-b")),
        };
        let result = compute_reinstatement_score(&ctx, &[], &[], Some("ns-b"));
        assert!((result.score - 0.2).abs() < 1e-6);
        assert!(result.matched_namespace);
    }

    #[test]
    fn reinstatement_score_bounded() {
        let ctx = ReinstatementContext {
            episode_ids: vec![s("ep-1"), s("ep-2")],
            entity_ids: vec![s("ent-1")],
            namespace: Some(s("ns")),
        };
        let result =
            compute_reinstatement_score(&ctx, &[s("ep-1"), s("ep-2")], &[s("ent-1")], Some("ns"));
        assert!(result.score >= 0.0 && result.score <= 1.0);
    }
}
