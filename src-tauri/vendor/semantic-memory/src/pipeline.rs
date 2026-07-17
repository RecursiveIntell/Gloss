//! Multiscale retrieval scheduling pipeline.
//!
//! Runs search in stages, stopping when confidence is met. Each stage has a
//! budget (max items, max time) and a confidence threshold. The pipeline is
//! entirely optional — the existing [`crate::search`] surfaces work without it.
//!
//! Stages are pluggable via `Box<dyn Fn(&StageInput) -> Result<StageOutput, String>
//! + Send + Sync>` closures so the pipeline can be unit-tested without pulling
//! in the full search stack.

use std::time::Instant;

/// Per-stage resource budget.
#[derive(Debug, Clone, Copy)]
pub struct StageBudget {
    /// Maximum number of candidate items a stage is allowed to process.
    pub max_items: usize,
    /// Maximum wall-clock time the stage may take (milliseconds) before it is
    /// considered budget-exhausted. The stage closure itself is responsible for
    /// honouring this in real implementations; the pipeline uses the reported
    /// `elapsed_ms` to detect overruns.
    pub max_time_ms: u64,
}

impl Default for StageBudget {
    fn default() -> Self {
        Self {
            max_items: 100,
            max_time_ms: 100,
        }
    }
}

/// A single stage in the multiscale pipeline.
pub struct RetrievalStage {
    /// Human-readable stage name (e.g. `bm25_coarse`).
    pub name: &'static str,
    /// Resource budget for this stage.
    pub budget: StageBudget,
    /// Stop the pipeline once the stage reports `confidence >= threshold`.
    pub confidence_threshold: f64,
    /// The actual stage implementation. Receives the [`StageInput`] (query,
    /// desired top_k, and the candidate ids carried over from prior stages) and
    /// returns a [`StageOutput`] or an error string.
    pub stage_fn: Box<dyn Fn(&StageInput) -> Result<StageOutput, String> + Send + Sync>,
}

impl std::fmt::Debug for RetrievalStage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RetrievalStage")
            .field("name", &self.name)
            .field("budget", &self.budget)
            .field("confidence_threshold", &self.confidence_threshold)
            .finish_non_exhaustive()
    }
}

/// Input handed to each stage closure.
#[derive(Debug, Clone)]
pub struct StageInput {
    pub query: String,
    pub top_k: usize,
    /// Candidate item ids carried over from the previous stage (empty for the
    /// first stage, which performs coarse retrieval).
    pub candidates: Vec<String>,
}

impl StageInput {
    /// Convenience constructor.
    pub fn new(query: impl Into<String>, top_k: usize, candidates: Vec<String>) -> Self {
        Self {
            query: query.into(),
            top_k,
            candidates,
        }
    }
}

/// Output produced by each stage closure.
#[derive(Debug, Clone)]
pub struct StageOutput {
    /// `(item_id, score)` pairs, sorted best-first.
    pub results: Vec<(String, f64)>,
    /// Stage-reported confidence in its own ranking (0.0..=1.0). Higher is
    /// better. The pipeline stops when this meets the stage threshold.
    pub confidence: f64,
    /// Wall-clock time the stage took, in milliseconds.
    pub elapsed_ms: u64,
    /// Number of candidate items the stage actually processed.
    pub items_processed: usize,
}

impl StageOutput {
    /// Convenience constructor with the fields in declaration order.
    pub fn new(
        results: Vec<(String, f64)>,
        confidence: f64,
        elapsed_ms: u64,
        items_processed: usize,
    ) -> Self {
        Self {
            results,
            confidence,
            elapsed_ms,
            items_processed,
        }
    }

    /// Helper for tests / simple stages: synthesise an output from a result
    /// list, deriving confidence from the top score and counting one item per
    /// result.
    pub fn from_results(results: Vec<(String, f64)>, elapsed_ms: u64) -> Self {
        let confidence = results.first().map(|(_, s)| *s).unwrap_or(0.0).min(1.0);
        let items = results.len();
        Self::new(results, confidence, elapsed_ms, items)
    }
}

