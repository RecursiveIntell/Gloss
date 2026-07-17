//! Adaptive retrieval routing — query-aware stage selection.
//!
//! Instead of running all pipeline stages in fixed order, the router
//! inspects the query and corpus characteristics and decides which
//! stages to activate. This is the RAGRouter-Bench pattern: benchmark
//! retrieval SELECTION correctness, not just retrieval quality.
//!
//! Behind `#[cfg(feature = "routing")]`.

use serde::{Deserialize, Serialize};

// ─── Query characteristics ───────────────────────────────────────────────

/// Query complexity class for adaptive routing.
/// Based on Adaptive-RAG (arxiv 2406.19387) and RAGRouter-Bench (arxiv 2602.00296).
/// Used to gate expensive tools (discord, graph, decoder, factor_graph) — graph
/// tools hurt simple lookups (GraphRAG-Bench, arxiv 2506.05690).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryComplexityClass {
    /// Single fact, direct entity, definition, keyword match.
    Simple,
    /// Requires traversing relationships between 2+ entities.
    MultiHop,
    /// Asks about conflicting information, comparisons, "is X still true".
    Contradiction,
    /// Aggregating across many sources, themes, or communities.
    Synthesis,
    /// Asks about when things happened, chronological order, current vs stale.
    Temporal,
    /// Open-ended, story/essay/code generation.
    Creative,
}

/// Characteristics of a query that influence routing decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]

pub struct QueryProfile {
    /// Query length in tokens (approximate: word count).
    pub token_count: usize,
    /// Whether the query contains entity-like terms (capitalized words, quoted phrases).
    pub has_entities: bool,
    /// Whether the query is a question (contains ? or question words).
    pub is_question: bool,
    /// Estimated query specificity (0.0 = very broad, 1.0 = very specific).
    pub specificity: f64,
    /// Whether the query likely needs provenance (contains "source", "evidence", "cite").
    pub needs_provenance: bool,
    /// Whether the query likely needs temporal awareness ("recent", "latest", "before", "after").
    pub needs_temporal: bool,
    /// Whether the query likely has contradiction risk ("vs", "compare", "conflict", "contradict").
    pub contradiction_risk: bool,
    /// Whether the query contains relation words ("connects", "between", "depends on", "relates to").
    pub has_relation_words: bool,
    /// Whether the query contains synthesis words ("summarize", "overview", "all about", "themes").
    pub has_synthesis_words: bool,
    /// Classified query complexity class for adaptive routing.
    pub complexity_class: QueryComplexityClass,
    /// Latency budget in milliseconds (0 = no limit).
    pub latency_budget_ms: u64,
    /// Token budget for downstream LLM consumption (0 = no limit).
    pub token_budget: usize,
}

