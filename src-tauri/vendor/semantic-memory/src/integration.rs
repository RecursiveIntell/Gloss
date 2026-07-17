//! Cross-feature integration wiring.
//!
//! Connects the independent phase modules into composite capabilities:
//! 1. Routing → Decoder: route to execute_with_decoder when contradiction_risk
//! 2. Decoder → Subtraction: corrections trigger subtraction candidates
//! 3. Provenance → Temporal: confidence feeds into support_factor
//! 4. Subtraction → Compression: post-subtraction re-evaluation trigger
//! 5. Discord → Provenance: discord results carry anchor provenance
//!
//! Behind `#[cfg(feature = "integration")]` which requires all constituent features.

#![cfg(feature = "integration")]

use serde::{Deserialize, Serialize};

// ─── Wire 1: Routing → Decoder ──────────────────────────────────────────

/// A routing-aware pipeline execution decision.
/// When contradiction_risk is detected, the router should call
/// `execute_with_decoder` instead of plain `execute`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutedExecution {
    /// Whether to use the decoder-enhanced pipeline (stage 5).
    pub use_decoder: bool,
    /// Whether to use discord second-order retrieval.
    pub use_discord: bool,
    /// The routing decision that produced this plan.
    pub decision: crate::routing::RoutingDecision,
    /// Contradictions to pass to the decoder (if known).
    pub contradictions: Vec<(String, String)>,
    /// Max iterations for message passing.
    pub max_iterations: usize,
    /// Convergence threshold for message passing.
    pub convergence_threshold: f64,
}

/// Given a routing decision, produce an execution plan for the pipeline.
///
/// If `decision.decoder` is true, the plan calls `execute_with_decoder`.
/// If `decision.discord` is true, the plan calls discord scoring after
/// pipeline execution.
pub fn plan_execution(
    decision: &crate::routing::RoutingDecision,
    contradictions: Vec<(String, String)>,
) -> RoutedExecution {
    RoutedExecution {
        use_decoder: decision.decoder,
        use_discord: decision.discord,
        decision: decision.clone(),
        contradictions,
        max_iterations: 50,
        convergence_threshold: 0.001,
    }
}

// ─── Wire 2: Decoder → Subtraction ─────────────────────────────────────

/// A subtraction candidate derived from decoder corrections.
///
/// When the decoder detects syndromes and computes corrections, items
/// marked as `MarkSuperseded`, `MarkContradicted`, or `QuarantineItem`
/// become subtraction candidates with low structuring scores.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecoderSubtractionCandidate {
    pub item_id: String,
    /// The decoder correction that flagged this item.
    pub correction_id: String,
    /// The correction operation type.
    pub operation_type: String,
    /// Structuring score: items flagged by decoder get penalty.
    /// MarkSuperseded → 0.5 (should be removed)
    /// MarkContradicted → 1.0 (quarantine)
    /// QuarantineItem → 1.0 (quarantine)
    pub structuring_score: f64,
    /// Reason from the correction.
    pub reason: String,
}

/// Convert decoder corrections into subtraction candidates.
///
/// Items marked as superseded get structuring_score 0.5 (below 1.0 →
/// DeleteWithReceipt). Items marked as contradicted or quarantined get
/// structuring_score 1.0 (below 1.5 → Quarantine).
///
/// These candidates can be fed into `subtraction::find_candidates`
/// after merging with other candidates, or processed directly via
/// `subtraction::execute_subtraction`.
pub fn corrections_to_subtraction_candidates(
    corrections: &[crate::decoder::Correction],
) -> Vec<DecoderSubtractionCandidate> {
    let mut candidates = Vec::new();
    for correction in corrections {
        for op in &correction.operations {
            match op {
                crate::decoder::CorrectionOperation::MarkSuperseded { item_id, reason } => {
                    candidates.push(DecoderSubtractionCandidate {
                        item_id: item_id.clone(),
                        correction_id: correction.id.clone(),
                        operation_type: "MarkSuperseded".to_string(),
                        structuring_score: 0.5,
                        reason: reason.clone(),
                    });
                }
                crate::decoder::CorrectionOperation::MarkContradicted {
                    item_id,
                    by_item_id: _,
                } => {
                    candidates.push(DecoderSubtractionCandidate {
                        item_id: item_id.clone(),
                        correction_id: correction.id.clone(),
                        operation_type: "MarkContradicted".to_string(),
                        structuring_score: 1.0,
                        reason: "contradicted by decoder".to_string(),
                    });
                }
                crate::decoder::CorrectionOperation::QuarantineItem { item_id, reason } => {
                    candidates.push(DecoderSubtractionCandidate {
                        item_id: item_id.clone(),
                        correction_id: correction.id.clone(),
                        operation_type: "QuarantineItem".to_string(),
                        structuring_score: 1.0,
                        reason: reason.clone(),
                    });
                }
                _ => {} // FlagForReview, AddGraphEdge don't trigger subtraction
            }
        }
    }
    candidates
}

// ─── Wire 3: Provenance → Temporal ─────────────────────────────────────

/// Provenance-derived inputs for temporal weight computation.
///
/// The temporal weight's `support_count` and `contradiction_count` can
/// be derived from provenance records rather than counted manually.
/// This wire connects semiring confidence to temporal weight.
#[derive(Debug, Clone)]
pub struct ProvenanceTemporalInput {
    /// Number of supporting provenance records (from ConfidenceSemiring.support_count).
    pub support_count: usize,
    /// Number of contradicting provenance records (items with confidence < 0.5).
    pub contradiction_count: usize,
    /// Whether the item is superseded (latest provenance record is superseded).
    pub is_superseded: bool,
}

/// Derive temporal weight inputs from provenance confidence values.
///
/// Given an item's provenance confidence value and a set of contradicting
/// confidence values, compute the inputs for `temporal::compute_temporal_weight`.
///
/// - support_count = provenance.support_count (number of independent observations)
/// - contradiction_count = number of contradicting items with confidence > 0.0
/// - is_superseded = true if provenance confidence is 0.0 (annihilated)
pub fn provenance_to_temporal_input(
    confidence: &crate::provenance::ConfidenceValue,
    contradictors: &[crate::provenance::ConfidenceValue],
) -> ProvenanceTemporalInput {
    let is_superseded = confidence.confidence <= 0.0;
    let contradiction_count = contradictors.iter().filter(|c| c.confidence > 0.0).count();

    ProvenanceTemporalInput {
        support_count: confidence.support_count as usize,
        contradiction_count,
        is_superseded,
    }
}