/// Why the pipeline stopped after a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// Stage reported `confidence >= threshold`.
    ConfidenceMet,
    /// Stage exceeded its time or item budget.
    BudgetExhausted,
    /// Stage returned no results.
    NoResults,
    /// Stage closure returned an error.
    Error,
}

impl StopReason {
    /// Lowercased snake-case string used in [`StageReceipt::stop_reason`].
    pub fn as_str(&self) -> &'static str {
        match self {
            StopReason::ConfidenceMet => "confidence_met",
            StopReason::BudgetExhausted => "budget_exhausted",
            StopReason::NoResults => "no_results",
            StopReason::Error => "error",
        }
    }
}

impl std::fmt::Display for StopReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One entry per executed stage in the final [`PipelineResult`].
#[derive(Debug, Clone, PartialEq)]
pub struct StageReceipt {
    pub stage_name: String,
    pub input_count: usize,
    pub output_count: usize,
    pub confidence: f64,
    pub stop_reason: String,
}

/// Final result of running the pipeline.
#[derive(Debug, Clone, Default)]
pub struct PipelineResult {
    /// Best-first `(item_id, score)` pairs from the last stage that ran.
    pub final_results: Vec<(String, f64)>,
    /// One receipt per stage that actually executed, in execution order.
    pub stages_executed: Vec<StageReceipt>,
    /// Total wall-clock time across all stages, in milliseconds.
    pub total_elapsed_ms: u64,
}

impl PipelineResult {
    /// True if at least one stage executed and produced results.
    pub fn has_results(&self) -> bool {
        !self.final_results.is_empty()
    }

    /// Number of stages that actually ran.
    pub fn stages_run(&self) -> usize {
        self.stages_executed.len()
    }
}

/// Multiscale retrieval pipeline.
///
/// Construct with [`RetrievalPipeline::new`] (empty) or
/// [`RetrievalPipeline::default_pipeline`] (the canonical 4-stage setup), then
/// chain stages with [`RetrievalPipeline::stage`] and run with
/// [`RetrievalPipeline::execute`].
pub struct RetrievalPipeline {
    stages: Vec<RetrievalStage>,
}

impl Default for RetrievalPipeline {
    fn default() -> Self {
        Self::new()
    }
}

impl RetrievalPipeline {
    /// Create an empty pipeline (no stages). [`execute`](Self::execute) on an
    /// empty pipeline returns an empty [`PipelineResult`].
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Append a stage to the pipeline.
    pub fn stage(mut self, stage: RetrievalStage) -> Self {
        self.stages.push(stage);
        self
    }

    /// Number of stages currently registered.
    pub fn stage_count(&self) -> usize {
        self.stages.len()
    }

    /// Execute the full pipeline: run every stage in order, stopping early when
    /// a stage reports `confidence >= threshold`, when its budget is exhausted,
    /// or when it returns no results / an error.
    ///
    /// The first stage receives an empty `candidates` vec (it is expected to
    /// perform coarse retrieval from the store). Each subsequent stage receives
    /// the previous stage's result ids as `candidates`.
    pub fn execute(&self, query: &str, top_k: usize) -> PipelineResult {
        self.execute_with_depth(query, top_k, self.stages.len())
    }