impl QueryProfile {
    /// Profile a raw query string into routing characteristics.
    pub fn from_query(query: &str) -> Self {
        let lower = query.to_lowercase();
        let token_count = query.split_whitespace().count();

        let has_entities = query
            .split_whitespace()
            .any(|w| w.chars().next().map(|c| c.is_uppercase()).unwrap_or(false));

        let question_words = [
            "what", "why", "how", "when", "where", "who", "which", "is", "are", "can", "does",
        ];
        let is_question =
            query.contains('?') || question_words.iter().any(|w| lower.starts_with(w));

        // Specificity heuristic: longer queries with more specific terms = higher specificity.
        let specificity = (token_count as f64 / 20.0).min(1.0);

        let needs_provenance = [
            "source",
            "evidence",
            "cite",
            "citation",
            "reference",
            "proof",
        ]
        .iter()
        .any(|w| lower.contains(w));

        let needs_temporal = [
            "recent",
            "latest",
            "before",
            "after",
            "since",
            "until",
            "current",
            "new",
            "old",
            "updated",
            "when",
            "chronological",
            "timeline",
        ]
        .iter()
        .any(|w| lower.contains(w));

        let contradiction_risk = [
            "vs",
            "compare",
            "conflict",
            "contradict",
            "versus",
            "difference",
            "disagree",
        ]
        .iter()
        .any(|w| lower.contains(w));

        let relation_words = [
            "connects",
            "between",
            "depends on",
            "relates to",
            "relationship",
            "how did",
            "how does",
            "work with",
            "lead to",
            "link",
            "lineage",
            "chain",
            "integrate",
            "integrate with",
        ];
        let has_relation_words = relation_words.iter().any(|w| lower.contains(w));

        let synthesis_words = [
            "summarize",
            "overview",
            "all about",
            "themes",
            "landscape",
            "everything about",
            "synthesis",
            "overall",
        ];
        let has_synthesis_words = synthesis_words.iter().any(|w| lower.contains(w));

        // Classify query complexity (Adaptive-RAG + RAGRouter-Bench validated).
        // Priority: C > B > D > E > A (contradiction takes precedence over multi-hop).
        let complexity_class = if contradiction_risk {
            QueryComplexityClass::Contradiction
        } else if has_relation_words && has_entities {
            QueryComplexityClass::MultiHop
        } else if has_synthesis_words {
            QueryComplexityClass::Synthesis
        } else if needs_temporal {
            QueryComplexityClass::Temporal
        } else {
            // Default: single-fact lookup. (Creative/generative intent is not
            // inferable from the query text alone and is left to the caller.)
            QueryComplexityClass::Simple
        };

        Self {
            token_count,
            has_entities,
            is_question,
            specificity,
            needs_provenance,
            needs_temporal,
            contradiction_risk,
            has_relation_words,
            has_synthesis_words,
            complexity_class,
            latency_budget_ms: 0,
            token_budget: 0,
        }
    }
}

// ─── Routing decision ──────────────────────────────────────────────────

/// Which stages to activate in the pipeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingDecision {
    /// Whether to run BM25 coarse stage.
    pub bm25_coarse: bool,
    /// Whether to run vector similarity stage.
    pub vector_medium: bool,
    /// Whether to run exact rerank stage.
    pub rerank_fine: bool,
    /// Whether to run graph expansion stage.
    pub graph_expansion: bool,
    /// Whether to run decoder (syndrome detection + correction).
    pub decoder: bool,
    /// Whether to run discord (second-order retrieval).
    pub discord: bool,
    /// Whether to skip retrieval entirely (model already knows).
    pub no_retrieval: bool,
    /// Reasoning for the routing decision (human-readable).
    pub reasoning: String,
}

// ─── Router ─────────────────────────────────────────────────────────────

/// Adaptive retrieval router. Inspects query + corpus characteristics
/// and decides which pipeline stages to run.
pub struct RetrievalRouter {
    /// Minimum query length to justify retrieval (below this, model may already know).
    pub min_query_length: usize,
    /// Specificity threshold below which we skip vector search (too broad).
    pub broad_query_threshold: f64,
    /// Whether decoder feature is available.
    pub decoder_enabled: bool,
    /// Whether discord feature is available.
    pub discord_enabled: bool,
    /// Corpus density (0.0 = sparse, 1.0 = dense). Affects graph expansion.
    pub corpus_density: f64,
}

impl Default for RetrievalRouter {
    fn default() -> Self {
        Self {
            min_query_length: 3,
            broad_query_threshold: 0.15,
            decoder_enabled: false,
            discord_enabled: false,
            corpus_density: 0.5,
        }
    }
}