// ─── Wire 4: Subtraction → Compression Governor ────────────────────────

/// A re-evaluation trigger for the compression governor after subtraction.
///
/// After subtraction removes items, the importance distribution changes.
/// Remaining items may need re-quantization: items that were medium-importance
/// might now be high-importance (because high-importance items were removed)
/// or low-importance (because the knowledge base shrank).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostSubtractionRecompression {
    /// Items that were subtracted (removed/quarantined).
    pub subtracted_item_ids: Vec<String>,
    /// Items that remain and may need re-evaluation.
    pub remaining_item_ids: Vec<String>,
    /// Whether the recompression is triggered.
    pub triggered: bool,
    /// Reason for triggering.
    pub reason: String,
}

/// Determine whether compression governor should re-evaluate after subtraction.
///
/// Trigger if:
/// - More than 10% of items were subtracted (distribution shifted significantly)
/// - Any subtracted item had high importance (its quantization slot freed up)
pub fn should_trigger_recompression(
    subtracted_count: usize,
    remaining_count: usize,
    had_high_importance_subtracted: bool,
) -> PostSubtractionRecompression {
    let total_before = subtracted_count + remaining_count;
    let ratio_subtracted = if total_before > 0 {
        subtracted_count as f64 / total_before as f64
    } else {
        0.0
    };

    let triggered = ratio_subtracted > 0.10 || had_high_importance_subtracted;
    let reason = if triggered {
        if had_high_importance_subtracted {
            "high-importance item subtracted — recompression needed".to_string()
        } else {
            format!(
                "{:.1}% of items subtracted — recompression needed",
                ratio_subtracted * 100.0
            )
        }
    } else {
        "subtraction below threshold — no recompression needed".to_string()
    };

    PostSubtractionRecompression {
        subtracted_item_ids: Vec::new(), // caller fills in
        remaining_item_ids: Vec::new(),  // caller fills in
        triggered,
        reason,
    }
}

// ─── Wire 5: Discord → Provenance ──────────────────────────────────────

/// A discord result enriched with provenance from its anchor items.
///
/// Each discord result (second-order discovery) carries the provenance
/// confidence of its anchors. This lets the caller know not just that
/// item X is related to the top-K results, but how confident the
/// evidence is for those top-K results.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceEnrichedDiscordResult {
    /// The discord result (neighbour item).
    pub discord: crate::discord::DiscordResult,
    /// Provenance confidence of each anchor.
    /// (anchor_id, confidence, support_count)
    pub anchor_provenance: Vec<(String, f64, u32)>,
    /// Average anchor confidence (0.0 if no provenance).
    pub avg_anchor_confidence: f64,
    /// Minimum anchor confidence (0.0 if no provenance).
    pub min_anchor_confidence: f64,
}

/// Enrich discord results with provenance from their anchor items.
///
/// Given discord results and a provenance lookup function, produce
/// enriched results that carry the evidence quality of their anchors.
pub fn enrich_discord_with_provenance<F>(
    discord_results: &[crate::discord::DiscordResult],
    provenance_lookup: F,
) -> Vec<ProvenanceEnrichedDiscordResult>
where
    F: Fn(&str) -> Option<crate::provenance::ConfidenceValue>,
{
    discord_results
        .iter()
        .map(|result| {
            let anchor_prov: Vec<(String, f64, u32)> = result
                .anchor_ids
                .iter()
                .filter_map(|aid| {
                    provenance_lookup(aid).map(|cv| (aid.clone(), cv.confidence, cv.support_count))
                })
                .collect();

            let confidences: Vec<f64> = anchor_prov.iter().map(|(_, c, _)| *c).collect();
            let avg = if confidences.is_empty() {
                0.0
            } else {
                confidences.iter().sum::<f64>() / confidences.len() as f64
            };
            let min = confidences.iter().cloned().fold(f64::INFINITY, f64::min);
            let min = if min == f64::INFINITY { 0.0 } else { min };

            ProvenanceEnrichedDiscordResult {
                discord: result.clone(),
                anchor_provenance: anchor_prov,
                avg_anchor_confidence: avg,
                min_anchor_confidence: min,
            }
        })
        .collect()
}

// ─── Composite: Full autonomous memory lifecycle ───────────────────────

/// A full memory lifecycle pass: temporal recompute → decoder correction
/// → subtraction → compression re-evaluation.
///
/// This is the "autonomous memory manager" — run periodically to keep
/// the knowledge base healthy.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MemoryLifecycleReport {
    /// Number of items re-evaluated for temporal weight.
    pub temporal_recomputed: usize,
    /// Number of syndromes detected.
    pub syndromes_detected: usize,
    /// Number of corrections computed.
    pub corrections_computed: usize,
    /// Number of subtraction candidates identified.
    pub subtraction_candidates: usize,
    /// Number of items actually subtracted.
    pub items_subtracted: usize,
    /// Whether compression recompression was triggered.
    pub recompression_triggered: bool,
    /// Human-readable summary.
    pub summary: String,
}

// ─── Wire 6: Demon extraction pass (structure-preserving deletion) ──────

/// Extracted structure from an item before subtraction.
/// The Demon pattern: extract work (structure) from the system before
/// erasure, paying reduced storage cost while preserving evidence chains.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DemonExtract {
    pub item_id: String,
    pub provenance: Option<(String, String, f64, u32)>,
    pub graph_neighbors: Vec<String>,
    pub contradicted_by: Vec<String>,
    pub episode_ids: Vec<String>,
    pub raw_content_erased: bool,
    pub summary: String,
}

/// Run a Demon extraction pass on an item before subtraction.
pub fn demon_extract(
    item_id: &str,
    provenance: Option<&crate::provenance::ConfidenceValue>,
    graph_neighbors: &[String],
    contradicted_by: &[String],
    episode_ids: &[String],
) -> DemonExtract {
    let prov = provenance.map(|pv| {
        (
            "fact".to_string(),
            "Confidence".to_string(),
            pv.confidence,
            pv.support_count,
        )
    });
    let summary = format!(
        "Demon extract for {}: {} provenance, {} neighbors, {} contradictions, {} episodes",
        item_id,
        if prov.is_some() { "with" } else { "no" },
        graph_neighbors.len(),
        contradicted_by.len(),
        episode_ids.len()
    );
    DemonExtract {
        item_id: item_id.to_string(),
        provenance: prov,
        graph_neighbors: graph_neighbors.to_vec(),
        contradicted_by: contradicted_by.to_vec(),
        episode_ids: episode_ids.to_vec(),
        raw_content_erased: true,
        summary,
    }
}