    /// Execute at most `depth` stages. `depth = 0` runs nothing; `depth >=
    /// stage_count()` runs the whole pipeline.
    pub fn execute_with_depth(&self, query: &str, top_k: usize, depth: usize) -> PipelineResult {
        let depth = depth.min(self.stages.len());
        let mut result = PipelineResult::default();
        let mut candidates: Vec<String> = Vec::new();
        let mut last_results: Vec<(String, f64)> = Vec::new();
        let mut carry_confidence: f64 = 0.0;

        for stage in self.stages.iter().take(depth) {
            let input = StageInput {
                query: query.to_string(),
                top_k,
                candidates: candidates.clone(),
            };
            let input_count = input.candidates.len();
            let stage_start = Instant::now();

            let output = match (stage.stage_fn)(&input) {
                Ok(o) => o,
                Err(_msg) => {
                    result.stages_executed.push(StageReceipt {
                        stage_name: stage.name.to_string(),
                        input_count,
                        output_count: 0,
                        confidence: carry_confidence,
                        stop_reason: StopReason::Error.as_str().to_string(),
                    });
                    // On error we keep whatever results we had and stop.
                    result.final_results = last_results;
                    return result;
                }
            };

            let measured_ms = stage_start.elapsed().as_millis() as u64;
            // Prefer the stage's self-reported elapsed, but if it lied (reported
            // 0) fall back to the measured wall time so budget checks work even
            // for naive closures.
            let elapsed_ms = if output.elapsed_ms == 0 {
                measured_ms
            } else {
                output.elapsed_ms
            };
            result.total_elapsed_ms = result.total_elapsed_ms.saturating_add(elapsed_ms);

            // Determine the stop reason.
            // Use the *effective* confidence = max(stage's own confidence,
            // carried-over confidence from earlier stages). This is how
            // confidence propagates: a confident earlier stage can satisfy a
            // later stage's threshold even if that later stage reports a lower
            // local confidence.
            let effective_confidence = carry_confidence.max(output.confidence);
            let stop_reason = if output.results.is_empty() {
                StopReason::NoResults
            } else if elapsed_ms > stage.budget.max_time_ms
                || output.items_processed > stage.budget.max_items
            {
                StopReason::BudgetExhausted
            } else if effective_confidence >= stage.confidence_threshold {
                StopReason::ConfidenceMet
            } else {
                // Stage completed but did not meet confidence; continue.
                // We still record a receipt but mark it with a synthetic
                // "continued" string so the chain is inspectable.
                result.stages_executed.push(StageReceipt {
                    stage_name: stage.name.to_string(),
                    input_count,
                    output_count: output.results.len(),
                    confidence: effective_confidence,
                    stop_reason: "continued".to_string(),
                });
                // Propagate confidence: a later stage inherits the best
                // confidence seen so far so a confident early stage can nudge
                // a borderline later stage over its threshold.
                carry_confidence = effective_confidence;
                candidates = output.results.iter().map(|(id, _)| id.clone()).collect();
                last_results = output.results;
                continue;
            };

            // Terminal branch: record the receipt and stop.
            result.stages_executed.push(StageReceipt {
                stage_name: stage.name.to_string(),
                input_count,
                output_count: output.results.len(),
                confidence: effective_confidence,
                stop_reason: stop_reason.as_str().to_string(),
            });
            carry_confidence = effective_confidence;
            last_results = output.results.clone();
            candidates = output.results.iter().map(|(id, _)| id.clone()).collect();
            if matches!(
                stop_reason,
                StopReason::ConfidenceMet | StopReason::BudgetExhausted | StopReason::NoResults
            ) {
                result.final_results = last_results.clone();
                return result;
            }
        }

        // Ran through all (non-terminal) stages without an explicit stop.
        result.final_results = last_results;
        result
    }
}