impl RetrievalRouter {
    /// Route a query to a set of pipeline stages.
    pub fn route(&self, profile: &QueryProfile) -> RoutingDecision {
        let mut reasoning_parts = Vec::new();

        // Decision: skip retrieval entirely?
        // If query is very short and doesn't need provenance/temporal, model may know.
        let no_retrieval = profile.token_count < self.min_query_length
            && !profile.needs_provenance
            && !profile.needs_temporal;
        if no_retrieval {
            reasoning_parts.push(format!(
                "query too short ({} tokens < {}), no provenance/temporal need → skip retrieval",
                profile.token_count, self.min_query_length
            ));
            return RoutingDecision {
                bm25_coarse: false,
                vector_medium: false,
                rerank_fine: false,
                graph_expansion: false,
                decoder: false,
                discord: false,
                no_retrieval: true,
                reasoning: reasoning_parts.join("; "),
            };
        }

        // BM25 is always on (cheap, catches keyword matches).
        let bm25_coarse = true;
        reasoning_parts.push("BM25 always on (cheap keyword match)".to_string());

        // Vector search: skip if query is extremely broad (low specificity).
        let vector_medium = profile.specificity >= self.broad_query_threshold;
        if !vector_medium {
            reasoning_parts.push(format!(
                "vector search skipped (specificity {:.2} < {:.2} → too broad)",
                profile.specificity, self.broad_query_threshold
            ));
        } else {
            reasoning_parts.push("vector search on (sufficient specificity)".to_string());
        }

        // Rerank: always on if vector search is on (it's the precision stage).
        let rerank_fine = vector_medium;
        if rerank_fine {
            reasoning_parts.push("rerank on (follows vector search)".to_string());
        }

        // Graph expansion: on if corpus is dense enough and query has entities.
        let graph_expansion = self.corpus_density > 0.3 && profile.has_entities;
        if graph_expansion {
            reasoning_parts.push(format!(
                "graph expansion on (corpus density {:.2} > 0.3, entities detected)",
                self.corpus_density
            ));
        } else {
            reasoning_parts.push("graph expansion off (sparse corpus or no entities)".to_string());
        }

        // Decoder: on if contradiction risk is detected and decoder feature is available.
        let decoder = profile.contradiction_risk && self.decoder_enabled;
        if decoder {
            reasoning_parts.push("decoder on (contradiction risk detected)".to_string());
        } else if profile.contradiction_risk {
            reasoning_parts
                .push("decoder off (contradiction risk but feature not enabled)".to_string());
        }

        // Discord: on for multi-hop and synthesis queries (class B/D), NOT simple lookups.
        // GraphRAG-Bench (arxiv 2506.05690): graph/discord tools hurt simple queries.
        let discord = self.discord_enabled
            && profile.has_entities
            && !matches!(
                profile.complexity_class,
                QueryComplexityClass::Simple | QueryComplexityClass::Creative
            );
        if discord {
            reasoning_parts.push(format!(
                "discord on ({:?} query, entities detected, second-order discovery)",
                profile.complexity_class
            ));
        }

        // Temporal boost: noted in reasoning but doesn't add a stage — it modifies scoring.
        if profile.needs_temporal {
            reasoning_parts.push("temporal boost recommended (temporal query)".to_string());
        }

        // Provenance filter: noted in reasoning.
        if profile.needs_provenance {
            reasoning_parts.push("provenance filter recommended (provenance query)".to_string());
        }

        RoutingDecision {
            bm25_coarse,
            vector_medium,
            rerank_fine,
            graph_expansion,
            decoder,
            discord,
            no_retrieval: false,
            reasoning: reasoning_parts.join("; "),
        }
    }

    /// Route a raw query string directly.
    pub fn route_query(&self, query: &str) -> RoutingDecision {
        let profile = QueryProfile::from_query(query);
        self.route(&profile)
    }
}

// ─── Benchmark metrics ──────────────────────────────────────────────────