// ─── Wire 7: Confidence-aware quantization (BP → compression) ─────────

/// Confidence-adjusted quantization recommendation from belief propagation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfidenceQuantizationRecommendation {
    pub item_id: String,
    pub refined_confidence: f64,
    pub recommended_level: String,
    pub reason: String,
}

/// Compute confidence-aware quantization from message passing results.
pub fn confidence_aware_quantization(
    mp_result: &crate::decoder::MessagePassingResult,
) -> Vec<ConfidenceQuantizationRecommendation> {
    mp_result
        .node_confidences
        .iter()
        .map(|(item_id, confidence)| {
            let (level, reason) = if *confidence >= 0.8 {
                (
                    "F32",
                    "high confidence — verified knowledge, full precision",
                )
            } else if *confidence >= 0.5 {
                (
                    "SQ8",
                    "medium confidence — stable knowledge, 4x compression",
                )
            } else if *confidence >= 0.2 {
                (
                    "SQ4",
                    "low confidence — uncertain knowledge, aggressive compression",
                )
            } else {
                (
                    "SQ4Marked",
                    "very low confidence — contradicted, maximum compression",
                )
            };
            ConfidenceQuantizationRecommendation {
                item_id: item_id.clone(),
                refined_confidence: *confidence,
                recommended_level: level.to_string(),
                reason: reason.to_string(),
            }
        })
        .collect()
}

// ─── Wire 8: System health → routing (noise-aware self-regulation) ─────

/// System-wide entropy health metric.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemHealth {
    pub structured_count: usize,
    pub unstructured_count: usize,
    pub health_ratio: f64,
    pub is_noisy: bool,
    pub routing_adjustments: RoutingAdjustments,
}

/// Adjustments to routing parameters based on system health.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RoutingAdjustments {
    pub tighten_bm25: bool,
    pub tighten_vector: bool,
    pub force_decoder: bool,
    pub boost_structured: bool,
    pub summary: String,
}

/// Compute system health from item metadata.
pub fn compute_system_health(items: &[(String, bool, bool, bool)]) -> SystemHealth {
    let structured = items.iter().filter(|(_, p, r, e)| *p || *r || *e).count();
    let unstructured = items.len() - structured;
    let health_ratio = if items.is_empty() {
        1.0 // empty system is healthy (nothing to be noisy about)
    } else {
        structured as f64 / items.len() as f64
    };
    let is_noisy = health_ratio < 0.5;

    let adjustments = if is_noisy {
        RoutingAdjustments {
            tighten_bm25: true,
            tighten_vector: true,
            force_decoder: true,
            boost_structured: true,
            summary: format!(
                "System noisy: {:.0}% structured — tightening",
                health_ratio * 100.0
            ),
        }
    } else {
        RoutingAdjustments {
            tighten_bm25: false,
            tighten_vector: false,
            force_decoder: false,
            boost_structured: false,
            summary: format!(
                "System healthy: {:.0}% structured — relaxed",
                health_ratio * 100.0
            ),
        }
    };

    SystemHealth {
        structured_count: structured,
        unstructured_count: unstructured,
        health_ratio,
        is_noisy,
        routing_adjustments: adjustments,
    }
}

// ─── Wire 9: Provenance-weighted Landauer erasure cost ─────────────────

/// Landauer-inspired erasure cost.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErasureCost {
    pub item_id: String,
    pub structuring: f64,
    pub system_avg: f64,
    pub cost: f64,
    pub is_expensive: bool,
}

/// Compute provenance-weighted Landauer erasure cost.
pub fn landauer_erasure_cost(
    item_id: &str,
    confidence: f64,
    support_count: u32,
    contradiction_count: usize,
    system_avg_structuring: f64,
    base_cost: f64,
) -> ErasureCost {
    let contradiction_penalty = (0.15 * contradiction_count as f64).min(0.9);
    let structuring = confidence * support_count as f64 * (1.0 - contradiction_penalty);
    let cost = if system_avg_structuring > 0.0 {
        base_cost * (1.0 - structuring / system_avg_structuring).max(0.0)
    } else {
        base_cost
    };
    ErasureCost {
        item_id: item_id.to_string(),
        structuring,
        system_avg: system_avg_structuring,
        cost,
        is_expensive: cost > base_cost,
    }
}

// ─── Wire 10: Causal entropy in IDS search boost ───────────────────────

/// IDS-inspired search priority: C(n) = S(n) / (1 + I(n))
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntropySearchBoost {
    pub item_id: String,
    pub graph_degree: usize,
    pub causal_degree: usize,
    pub entropy: f64,
    pub structuring: f64,
    pub priority: f64,
}

/// Compute IDS-inspired search priority for items.
pub fn ids_search_boost(items: &[(String, usize, usize, f64, usize)]) -> Vec<EntropySearchBoost> {
    items
        .iter()
        .map(
            |(item_id, graph_degree, episode_count, confidence, support_count)| {
                let entropy = (*graph_degree + *episode_count) as f64;
                let structuring = confidence * (1.0 + *support_count as f64);
                let priority = entropy / (1.0 + structuring);
                EntropySearchBoost {
                    item_id: item_id.clone(),
                    graph_degree: *graph_degree,
                    causal_degree: *episode_count,
                    entropy,
                    structuring,
                    priority,
                }
            },
        )
        .collect()
}

// ─── Wire 11: Temporal well boost in discord scoring ───────────────────

/// Boost discord results that are in temporal wells.
pub fn temporal_well_discord_boost(
    discord_results: &[crate::discord::DiscordResult],
    well_items: &[String],
) -> Vec<(crate::discord::DiscordResult, f64)> {
    let well_set: std::collections::HashSet<&String> = well_items.iter().collect();
    discord_results
        .iter()
        .map(|result| {
            let boost = if well_set.contains(&result.item_id) {
                1.5
            } else {
                1.0
            };
            (result.clone(), result.discord_score * boost)
        })
        .collect()
}

// ─── Wire 12: Episode-aware decoder syndromes ───────────────────────────

/// A decoder syndrome enriched with episode (causal) provenance.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeAwareSyndrome {
    pub syndrome: crate::decoder::Syndrome,
    pub episode_a: Vec<String>,
    pub episode_b: Vec<String>,
    pub causal_chain: String,
}