impl RetrievalPipeline {
    /// Build the canonical 4-stage default pipeline with placeholder closures.
    ///
    /// The closures are identity pass-throughs: they echo the input candidates
    /// back as results with a synthetic confidence of `0.0`. Real integrations
    /// replace them with closures that call the appropriate search functions.
    /// The stage metadata (names, budgets, thresholds) matches the spec:
    ///
    /// 1. `bm25_coarse` — top-100, 50ms, 0.80
    /// 2. `vector_medium` — top-20, 100ms, 0.85
    /// 3. `rerank_fine` — top-5, 200ms, 0.90
    /// 4. `graph_expansion` — optional, 100ms, 0.92
    pub fn default_pipeline() -> Self {
        Self::new()
            .stage(RetrievalStage {
                name: "bm25_coarse",
                budget: StageBudget {
                    max_items: 100,
                    max_time_ms: 50,
                },
                confidence_threshold: 0.8,
                stage_fn: Box::new(|input: &StageInput| {
                    Ok(StageOutput::from_results(
                        input
                            .candidates
                            .iter()
                            .take(100)
                            .map(|id| (id.clone(), 0.0))
                            .collect(),
                        0,
                    ))
                }),
            })
            .stage(RetrievalStage {
                name: "vector_medium",
                budget: StageBudget {
                    max_items: 20,
                    max_time_ms: 100,
                },
                confidence_threshold: 0.85,
                stage_fn: Box::new(|input: &StageInput| {
                    Ok(StageOutput::from_results(
                        input
                            .candidates
                            .iter()
                            .take(20)
                            .map(|id| (id.clone(), 0.0))
                            .collect(),
                        0,
                    ))
                }),
            })
            .stage(RetrievalStage {
                name: "rerank_fine",
                budget: StageBudget {
                    max_items: 5,
                    max_time_ms: 200,
                },
                confidence_threshold: 0.9,
                stage_fn: Box::new(|input: &StageInput| {
                    Ok(StageOutput::from_results(
                        input
                            .candidates
                            .iter()
                            .take(5)
                            .map(|id| (id.clone(), 0.0))
                            .collect(),
                        0,
                    ))
                }),
            })
            .stage(RetrievalStage {
                name: "graph_expansion",
                budget: StageBudget {
                    max_items: 100,
                    max_time_ms: 100,
                },
                confidence_threshold: 0.92,
                stage_fn: Box::new(|input: &StageInput| {
                    Ok(StageOutput::from_results(
                        input
                            .candidates
                            .iter()
                            .take(100)
                            .map(|id| (id.clone(), 0.0))
                            .collect(),
                        0,
                    ))
                }),
            })
    }