/// Metrics for a single routing decision, measured against ground truth.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingMetrics {
    /// Whether the routing decision was correct (matched expected stages).
    pub routing_correct: bool,
    /// Quality of results divided by tokens consumed.
    pub quality_per_token: f64,
    /// Quality of results divided by latency in ms.
    pub quality_per_latency: f64,
    /// Whether retrieval was used when it shouldn't have been.
    pub retrieval_overuse: bool,
    /// Whether retrieval was NOT used when it should have been.
    pub retrieval_underuse: bool,
    /// Whether the routing adapted to contradiction risk.
    pub contradiction_aware: bool,
    /// Whether the routing adapted to provenance requirements.
    pub provenance_aware: bool,
    /// Whether the same query always produces the same routing.
    pub routing_deterministic: bool,
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn route_short_query_skips_retrieval() {
        let router = RetrievalRouter::default();
        let decision = router.route_query("hi");
        assert!(decision.no_retrieval, "short query should skip retrieval");
        assert!(!decision.bm25_coarse);
    }

    #[test]
    fn route_normal_query_uses_bm25_and_vector() {
        let router = RetrievalRouter::default();
        let decision = router.route_query("what is the architecture of semantic memory");
        assert!(decision.bm25_coarse, "normal query should use BM25");
        assert!(
            decision.vector_medium,
            "normal query should use vector search"
        );
        assert!(!decision.no_retrieval);
    }

    #[test]
    fn route_broad_query_skips_vector() {
        let router = RetrievalRouter {
            broad_query_threshold: 0.15,
            ..Default::default()
        };
        // 3 tokens = specificity 0.15 >= 0.15 → just barely passes vector threshold.
        // Use 3 short words to stay broad but avoid no_retrieval (needs >= 3 tokens).
        let decision = router.route_query("a b c");
        assert!(decision.bm25_coarse, "BM25 always on");
        // specificity = 3/20 = 0.15 >= 0.15 → vector IS on (borderline).
        // Use a query with exactly 3 tokens but lower specificity by using common words.
        // Actually 0.15 >= 0.15 is true, so vector_medium will be true.
        // We need specificity < 0.15 with >= 3 tokens. That means < 3 tokens worth.
        // 3 tokens → 0.15 exactly. So use threshold 0.2 instead.
        let router2 = RetrievalRouter {
            broad_query_threshold: 0.2,
            ..Default::default()
        };
        let decision2 = router2.route_query("a b c");
        assert!(decision2.bm25_coarse, "BM25 always on");
        assert!(
            !decision2.vector_medium,
            "broad query should skip vector with threshold 0.2"
        );
    }

    #[test]
    fn route_contradiction_query_enables_decoder() {
        let router = RetrievalRouter {
            decoder_enabled: true,
            ..Default::default()
        };
        let decision = router.route_query("compare rust vs python performance differences");
        assert!(
            decision.decoder,
            "contradiction query should enable decoder"
        );
    }

    #[test]
    fn route_contradiction_query_decoder_disabled_when_feature_off() {
        let router = RetrievalRouter {
            decoder_enabled: false,
            ..Default::default()
        };
        let decision = router.route_query("compare rust vs python performance differences");
        assert!(
            !decision.decoder,
            "decoder should be off when feature not enabled"
        );
    }

    #[test]
    fn route_provenance_query_noted_in_reasoning() {
        let router = RetrievalRouter::default();
        let decision =
            router.route_query("what is the source of the turbo-quant compression algorithm");
        assert!(
            decision.reasoning.contains("provenance"),
            "reasoning should mention provenance"
        );
    }

    #[test]
    fn route_temporal_query_noted_in_reasoning() {
        let router = RetrievalRouter::default();
        let decision = router.route_query("what are the latest developments in vector search");
        assert!(
            decision.reasoning.contains("temporal"),
            "reasoning should mention temporal"
        );
    }

    #[test]
    fn route_entity_query_enables_graph_expansion() {
        let router = RetrievalRouter {
            corpus_density: 0.7,
            ..Default::default()
        };
        let decision = router.route_query("how does Semantic-Memory integrate with Turbo-Quant");
        assert!(
            decision.graph_expansion,
            "entity query with dense corpus should enable graph expansion"
        );
    }

    #[test]
    fn route_entity_query_skips_graph_if_sparse_corpus() {
        let router = RetrievalRouter {
            corpus_density: 0.1, // sparse
            ..Default::default()
        };
        let decision = router.route_query("how does Semantic-Memory integrate with Turbo-Quant");
        assert!(
            !decision.graph_expansion,
            "entity query with sparse corpus should skip graph expansion"
        );
    }

    #[test]
    fn route_discord_enabled_with_entities() {
        let router = RetrievalRouter {
            discord_enabled: true,
            ..Default::default()
        };
        let decision = router.route_query("how does AiDENs work with Recall");
        assert!(
            decision.discord,
            "entity query with discord enabled should activate discord"
        );
    }

    #[test]
    fn route_is_deterministic() {
        let router = RetrievalRouter::default();
        let query = "what is the architecture of semantic memory";
        let d1 = router.route_query(query);
        let d2 = router.route_query(query);
        assert_eq!(d1.bm25_coarse, d2.bm25_coarse);
        assert_eq!(d1.vector_medium, d2.vector_medium);
        assert_eq!(d1.decoder, d2.decoder);
        assert_eq!(d1.no_retrieval, d2.no_retrieval);
    }

    #[test]
    fn route_reasoning_is_populated() {
        let router = RetrievalRouter::default();
        let decision = router.route_query("compare the latest source evidence for rust vs python");
        assert!(
            !decision.reasoning.is_empty(),
            "reasoning should be populated"
        );
        assert!(
            decision.reasoning.contains(";"),
            "reasoning should have multiple parts"
        );
    }

    #[test]
    fn query_profile_detects_characteristics() {
        let profile =
            QueryProfile::from_query("What is the latest source of evidence for Rust vs Python?");
        assert!(profile.is_question);
        assert!(profile.needs_provenance);
        assert!(profile.needs_temporal);
        assert!(profile.contradiction_risk);
        assert!(profile.has_entities);
    }

    #[test]
    fn query_complexity_class_simple() {
        let profile = QueryProfile::from_query("turbo-quant version");
        assert_eq!(profile.complexity_class, QueryComplexityClass::Simple);
    }

    #[test]
    fn query_complexity_class_multi_hop() {
        let profile = QueryProfile::from_query("how does AiDENs work with Recall");
        assert_eq!(profile.complexity_class, QueryComplexityClass::MultiHop);
        assert!(profile.has_relation_words);
    }

    #[test]
    fn query_complexity_class_contradiction() {
        let profile = QueryProfile::from_query("compare rust vs python performance");
        assert_eq!(
            profile.complexity_class,
            QueryComplexityClass::Contradiction
        );
    }

    #[test]
    fn query_complexity_class_synthesis() {
        let profile = QueryProfile::from_query("overview of all RecursiveIntell architecture");
        assert_eq!(profile.complexity_class, QueryComplexityClass::Synthesis);
        assert!(profile.has_synthesis_words);
    }

    #[test]
    fn query_complexity_class_temporal() {
        let profile = QueryProfile::from_query("when was semantic-memory published");
        assert_eq!(profile.complexity_class, QueryComplexityClass::Temporal);
    }

    #[test]
    fn discord_skipped_for_simple_lookup() {
        let router = RetrievalRouter {
            discord_enabled: true,
            corpus_density: 0.5,
            ..Default::default()
        };
        // "turbo-quant version" is class A (Simple) — discord should be OFF
        let decision = router.route_query("turbo-quant version");
        assert!(
            !decision.discord,
            "simple lookup should NOT activate discord"
        );
    }

    #[test]
    fn discord_active_for_multi_hop() {
        let router = RetrievalRouter {
            discord_enabled: true,
            corpus_density: 0.5,
            ..Default::default()
        };
        // "how does AiDENs work with Recall" is class B (MultiHop) — discord should be ON
        let decision = router.route_query("how does AiDENs work with Recall");
        assert!(decision.discord, "multi-hop query should activate discord");
    }
}