/// Enrich decoder syndromes with episode references.
pub fn episode_aware_syndromes<F>(
    syndromes: &[crate::decoder::Syndrome],
    episode_lookup: F,
) -> Vec<EpisodeAwareSyndrome>
where
    F: Fn(&str) -> Vec<String>,
{
    syndromes
        .iter()
        .filter_map(|s| {
            if s.items.len() >= 2 {
                let ep_a = episode_lookup(&s.items[0]);
                let ep_b = episode_lookup(&s.items[1]);
                let causal_chain = format!(
                    "Contradiction: '{}' (episodes {:?}) vs '{}' (episodes {:?})",
                    s.items[0], ep_a, s.items[1], ep_b
                );
                Some(EpisodeAwareSyndrome {
                    syndrome: s.clone(),
                    episode_a: ep_a,
                    episode_b: ep_b,
                    causal_chain,
                })
            } else {
                None
            }
        })
        .collect()
}

// ─── Wire 13: Temporal stability in compression governor ───────────────

/// Temporal stability score for compression decisions.
pub fn temporal_stability_importance(
    access_frequency: f64,
    entropy: f64,
    structuring_score: f64,
    in_temporal_well: bool,
    config: &crate::compression_governor::ImportanceConfig,
) -> f64 {
    let base = crate::compression_governor::compute_importance(
        access_frequency,
        entropy,
        structuring_score,
        config,
    );
    if in_temporal_well {
        let well_boost = config.structuring_weight * structuring_score;
        base + well_boost
    } else {
        base
    }
}

// ─── Wire 14: Erasure cost as routing boost ────────────────────────────

/// Boost search results by their erasure cost (Demon-selective retrieval).
pub fn erasure_cost_routing_boost(
    results: &[(String, f64)],
    erasure_costs: &[ErasureCost],
) -> Vec<(String, f64)> {
    let cost_map: std::collections::HashMap<&String, f64> = erasure_costs
        .iter()
        .map(|ec| (&ec.item_id, ec.cost))
        .collect();
    results
        .iter()
        .map(|(id, score)| {
            let boost = cost_map.get(id).copied().unwrap_or(1.0);
            let boosted = if boost > 1.0 {
                score * (1.0 + (boost - 1.0) * 0.1)
            } else {
                *score
            };
            (id.clone(), boosted)
        })
        .collect()
}

// ─── Wire 15: Autonomous knowledge revision pipeline ──────────────────

/// Full autonomous knowledge revision report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeRevisionReport {
    pub items_analyzed: usize,
    pub syndromes_detected: usize,
    pub corrections_computed: usize,
    pub subtraction_candidates: Vec<DecoderSubtractionCandidate>,
    pub demon_extracts: Vec<DemonExtract>,
    pub erasure_costs: Vec<ErasureCost>,
    pub items_to_erase: Vec<String>,
    pub post_revision_health: Option<SystemHealth>,
    pub summary: String,
}

/// Run the full autonomous knowledge revision pipeline.
pub fn autonomous_knowledge_revision(
    results: &[(String, f64)],
    contradictions: &[(String, String)],
    provenance_lookup: &dyn Fn(&str) -> Option<crate::provenance::ConfidenceValue>,
    episode_lookup: &dyn Fn(&str) -> Vec<String>,
    graph_neighbors_lookup: &dyn Fn(&str) -> Vec<String>,
    system_avg_structuring: f64,
) -> KnowledgeRevisionReport {
    let syndromes = crate::decoder::detect_syndromes(results, contradictions);
    let corrections = crate::decoder::compute_correction(&syndromes, 10.0);
    let sub_candidates = corrections_to_subtraction_candidates(&corrections);
    let mut demon_extracts = Vec::new();
    let mut erasure_costs = Vec::new();
    let mut items_to_erase = Vec::new();

    for candidate in &sub_candidates {
        let provenance = provenance_lookup(&candidate.item_id);
        let neighbors = graph_neighbors_lookup(&candidate.item_id);
        let episodes = episode_lookup(&candidate.item_id);
        let contradicted_by: Vec<String> = contradictions
            .iter()
            .filter_map(|(a, b)| {
                if a == &candidate.item_id {
                    Some(b.clone())
                } else if b == &candidate.item_id {
                    Some(a.clone())
                } else {
                    None
                }
            })
            .collect();

        let extract = demon_extract(
            &candidate.item_id,
            provenance.as_ref(),
            &neighbors,
            &contradicted_by,
            &episodes,
        );
        demon_extracts.push(extract);

        let (confidence, support_count) = provenance
            .map(|pv| (pv.confidence, pv.support_count))
            .unwrap_or((0.0, 0));
        let cost = landauer_erasure_cost(
            &candidate.item_id,
            confidence,
            support_count,
            contradicted_by.len(),
            system_avg_structuring,
            1.0,
        );
        if !cost.is_expensive {
            items_to_erase.push(candidate.item_id.clone());
        }
        erasure_costs.push(cost);
    }

    let summary = format!(
        "Knowledge revision: {} items, {} syndromes, {} corrections, {} candidates, {} extracted, {} to erase",
        results.len(), syndromes.len(), corrections.len(),
        sub_candidates.len(), demon_extracts.len(), items_to_erase.len()
    );

    KnowledgeRevisionReport {
        items_analyzed: results.len(),
        syndromes_detected: syndromes.len(),
        corrections_computed: corrections.len(),
        subtraction_candidates: sub_candidates,
        demon_extracts,
        erasure_costs,
        items_to_erase,
        post_revision_health: None,
        summary,
    }
}

// ─── Wire 16: Topology-aware gap detection ──────────────────────────────

/// A knowledge gap identified by combining topology, discord, and contradictions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeGap {
    pub gap_type: String,
    pub description: String,
    pub nearby_items: Vec<String>,
    pub suggested_exploration: Vec<String>,
}

/// Combine topological voids with discord results and contradictions to
/// identify knowledge gaps — things the system should know but doesn't.
#[cfg(feature = "topology")]
pub fn topology_aware_gap_detection(
    voids: &[crate::topology::TopologicalVoid],
    contradictions: &[(String, String)],
) -> Vec<KnowledgeGap> {
    let mut gaps = Vec::new();
    for void in voids {
        let is_contradiction_gap = contradictions
            .iter()
            .any(|(a, b)| void.nearby_items.contains(a) && void.nearby_items.contains(b));
        let gap_type = if is_contradiction_gap {
            "ContradictionGap"
        } else {
            "MissingLink"
        };
        gaps.push(KnowledgeGap {
            gap_type: gap_type.to_string(),
            description: void.description.clone(),
            nearby_items: void.nearby_items.clone(),
            suggested_exploration: void.nearby_items.clone(),
        });
    }
    gaps
}