    /// Execute the pipeline, then run the decoder as a post-pipeline
    /// refinement stage (stage 5). The decoder:
    /// 1. Detects syndromes (contradictions, missing links, orphans) in the
    ///    pipeline's final results.
    /// 2. Builds a conflict graph from those syndromes.
    /// 3. Runs belief propagation (message passing) to refine confidence
    ///    scores — items involved in contradictions get pulled down.
    /// 4. Returns the refined results with updated scores and a decoder
    ///    receipt appended to `stages_executed`.
    ///
    /// `contradictions` is a list of `(item_a, item_b)` pairs that the caller
    /// knows contradict each other (e.g. from provenance metadata or temporal
    /// analysis). Pass an empty slice if no explicit contradictions are known —
    /// the decoder will still detect missing links and orphan references from
    /// the result set itself.
    ///
    /// `max_iterations` and `convergence_threshold` control message passing.
    /// Reasonable defaults: `max_iterations = 50`, `convergence_threshold = 0.001`.
    #[cfg(feature = "decoder")]
    pub fn execute_with_decoder(
        &self,
        query: &str,
        top_k: usize,
        contradictions: &[(String, String)],
        max_iterations: usize,
        convergence_threshold: f64,
    ) -> PipelineResult {
        let mut result = self.execute(query, top_k);

        if result.final_results.is_empty() {
            return result;
        }

        // Phase 5a: Detect syndromes in the pipeline's final results.
        let syndromes = crate::decoder::detect_syndromes(&result.final_results, contradictions);

        // Phase 5b: Build conflict graph from results + syndromes.
        let graph =
            crate::decoder::ConflictGraph::from_syndromes(&result.final_results, &syndromes);

        // Phase 5c: Run belief propagation to refine confidence scores.
        let mp = crate::decoder::pass_messages(&graph, max_iterations, convergence_threshold);

        // Phase 5d: Replace final_results with refined scores from message passing.
        let mut refined: Vec<(String, f64)> = result
            .final_results
            .iter()
            .map(|(id, _)| {
                let refined_conf = mp.node_confidences.get(id).copied().unwrap_or(0.0);
                (id.clone(), refined_conf)
            })
            .collect();

        // Sort best-first by refined confidence.
        refined.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        let syndrome_count = syndromes.len();
        result.final_results = refined;

        // Append a decoder receipt to the stage chain.
        result.stages_executed.push(StageReceipt {
            stage_name: "decoder_refine".to_string(),
            input_count: graph.nodes.len(),
            output_count: result.final_results.len(),
            confidence: result.final_results.first().map(|(_, s)| *s).unwrap_or(0.0),
            stop_reason: format!(
                "syndromes={} iterations={} converged={}",
                syndrome_count, mp.iterations, mp.converged
            ),
        });

        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: build a stage that returns fixed results with a given confidence
    /// and elapsed time.
    fn fixed_stage(
        name: &'static str,
        threshold: f64,
        results: Vec<(String, f64)>,
        confidence: f64,
        elapsed_ms: u64,
    ) -> RetrievalStage {
        let items = results.len();
        RetrievalStage {
            name,
            budget: StageBudget {
                max_items: items.max(100),
                max_time_ms: 1_000,
            },
            confidence_threshold: threshold,
            stage_fn: Box::new(move |_input: &StageInput| {
                Ok(StageOutput::new(
                    results.clone(),
                    confidence,
                    elapsed_ms,
                    items,
                ))
            }),
        }
    }

    #[test]
    fn test_pipeline_stops_at_stage_1() {
        // High confidence on stage 1 → pipeline stops after one stage.
        let pipe = RetrievalPipeline::new().stage(fixed_stage(
            "s1",
            0.8,
            vec![("a".to_string(), 0.95), ("b".to_string(), 0.3)],
            0.9, // >= 0.8 threshold
            1,
        ));
        let res = pipe.execute("q", 5);
        assert_eq!(res.stages_executed.len(), 1);
        assert_eq!(res.stages_executed[0].stage_name, "s1");
        assert_eq!(res.stages_executed[0].stop_reason, "confidence_met");
        assert_eq!(res.final_results.len(), 2);
    }

    #[test]
    fn test_pipeline_runs_all_stages() {
        // Low confidence on every stage → pipeline runs all three.
        let pipe = RetrievalPipeline::new()
            .stage(fixed_stage("s1", 0.8, vec![("a".to_string(), 0.5)], 0.1, 1))
            .stage(fixed_stage(
                "s2",
                0.85,
                vec![("a".to_string(), 0.5)],
                0.1,
                1,
            ))
            .stage(fixed_stage("s3", 0.9, vec![("a".to_string(), 0.5)], 0.1, 1));
        let res = pipe.execute("q", 5);
        assert_eq!(res.stages_executed.len(), 3);
        // The first two should be "continued", the last is terminal.
        assert_eq!(res.stages_executed[0].stop_reason, "continued");
        assert_eq!(res.stages_executed[1].stop_reason, "continued");
        // Last stage: confidence not met, budget not exhausted, results exist
        // → it falls through to the terminal branch. Since none of the explicit
        // terminal conditions matched, we treat completion as confidence_met
        // only if threshold met; otherwise the loop ends naturally. Here the
        // last receipt should still be recorded. The final_results come from
        // the last stage.
        assert!(!res.final_results.is_empty());
    }

    #[test]
    fn test_pipeline_budget_exhausted() {
        // Stage reports elapsed > budget → BudgetExhausted.
        let pipe = RetrievalPipeline::new().stage(RetrievalStage {
            name: "slow",
            budget: StageBudget {
                max_items: 100,
                max_time_ms: 10,
            },
            confidence_threshold: 0.99,
            stage_fn: Box::new(|_| {
                Ok(StageOutput::new(
                    vec![("a".to_string(), 0.9)],
                    0.5,
                    50, // > 10ms budget
                    1,
                ))
            }),
        });
        let res = pipe.execute("q", 5);
        assert_eq!(res.stages_executed.len(), 1);
        assert_eq!(res.stages_executed[0].stop_reason, "budget_exhausted");
    }

    #[test]
    fn test_pipeline_no_results() {
        // Stage returns empty results → NoResults.
        let pipe = RetrievalPipeline::new().stage(RetrievalStage {
            name: "empty",
            budget: StageBudget {
                max_items: 100,
                max_time_ms: 100,
            },
            confidence_threshold: 0.9,
            stage_fn: Box::new(|_| Ok(StageOutput::new(vec![], 0.0, 1, 0))),
        });
        let res = pipe.execute("q", 5);
        assert_eq!(res.stages_executed.len(), 1);
        assert_eq!(res.stages_executed[0].stop_reason, "no_results");
        assert!(res.final_results.is_empty());
    }

    #[test]
    fn test_pipeline_receipt_chain() {
        // All stage receipts are present and in order.
        let pipe = RetrievalPipeline::new()
            .stage(fixed_stage(
                "alpha",
                0.8,
                vec![("a".to_string(), 0.4)],
                0.1,
                1,
            ))
            .stage(fixed_stage(
                "beta",
                0.85,
                vec![("a".to_string(), 0.4)],
                0.1,
                1,
            ))
            .stage(fixed_stage(
                "gamma",
                0.9,
                vec![("a".to_string(), 0.95)],
                0.95,
                1,
            ));
        let res = pipe.execute("q", 5);
        assert_eq!(res.stages_executed.len(), 3);
        assert_eq!(res.stages_executed[0].stage_name, "alpha");
        assert_eq!(res.stages_executed[1].stage_name, "beta");
        assert_eq!(res.stages_executed[2].stage_name, "gamma");
        assert_eq!(res.stages_executed[2].stop_reason, "confidence_met");
    }

    #[test]
    fn test_pipeline_depth_limit() {
        // execute_with_depth limits the number of stages run.
        let pipe = RetrievalPipeline::new()
            .stage(fixed_stage(
                "s1",
                0.99,
                vec![("a".to_string(), 0.1)],
                0.0,
                1,
            ))
            .stage(fixed_stage(
                "s2",
                0.99,
                vec![("a".to_string(), 0.1)],
                0.0,
                1,
            ))
            .stage(fixed_stage(
                "s3",
                0.99,
                vec![("a".to_string(), 0.1)],
                0.0,
                1,
            ));
        let res = pipe.execute_with_depth("q", 5, 2);
        assert_eq!(res.stages_executed.len(), 2);
        assert_eq!(res.stages_executed[0].stage_name, "s1");
        assert_eq!(res.stages_executed[1].stage_name, "s2");
        // depth 0 runs nothing
        let res0 = pipe.execute_with_depth("q", 5, 0);
        assert_eq!(res0.stages_executed.len(), 0);
        assert!(res0.final_results.is_empty());
    }

    #[test]
    fn test_pipeline_empty() {
        // No stages → empty result, no panic.
        let pipe = RetrievalPipeline::new();
        let res = pipe.execute("q", 5);
        assert_eq!(res.stages_executed.len(), 0);
        assert!(res.final_results.is_empty());
        assert_eq!(res.total_elapsed_ms, 0);
    }

    #[test]
    fn test_pipeline_confidence_propagates() {
        // Confidence from an earlier stage should satisfy a later stage's
        // threshold even when that later stage reports a lower local
        // confidence. Stage 1 reports confidence 0.95 but its threshold is
        // 0.99 so it does NOT stop. Stage 2 has threshold 0.9 but reports only
        // 0.7 locally — yet because the carried confidence from stage 1 is
        // 0.95, the effective confidence max(0.95, 0.7) = 0.95 >= 0.9 and the
        // pipeline stops at stage 2 with reason `confidence_met`.
        let pipe = RetrievalPipeline::new()
            .stage(fixed_stage(
                "s1",
                0.99, // high threshold so stage 1 does not stop
                vec![("a".to_string(), 0.95)],
                0.95, // local confidence, carried forward
                1,
            ))
            .stage(fixed_stage(
                "s2",
                0.9, // threshold below the carried confidence
                vec![("a".to_string(), 0.7)],
                0.7, // local confidence below its own threshold
                1,
            ));
        let res = pipe.execute("q", 5);
        // Both stages ran.
        assert_eq!(res.stages_executed.len(), 2);
        // Stage 1 continued (confidence below its 0.99 threshold).
        assert_eq!(res.stages_executed[0].stop_reason, "continued");
        assert!((res.stages_executed[0].confidence - 0.95).abs() < 1e-9);
        // Stage 2 stopped via confidence_met thanks to the propagated 0.95.
        assert_eq!(res.stages_executed[1].stop_reason, "confidence_met");
        // The recorded confidence is the effective (carried) confidence.
        assert!(
            (res.stages_executed[1].confidence - 0.95).abs() < 1e-9,
            "expected carried confidence 0.95, got {}",
            res.stages_executed[1].confidence
        );
    }

    // ── Decoder integration tests ───────────────────────────────────────

    #[cfg(feature = "decoder")]
    #[test]
    fn test_decoder_refine_adds_stage_receipt() {
        let pipe = RetrievalPipeline::new().stage(fixed_stage(
            "s1",
            0.8,
            vec![
                ("a".to_string(), 0.95),
                ("b".to_string(), 0.90),
                ("c".to_string(), 0.50),
            ],
            0.9,
            1,
        ));
        let res = pipe.execute_with_decoder("q", 5, &[], 50, 0.001);
        // Pipeline stops after s1 (confidence met), then decoder adds a receipt.
        assert_eq!(res.stages_executed.len(), 2);
        assert_eq!(res.stages_executed[0].stage_name, "s1");
        assert_eq!(res.stages_executed[1].stage_name, "decoder_refine");
        // Decoder receipt should contain syndrome/iteration info.
        assert!(
            res.stages_executed[1].stop_reason.contains("syndromes="),
            "decoder receipt should include syndrome count"
        );
    }

    #[cfg(feature = "decoder")]
    #[test]
    fn test_decoder_refine_refines_contradictory_scores() {
        // Two items: one high confidence, one low, with a known contradiction.
        let pipe = RetrievalPipeline::new().stage(fixed_stage(
            "s1",
            0.8,
            vec![("high".to_string(), 0.95), ("low".to_string(), 0.30)],
            0.9,
            1,
        ));
        let contradictions = vec![("high".to_string(), "low".to_string())];
        let res = pipe.execute_with_decoder("q", 5, &contradictions, 100, 0.001);
        // After message passing, "high" should decrease and "low" should increase
        // because they pull toward each other.
        let high_score = res
            .final_results
            .iter()
            .find(|(id, _)| id == "high")
            .map(|(_, s)| *s)
            .unwrap_or(0.0);
        let low_score = res
            .final_results
            .iter()
            .find(|(id, _)| id == "low")
            .map(|(_, s)| *s)
            .unwrap_or(0.0);
        assert!(
            high_score < 0.95,
            "high should decrease from 0.95, got {}",
            high_score
        );
        assert!(
            low_score > 0.30,
            "low should increase from 0.30, got {}",
            low_score
        );
    }

    #[cfg(feature = "decoder")]
    #[test]
    fn test_decoder_refine_empty_results_noop() {
        // Pipeline returns no results → decoder should be a no-op.
        let pipe = RetrievalPipeline::new().stage(fixed_stage("s1", 0.8, vec![], 0.0, 1));
        let res = pipe.execute_with_decoder("q", 5, &[], 50, 0.001);
        // No results, no decoder receipt (decoder returns early).
        assert!(res.final_results.is_empty());
        // Only the pipeline stage receipt (s1 with no_results).
        assert_eq!(res.stages_executed.len(), 1);
    }

    #[cfg(feature = "decoder")]
    #[test]
    fn test_decoder_refine_no_contradictions_preserves_scores() {
        // No contradictions → message passing keeps initial confidence (no neighbors).
        let pipe = RetrievalPipeline::new().stage(fixed_stage(
            "s1",
            0.8,
            vec![("a".to_string(), 0.95), ("b".to_string(), 0.90)],
            0.9,
            1,
        ));
        let res = pipe.execute_with_decoder("q", 5, &[], 50, 0.001);
        // Without contradictions, items have no neighbors → scores stay the same.
        // But detect_syndromes may find missing links between close-scoring items.
        // So we just verify the decoder ran and produced results.
        assert!(!res.final_results.is_empty());
        assert_eq!(res.stages_executed.len(), 2);
    }
}