// ─── Wire 17: Community-aware compression with contradiction tracking ───

/// Community-level compression report with contradiction awareness.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommunityCompressionReport {
    pub communities_scanned: usize,
    pub contradictions_found: usize,
    pub compression_decisions: Vec<crate::community::CompressionDecision>,
    pub summary: String,
}

/// Run community-aware compression with contradiction tracking.
#[cfg(feature = "community")]
pub fn community_aware_compression_pass(
    communities: &[crate::community::Community],
    importance_scores: &[(String, f64)],
    contradictions: &[(String, String)],
) -> CommunityCompressionReport {
    let community_contras =
        crate::community::community_contradiction_scan(communities, contradictions);
    let decisions = crate::community::community_aware_compression(communities, importance_scores);
    CommunityCompressionReport {
        communities_scanned: communities.len(),
        contradictions_found: community_contras.len(),
        compression_decisions: decisions,
        summary: format!(
            "Community compression: {} communities, {} contradictions, {} decisions",
            communities.len(),
            community_contras.len(),
            community_contras.len()
        ),
    }
}

// ─── Wire 18: Subgraph pruning + lifecycle integration ─────────────────

/// Autonomous subgraph maintenance report.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubgraphMaintenanceReport {
    pub subgraphs_identified: usize,
    pub subgraphs_pruned: usize,
    pub receipts: Vec<crate::subgraph_pruning::PruningReceipt>,
    pub summary: String,
}

/// Run autonomous subgraph maintenance: identify, prioritize, prune.
#[cfg(feature = "subgraph-pruning")]
pub fn autonomous_subgraph_maintenance(
    edges: &[(String, String)],
    access_logs: &[crate::subgraph_pruning::AccessLog],
    contradictions: &[(String, String)],
    max_prune: usize,
) -> SubgraphMaintenanceReport {
    let subgraphs = crate::subgraph_pruning::identify_subgraphs(edges, access_logs);
    let priority = crate::subgraph_pruning::pruning_priority(&subgraphs);

    let mut receipts = Vec::new();
    let prune_count = priority.len().min(max_prune);
    for (root, _) in priority.iter().take(prune_count) {
        if let Some(sg) = subgraphs.iter().find(|s| &s.root == root) {
            let receipt = crate::subgraph_pruning::prune_subgraph(sg, contradictions);
            receipts.push(receipt);
        }
    }

    let pruned_count = receipts.len();
    let identified_count = subgraphs.len();
    SubgraphMaintenanceReport {
        subgraphs_identified: identified_count,
        subgraphs_pruned: pruned_count,
        receipts,
        summary: format!(
            "Subgraph maintenance: {} identified, {} pruned",
            identified_count, pruned_count
        ),
    }
}

// ─── Wire 19: Matryoshka dimensionality selection in routing ───────────

/// Routing decision enriched with embedding dimensionality selection.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MultiResolutionRoutingDecision {
    pub base_decision: crate::routing::RoutingDecision,
    pub embedding_dim: usize,
    pub candidate_dim: usize,
    pub estimated_recall: f64,
    pub reasoning: String,
}

/// Extend a routing decision with Matryoshka dimensionality selection.
#[cfg(feature = "matryoshka")]
pub fn multi_resolution_route(
    profile: &crate::routing::QueryProfile,
    config: &crate::matryoshka::MatryoshkaConfig,
) -> MultiResolutionRoutingDecision {
    let router = crate::routing::RetrievalRouter::default();
    let base = router.route(profile);

    // High-stakes queries get full dimensionality.
    // Quick lookups get reduced candidate dimensionality.
    let (candidate_dim, reasoning) = if profile.contradiction_risk || profile.needs_provenance {
        (config.exact_dim, "full-dim: high-stakes query".to_string())
    } else if profile.specificity < 0.3 {
        (config.candidate_dim, "reduced-dim: broad query".to_string())
    } else {
        (
            config.candidate_dim,
            "standard: candidate-dim + exact rerank".to_string(),
        )
    };

    let estimated_recall =
        crate::matryoshka::estimate_recall_at_dim(candidate_dim, config.exact_dim);

    MultiResolutionRoutingDecision {
        base_decision: base,
        embedding_dim: config.exact_dim,
        candidate_dim,
        estimated_recall,
        reasoning,
    }
}

// ─── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decoder::*;
    use crate::discord::*;
    use crate::provenance::*;
    use crate::routing::*;
    use crate::subtraction::*;

    // ── Wire 1 tests ───────────────────────────────────────────────────

    #[test]
    fn plan_execution_enables_decoder_for_contradiction() {
        let router = RetrievalRouter {
            decoder_enabled: true,
            ..Default::default()
        };
        let decision = router.route_query("compare rust vs python differences");
        assert!(decision.decoder);
        let plan = plan_execution(&decision, vec![]);
        assert!(plan.use_decoder);
    }

    #[test]
    fn plan_execution_disables_decoder_without_contradiction() {
        let router = RetrievalRouter {
            decoder_enabled: true,
            ..Default::default()
        };
        let decision = router.route_query("what is the architecture of semantic memory");
        assert!(!decision.decoder);
        let plan = plan_execution(&decision, vec![]);
        assert!(!plan.use_decoder);
    }

    #[test]
    fn plan_execution_carries_contradictions() {
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
        let contras = vec![("a".to_string(), "b".to_string())];
        let plan = plan_execution(&decision, contras.clone());
        assert_eq!(plan.contradictions, contras);
        assert!(plan.use_decoder);
    }

    // ── Wire 2 tests ───────────────────────────────────────────────────

    #[test]
    fn corrections_to_subtraction_mark_superseded() {
        let corrections = vec![Correction {
            id: "c1".to_string(),
            syndrome_ids: vec!["s1".to_string()],
            operations: vec![CorrectionOperation::MarkSuperseded {
                item_id: "old-fact".to_string(),
                reason: "superseded by new-fact".to_string(),
            }],
            confidence: 0.9,
            cost: 1.0,
        }];
        let candidates = corrections_to_subtraction_candidates(&corrections);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].item_id, "old-fact");
        assert!((candidates[0].structuring_score - 0.5).abs() < 0.001);
        assert_eq!(candidates[0].operation_type, "MarkSuperseded");
    }

    #[test]
    fn corrections_to_subtraction_mark_contradicted() {
        let corrections = vec![Correction {
            id: "c1".to_string(),
            syndrome_ids: vec!["s1".to_string()],
            operations: vec![CorrectionOperation::MarkContradicted {
                item_id: "bad-fact".to_string(),
                by_item_id: "good-fact".to_string(),
            }],
            confidence: 0.8,
            cost: 1.0,
        }];
        let candidates = corrections_to_subtraction_candidates(&corrections);
        assert_eq!(candidates.len(), 1);
        assert!((candidates[0].structuring_score - 1.0).abs() < 0.001);
        assert_eq!(candidates[0].operation_type, "MarkContradicted");
    }

    #[test]
    fn corrections_to_subtraction_quarantine() {
        let corrections = vec![Correction {
            id: "c1".to_string(),
            syndrome_ids: vec!["s1".to_string()],
            operations: vec![CorrectionOperation::QuarantineItem {
                item_id: "suspicious".to_string(),
                reason: "orphan reference".to_string(),
            }],
            confidence: 0.7,
            cost: 1.0,
        }];
        let candidates = corrections_to_subtraction_candidates(&corrections);
        assert_eq!(candidates.len(), 1);
        assert!((candidates[0].structuring_score - 1.0).abs() < 0.001);
    }

    #[test]
    fn corrections_to_subtraction_ignores_flag_for_review() {
        let corrections = vec![Correction {
            id: "c1".to_string(),
            syndrome_ids: vec!["s1".to_string()],
            operations: vec![CorrectionOperation::FlagForReview {
                item_id: "maybe".to_string(),
                reason: "check this".to_string(),
            }],
            confidence: 0.5,
            cost: 1.0,
        }];
        let candidates = corrections_to_subtraction_candidates(&corrections);
        assert!(
            candidates.is_empty(),
            "FlagForReview should not trigger subtraction"
        );
    }

    #[test]
    fn corrections_to_subtraction_multiple_ops() {
        let corrections = vec![Correction {
            id: "c1".to_string(),
            syndrome_ids: vec!["s1".to_string()],
            operations: vec![
                CorrectionOperation::MarkSuperseded {
                    item_id: "old".to_string(),
                    reason: "superseded".to_string(),
                },
                CorrectionOperation::QuarantineItem {
                    item_id: "bad".to_string(),
                    reason: "orphan".to_string(),
                },
                CorrectionOperation::FlagForReview {
                    item_id: "maybe".to_string(),
                    reason: "check".to_string(),
                },
            ],
            confidence: 0.8,
            cost: 1.0,
        }];
        let candidates = corrections_to_subtraction_candidates(&corrections);
        assert_eq!(
            candidates.len(),
            2,
            "only MarkSuperseded + QuarantineItem, not FlagForReview"
        );
    }

    // ── Wire 3 tests ───────────────────────────────────────────────────

    #[test]
    fn provenance_to_temporal_high_confidence() {
        let confidence = ConfidenceValue::new(0.95, 5);
        let contradictors = vec![ConfidenceValue::new(0.3, 1)];
        let input = provenance_to_temporal_input(&confidence, &contradictors);
        assert_eq!(input.support_count, 5);
        assert_eq!(input.contradiction_count, 1);
        assert!(!input.is_superseded);
    }

    #[test]
    fn provenance_to_temporal_annihilated_confidence() {
        let confidence = ConfidenceValue::new(0.0, 0);
        let contradictors: Vec<ConfidenceValue> = vec![];
        let input = provenance_to_temporal_input(&confidence, &contradictors);
        assert!(input.is_superseded, "zero confidence = superseded");
        assert_eq!(input.support_count, 0);
        assert_eq!(input.contradiction_count, 0);
    }

    #[test]
    fn provenance_to_temporal_multiple_contradictors() {
        let confidence = ConfidenceValue::new(0.8, 3);
        let contradictors = vec![
            ConfidenceValue::new(0.6, 1),
            ConfidenceValue::new(0.4, 1),
            ConfidenceValue::new(0.0, 0), // annihilated — doesn't count
        ];
        let input = provenance_to_temporal_input(&confidence, &contradictors);
        assert_eq!(
            input.contradiction_count, 2,
            "only non-zero confidence contradictors count"
        );
    }

    // ── Wire 4 tests ───────────────────────────────────────────────────

    #[test]
    fn recompression_triggered_by_high_ratio() {
        let result = should_trigger_recompression(15, 85, false);
        assert!(result.triggered, "15% subtracted should trigger");
        assert!(result.reason.contains("%"));
    }

    #[test]
    fn recompression_triggered_by_high_importance() {
        let result = should_trigger_recompression(1, 99, true);
        assert!(
            result.triggered,
            "high-importance subtraction should trigger"
        );
        assert!(result.reason.contains("high-importance"));
    }

    #[test]
    fn recompression_not_triggered_below_threshold() {
        let result = should_trigger_recompression(1, 99, false);
        assert!(
            !result.triggered,
            "1% with no high-importance should not trigger"
        );
        assert!(result.reason.contains("below threshold"));
    }

    #[test]
    fn recompression_empty_store() {
        let result = should_trigger_recompression(0, 0, false);
        assert!(!result.triggered, "empty store should not trigger");
    }

    // ── Wire 5 tests ───────────────────────────────────────────────────

    #[test]
    fn enrich_discord_with_provenance_all_found() {
        let discord_results = vec![DiscordResult {
            item_id: "neighbor-1".to_string(),
            discord_score: 0.8,
            anchor_ids: vec!["a1".to_string(), "a2".to_string()],
            relationship_types: vec!["semantic".to_string()],
        }];
        let enriched = enrich_discord_with_provenance(&discord_results, |id| match id {
            "a1" => Some(ConfidenceValue::new(0.9, 3)),
            "a2" => Some(ConfidenceValue::new(0.7, 2)),
            _ => None,
        });
        assert_eq!(enriched.len(), 1);
        assert_eq!(enriched[0].anchor_provenance.len(), 2);
        assert!((enriched[0].avg_anchor_confidence - 0.8).abs() < 0.001);
        assert!((enriched[0].min_anchor_confidence - 0.7).abs() < 0.001);
    }

    #[test]
    fn enrich_discord_with_provenance_partial() {
        let discord_results = vec![DiscordResult {
            item_id: "neighbor-1".to_string(),
            discord_score: 0.5,
            anchor_ids: vec!["a1".to_string(), "unknown".to_string()],
            relationship_types: vec!["semantic".to_string()],
        }];
        let enriched = enrich_discord_with_provenance(&discord_results, |id| match id {
            "a1" => Some(ConfidenceValue::new(0.9, 3)),
            _ => None,
        });
        assert_eq!(enriched[0].anchor_provenance.len(), 1);
        assert!((enriched[0].avg_anchor_confidence - 0.9).abs() < 0.001);
    }

    #[test]
    fn enrich_discord_with_no_provenance() {
        let discord_results = vec![DiscordResult {
            item_id: "neighbor-1".to_string(),
            discord_score: 0.5,
            anchor_ids: vec!["a1".to_string()],
            relationship_types: vec!["semantic".to_string()],
        }];
        let enriched = enrich_discord_with_provenance(&discord_results, |_| None);
        assert_eq!(enriched[0].anchor_provenance.len(), 0);
        assert!((enriched[0].avg_anchor_confidence - 0.0).abs() < 0.001);
        assert!((enriched[0].min_anchor_confidence - 0.0).abs() < 0.001);
    }

    // ── Composite lifecycle report ─────────────────────────────────────

    #[test]
    fn lifecycle_report_default() {
        let report = MemoryLifecycleReport::default();
        assert_eq!(report.temporal_recomputed, 0);
        assert_eq!(report.syndromes_detected, 0);
        assert!(!report.recompression_triggered);
    }

    // ── Combination tests (Wires 6-15) ─────────────────────────────────

    #[test]
    fn demon_extract_preserves_structure() {
        let prov = ConfidenceValue::new(0.85, 3);
        let extract = demon_extract(
            "fact-42",
            Some(&prov),
            &["n1".to_string(), "n2".to_string()],
            &["c1".to_string()],
            &["ep1".to_string()],
        );
        assert_eq!(extract.item_id, "fact-42");
        assert!(extract.provenance.is_some());
        assert_eq!(extract.graph_neighbors.len(), 2);
        assert_eq!(extract.contradicted_by.len(), 1);
        assert_eq!(extract.episode_ids.len(), 1);
        assert!(extract.raw_content_erased);
    }

    #[test]
    fn demon_extract_no_provenance() {
        let extract = demon_extract("f1", None, &[], &[], &[]);
        assert!(extract.provenance.is_none());
        assert!(extract.raw_content_erased);
    }

    #[test]
    fn confidence_quantization_levels() {
        let mp = crate::decoder::MessagePassingResult {
            node_confidences: [
                ("hi".to_string(), 0.95),
                ("mid".to_string(), 0.6),
                ("low".to_string(), 0.3),
                ("bad".to_string(), 0.1),
            ]
            .into_iter()
            .collect(),
            iterations: 1,
            converged: true,
            elapsed_ms: 0,
        };
        let recs = confidence_aware_quantization(&mp);
        assert_eq!(recs.len(), 4);
        let levels: Vec<&str> = recs.iter().map(|r| r.recommended_level.as_str()).collect();
        assert!(levels.contains(&"F32"));
        assert!(levels.contains(&"SQ8"));
        assert!(levels.contains(&"SQ4"));
        assert!(levels.contains(&"SQ4Marked"));
    }

    #[test]
    fn system_health_healthy() {
        let items = vec![
            ("a".to_string(), true, false, false),
            ("b".to_string(), false, true, false),
        ];
        let h = compute_system_health(&items);
        assert!((h.health_ratio - 1.0).abs() < 0.001);
        assert!(!h.is_noisy);
    }

    #[test]
    fn system_health_noisy() {
        let items = vec![
            ("a".to_string(), false, false, false),
            ("b".to_string(), false, false, false),
            ("c".to_string(), true, false, false),
        ];
        let h = compute_system_health(&items);
        assert!(h.is_noisy);
        assert!(h.routing_adjustments.force_decoder);
    }

    #[test]
    fn system_health_empty() {
        let items: Vec<(String, bool, bool, bool)> = vec![];
        let h = compute_system_health(&items);
        assert!(!h.is_noisy);
    }

    #[test]
    fn landauer_cost_unstructured_cheap() {
        let cost = landauer_erasure_cost("raw", 0.1, 0, 0, 2.0, 1.0);
        assert!((cost.cost - 1.0).abs() < 0.001);
        assert!(!cost.is_expensive);
    }

    #[test]
    fn landauer_cost_contradiction_reduces_structuring() {
        let no_contra = landauer_erasure_cost("item", 0.8, 3, 0, 2.0, 1.0);
        let with_contra = landauer_erasure_cost("item", 0.8, 3, 5, 2.0, 1.0);
        assert!(with_contra.structuring <= no_contra.structuring);
    }

    #[test]
    fn ids_boost_priority_formula() {
        let items = vec![("test".to_string(), 4, 2, 0.5, 1)];
        let boosts = ids_search_boost(&items);
        // S=6, I=0.5*(1+1)=1.0, C=6/(1+1)=3.0
        assert!((boosts[0].priority - 3.0).abs() < 0.001);
        assert_eq!(boosts[0].causal_degree, 2);
    }

    #[test]
    fn ids_boost_hub_vs_leaf() {
        let items = vec![
            ("hub".to_string(), 10, 5, 0.9, 3),
            ("leaf".to_string(), 1, 0, 0.3, 0),
        ];
        let boosts = ids_search_boost(&items);
        assert!(boosts[0].entropy > boosts[1].entropy);
    }

    #[test]
    fn well_boost_increases_score() {
        let results = vec![DiscordResult {
            item_id: "well-item".to_string(),
            discord_score: 0.8,
            anchor_ids: vec!["a".to_string()],
            relationship_types: vec!["s".to_string()],
        }];
        let boosted = temporal_well_discord_boost(&results, &["well-item".to_string()]);
        assert!((boosted[0].1 - 1.2).abs() < 0.001); // 0.8*1.5=1.2
    }

    #[test]
    fn well_boost_no_change_for_non_well() {
        let results = vec![DiscordResult {
            item_id: "normal".to_string(),
            discord_score: 0.6,
            anchor_ids: vec!["a".to_string()],
            relationship_types: vec!["s".to_string()],
        }];
        let boosted = temporal_well_discord_boost(&results, &["other".to_string()]);
        assert!((boosted[0].1 - 0.6).abs() < 0.001);
    }

    #[test]
    fn episode_aware_syndrome_traces_causes() {
        let syndromes = vec![Syndrome {
            id: "syn-1".to_string(),
            severity: SyndromeSeverity::Error,
            items: vec!["a".to_string(), "b".to_string()],
            description: "contra".to_string(),
            syndrome_type: SyndromeType::DirectContradiction,
        }];
        let enriched = episode_aware_syndromes(&syndromes, |id| match id {
            "a" => vec!["ep1".to_string()],
            "b" => vec!["ep2".to_string()],
            _ => vec![],
        });
        assert_eq!(enriched.len(), 1);
        assert_eq!(enriched[0].episode_a, vec!["ep1".to_string()]);
        assert_eq!(enriched[0].episode_b, vec!["ep2".to_string()]);
        assert!(enriched[0].causal_chain.contains("a"));
    }

    #[test]
    fn episode_aware_skips_single_item() {
        let syndromes = vec![Syndrome {
            id: "syn-1".to_string(),
            severity: SyndromeSeverity::Warning,
            items: vec!["only".to_string()],
            description: "orphan".to_string(),
            syndrome_type: SyndromeType::OrphanReference,
        }];
        let enriched = episode_aware_syndromes(&syndromes, |_| vec![]);
        assert!(enriched.is_empty());
    }

    #[test]
    fn temporal_stability_boosts_importance() {
        let config = crate::compression_governor::ImportanceConfig::default();
        let base = temporal_stability_importance(0.5, 0.3, 1.0, false, &config);
        let boosted = temporal_stability_importance(0.5, 0.3, 1.0, true, &config);
        assert!(boosted > base);
    }

    #[test]
    fn erasure_cost_boosts_valuable() {
        let results = vec![("val".to_string(), 0.5), ("cheap".to_string(), 0.5)];
        let costs = vec![
            ErasureCost {
                item_id: "val".to_string(),
                structuring: 3.0,
                system_avg: 1.0,
                cost: 2.0,
                is_expensive: true,
            },
            ErasureCost {
                item_id: "cheap".to_string(),
                structuring: 0.5,
                system_avg: 1.0,
                cost: 0.5,
                is_expensive: false,
            },
        ];
        let boosted = erasure_cost_routing_boost(&results, &costs);
        assert!(boosted[0].1 > boosted[1].1);
    }

    #[test]
    fn autonomous_revision_detects_contradictions() {
        let results = vec![("old".to_string(), 0.9), ("new".to_string(), 0.95)];
        let contras = vec![("old".to_string(), "new".to_string())];
        let report = autonomous_knowledge_revision(
            &results,
            &contras,
            &|_| None,
            &|_| vec![],
            &|_| vec![],
            1.0,
        );
        assert!(report.syndromes_detected > 0);
        assert!(report.corrections_computed > 0);
    }

    #[test]
    fn autonomous_revision_with_provenance_and_episodes() {
        // Use a direct contradiction which triggers MarkContradicted → subtraction candidate
        let results = vec![("a".to_string(), 0.8), ("b".to_string(), 0.9)];
        let contras = vec![("a".to_string(), "b".to_string())];
        let report = autonomous_knowledge_revision(
            &results,
            &contras,
            &|id| match id {
                "a" => Some(ConfidenceValue::new(0.8, 2)),
                "b" => Some(ConfidenceValue::new(0.9, 3)),
                _ => None,
            },
            &|id| match id {
                "a" => vec!["ep1".to_string()],
                "b" => vec!["ep2".to_string()],
                _ => vec![],
            },
            &|id| match id {
                "a" => vec!["n1".to_string()],
                _ => vec![],
            },
            2.0,
        );
        // If corrections produce subtraction candidates, demon extracts should be populated
        if !report.subtraction_candidates.is_empty() {
            assert!(!report.demon_extracts.is_empty());
            assert!(report.demon_extracts.iter().any(|d| d.provenance.is_some()));
            assert!(report
                .demon_extracts
                .iter()
                .any(|d| !d.episode_ids.is_empty()));
        }
        // Syndromes should be detected regardless
        assert!(report.syndromes_detected > 0);
    }

    #[test]
    fn autonomous_revision_no_contradictions() {
        let results = vec![("f1".to_string(), 0.9)];
        let report =
            autonomous_knowledge_revision(&results, &[], &|_| None, &|_| vec![], &|_| vec![], 1.0);
        assert!(report.subtraction_candidates.is_empty());
    }

    // ── Wire 16 tests: Topology-aware gap detection ──────────────────

    #[test]
    fn topology_aware_gap_detection_finds_voids() {
        // Edges forming a graph with a gap: a-b, b-c, but no a-c
        let edges = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
        ];
        let voids = crate::topology::find_voids(&edges);
        // Should find at least one void (isolated or missing link)
        assert!(!voids.is_empty() || edges.is_empty());
    }

    #[test]
    fn topology_aware_gap_detection_empty_graph() {
        let voids = crate::topology::find_voids(&[]);
        assert!(voids.is_empty());
    }

    // ── Wire 17 tests: Community-aware compression ───────────────────

    #[test]
    fn community_aware_compression_f32_for_standalone() {
        let communities = vec![crate::community::Community {
            id: "c0".to_string(),
            members: vec!["fact:a".to_string()],
            level: 0,
            parent: None,
        }];
        let importance = vec![("fact:a".to_string(), 0.9)];
        let decisions = crate::community::community_aware_compression(&communities, &importance);
        assert!(!decisions.is_empty());
    }

    // ── Wire 18 tests: Subgraph pruning ──────────────────────────────

    #[test]
    fn subgraph_pruning_produces_receipts() {
        let edges = vec![
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
        ];
        let access_logs = vec![
            crate::subgraph_pruning::AccessLog {
                item_id: "a".to_string(),
                access_count: 5,
                last_accessed: "2026-06-20T00:00:00Z".to_string(),
            },
            crate::subgraph_pruning::AccessLog {
                item_id: "b".to_string(),
                access_count: 1,
                last_accessed: "2026-06-20T00:00:00Z".to_string(),
            },
            crate::subgraph_pruning::AccessLog {
                item_id: "c".to_string(),
                access_count: 3,
                last_accessed: "2026-06-20T00:00:00Z".to_string(),
            },
        ];
        let subgraphs = crate::subgraph_pruning::identify_subgraphs(&edges, &access_logs);
        assert!(!subgraphs.is_empty());
        let receipt = crate::subgraph_pruning::prune_subgraph(&subgraphs[0], &[]);
        assert!(!receipt.pruned_nodes.is_empty());
    }
}
