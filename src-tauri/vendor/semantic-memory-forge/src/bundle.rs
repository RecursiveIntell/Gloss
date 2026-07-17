//! Evidence bundle schema.
//!
//! Each causal/effect verification produces an `EvidenceBundle` that captures
//! the full methodological chain from question through estimation and refutation.
//!
//! ## Required Fields (per canonical spec)
//!
//! - causal question
//! - unit definition
//! - treatment specification
//! - outcome specification
//! - covariates/confounders recorded
//! - identification rationale
//! - estimator and estimate
//! - refutations attempted + results
//! - raw receipt / trace / replay handles

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use stack_ids::{AttemptId, ClaimId, EnvelopeId, TraceCtx, TrialId};

use crate::estimator::EstimatorMeta;

/// Opaque identifier for an evidence bundle.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceBundleId(pub String);

impl EvidenceBundleId {
    /// Generates a fresh opaque evidence-bundle identifier.
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    /// Wraps an existing identifier string as an evidence-bundle identifier.
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    /// Returns the underlying identifier string.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EvidenceBundleId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// The causal question being investigated.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CausalQuestion {
    /// Natural language description of the causal question.
    pub description: String,
    /// The unit of analysis (e.g., "code patch", "configuration change").
    pub unit_definition: String,
}

/// Treatment specification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct TreatmentSpec {
    /// What intervention is being tested.
    pub description: String,
    /// Baseline condition description.
    pub baseline_description: String,
    /// Whether paired baseline-vs-patched trials were used.
    pub paired_trials: bool,
}

/// Outcome specification.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct OutcomeSpec {
    /// What outcome is being measured.
    pub description: String,
    /// Measurement method.
    pub measurement_method: String,
    /// Whether outcome is binary, continuous, ordinal, etc.
    pub outcome_type: String,
}

/// A refutation attempt and its result.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RefutationAttempt {
    /// Refutation method (e.g., "placebo_treatment", "random_cause", "subset_data").
    pub method: String,
    /// Result of the refutation attempt.
    pub result: RefutationResult,
    /// Estimator used for the refutation.
    pub estimator_kind: Option<String>,
    /// Parameters for the refutation.
    pub parameters: Option<serde_json::Value>,
}

/// Result of a refutation attempt.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum RefutationResult {
    /// Refutation did not invalidate the original estimate.
    Passed {
        /// How much the estimate changed under the refutation test.
        estimate_change: Option<f64>,
    },
    /// Refutation invalidated the original estimate.
    Failed {
        /// Description of how/why the refutation succeeded.
        reason: String,
        /// How much the estimate changed.
        estimate_change: Option<f64>,
    },
    /// Refutation could not be completed.
    Inconclusive { reason: String },
    /// Refutation was not attempted (e.g., not applicable).
    Skipped { reason: String },
}

/// Baseline-vs-patched side for a verification trial.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationTrialSide {
    Baseline,
    Patched,
}

/// First-class verification trial record preserved in canonical raw truth.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct VerificationTrialRecord {
    /// Trial identifier.
    pub trial_id: TrialId,
    /// Retry lineage / attempt family.
    pub attempt_id: AttemptId,
    /// Which side of the paired family this trial belongs to.
    pub side: VerificationTrialSide,
    /// Whether the trial completed.
    pub completed: bool,
    /// Opaque receipt handles tied to this trial.
    #[serde(default)]
    pub receipt_handles: Vec<String>,
}

/// Immutable comparability snapshot for a paired experiment family.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ComparabilitySnapshot {
    /// Which workload was exercised.
    pub workload_id: String,
    /// Backend family used for execution.
    pub backend_family: String,
    /// Ordered list of checks run for the pair.
    pub selected_checks: Vec<String>,
    /// Effective timeout class.
    pub timeout_class: String,
    /// Sorted execution-affecting flags.
    pub config_flags: Vec<String>,
    /// Explicit verdict when known; `None` preserves absence rather than inference.
    pub comparable: Option<bool>,
    /// Violations recorded during comparability checking.
    #[serde(default)]
    pub violations: Vec<String>,
}

/// First-class refutation artifact preserved in canonical raw truth.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RefutationArtifactRecord {
    /// Artifact identifier.
    pub artifact_id: String,
    /// Artifact type / family name.
    pub artifact_type: String,
    /// Trial that emitted the artifact, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<TrialId>,
    /// Attempt that emitted the artifact, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<AttemptId>,
    /// Artifact outcome.
    pub result: RefutationResult,
    /// Effect delta / estimate change when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimate_delta: Option<f64>,
    /// Structured debug/detail payload when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<String>,
}

/// Compact lifecycle state that can be projected into memory-visible truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerificationLifecycleState {
    Unverified,
    Verified,
    Contradicted,
    Superseded,
}

/// Promotion state for the claim represented by this bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum PromotionState {
    NotPromoted,
    Eligible,
    Blocked {
        reason: String,
    },
    Promoted {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        version_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        promoted_at: Option<String>,
    },
}

/// Compact verification summary designed for projection/import visibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VerificationSummary {
    /// Verification lifecycle.
    pub lifecycle_state: VerificationLifecycleState,
    /// Promotion state.
    pub promotion_state: PromotionState,
    /// Number of completed trials contributing to the summary.
    pub completed_trial_count: u32,
    /// Number of passed refutation artifacts.
    pub passed_refutation_count: u32,
    /// Number of failed refutation artifacts.
    pub failed_refutation_count: u32,
    /// Immutable comparability snapshot version when carried by the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparability_snapshot_version: Option<String>,
    /// Human-readable, source-authored notes.
    #[serde(default)]
    pub notes: Vec<String>,
}

/// An evidence bundle capturing the full methodological chain.
///
/// This is the minimal but real evidence-bundle substrate required by the
/// canonical verification pipeline. Every field maps to a requirement in
/// the master delta spec.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EvidenceBundle {
    /// Unique bundle identifier.
    pub id: EvidenceBundleId,

    // ── Causal design ──────────────────────────────────────
    /// The causal question being investigated.
    pub question: CausalQuestion,
    /// Treatment specification.
    pub treatment: TreatmentSpec,
    /// Outcome specification.
    pub outcome: OutcomeSpec,
    /// Covariates and confounders recorded.
    pub covariates: Vec<String>,
    /// Identification rationale (why we believe the causal identification is valid).
    pub identification_rationale: String,

    // ── Estimation ─────────────────────────────────────────
    /// Estimator kind (e.g., "diff_in_diff", "propensity_score", "iv").
    pub estimator_kind: String,
    /// Estimator version (semantic versioning or commit hash).
    pub estimator_version: String,
    /// Structured estimator metadata for replay/audit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimator_meta: Option<EstimatorMeta>,
    /// The causal estimate (effect size).
    pub estimate: f64,
    /// Standard error or uncertainty measure.
    pub estimate_uncertainty: Option<f64>,
    /// Confidence in the estimate (0.0 - 1.0).
    pub confidence: f32,
    /// Number of trials / observations.
    pub trial_count: u32,
    /// Whether variance-aware repeated trials were used.
    pub variance_aware: bool,
    /// Explicit paired verification-trial family.
    #[serde(default)]
    pub verification_trials: Vec<VerificationTrialRecord>,
    /// Immutable comparability snapshot for the paired family, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub comparability_snapshot: Option<ComparabilitySnapshot>,

    // ── Refutation ─────────────────────────────────────────
    /// Refutation attempts and their results.
    pub refutations: Vec<RefutationAttempt>,
    /// First-class refutation artifacts with stable identities and lineage.
    #[serde(default)]
    pub refutation_artifacts: Vec<RefutationArtifactRecord>,
    /// Compact verification/promotion summary for projection consumers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification_summary: Option<VerificationSummary>,

    // ── Provenance ─────────────────────────────────────────
    /// Raw receipt handle (opaque reference to the underlying raw data).
    pub raw_receipt_handle: Option<String>,
    /// Trace context for correlation.
    pub trace_ctx: Option<TraceCtx>,
    /// Attempt ID for retry lineage.
    pub attempt_id: Option<AttemptId>,
    /// Trial ID for the specific execution.
    pub trial_id: Option<TrialId>,
    /// Replay handle (linkage to original execution for replay).
    pub replay_handle: Option<String>,
    /// Source envelope ID if this bundle originated from an export.
    pub source_envelope_id: Option<EnvelopeId>,
    /// Claim IDs this bundle provides evidence for.
    pub claim_ids: Vec<ClaimId>,

    // ── Metadata ───────────────────────────────────────────
    /// When the bundle was created.
    pub created_at: String,
    /// Comparability snapshot version (immutable once set).
    pub comparability_snapshot_version: Option<String>,
    /// Additional metadata.
    pub metadata: Option<serde_json::Value>,
}

impl EvidenceBundle {
    /// Create a new evidence bundle with required fields.
    pub fn new(
        question: CausalQuestion,
        treatment: TreatmentSpec,
        outcome: OutcomeSpec,
        estimator_kind: impl Into<String>,
        estimator_version: impl Into<String>,
        estimate: f64,
    ) -> Self {
        Self {
            id: EvidenceBundleId::generate(),
            question,
            treatment,
            outcome,
            covariates: Vec::new(),
            identification_rationale: String::new(),
            estimator_kind: estimator_kind.into(),
            estimator_version: estimator_version.into(),
            estimator_meta: None,
            estimate,
            estimate_uncertainty: None,
            confidence: 0.0,
            trial_count: 0,
            variance_aware: false,
            verification_trials: Vec::new(),
            comparability_snapshot: None,
            refutations: Vec::new(),
            refutation_artifacts: Vec::new(),
            verification_summary: None,
            raw_receipt_handle: None,
            trace_ctx: None,
            attempt_id: None,
            trial_id: None,
            replay_handle: None,
            source_envelope_id: None,
            claim_ids: Vec::new(),
            created_at: chrono::Utc::now().to_rfc3339(),
            comparability_snapshot_version: None,
            metadata: None,
        }
    }

    /// Whether all refutation attempts passed.
    pub fn all_refutations_passed(&self) -> bool {
        self.refutations.iter().all(|r| {
            matches!(
                r.result,
                RefutationResult::Passed { .. } | RefutationResult::Skipped { .. }
            )
        })
    }

    /// Whether any refutation attempt failed.
    pub fn has_failed_refutation(&self) -> bool {
        self.refutations
            .iter()
            .any(|r| matches!(r.result, RefutationResult::Failed { .. }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_bundle_creation() {
        let bundle = EvidenceBundle::new(
            CausalQuestion {
                description: "Does patch X fix bug Y?".into(),
                unit_definition: "code patch".into(),
            },
            TreatmentSpec {
                description: "Apply patch X".into(),
                baseline_description: "Original code".into(),
                paired_trials: true,
            },
            OutcomeSpec {
                description: "Test suite passes".into(),
                measurement_method: "test runner".into(),
                outcome_type: "binary".into(),
            },
            "diff_in_diff",
            "1.0.0",
            0.85,
        );

        assert!(!bundle.id.as_str().is_empty());
        assert_eq!(bundle.estimate, 0.85);
        assert!(bundle.refutations.is_empty());
        assert!(bundle.verification_trials.is_empty());
        assert!(bundle.refutation_artifacts.is_empty());
        assert!(bundle.estimator_meta.is_none());
        assert!(bundle.verification_summary.is_none());
        assert!(bundle.all_refutations_passed());
        assert!(!bundle.has_failed_refutation());
    }

    #[test]
    fn refutation_tracking() {
        let mut bundle = EvidenceBundle::new(
            CausalQuestion {
                description: "test".into(),
                unit_definition: "unit".into(),
            },
            TreatmentSpec {
                description: "t".into(),
                baseline_description: "b".into(),
                paired_trials: false,
            },
            OutcomeSpec {
                description: "o".into(),
                measurement_method: "m".into(),
                outcome_type: "binary".into(),
            },
            "ols",
            "1.0.0",
            0.5,
        );

        bundle.refutations.push(RefutationAttempt {
            method: "placebo_treatment".into(),
            result: RefutationResult::Passed {
                estimate_change: Some(0.01),
            },
            estimator_kind: None,
            parameters: None,
        });
        assert!(bundle.all_refutations_passed());
        assert!(!bundle.has_failed_refutation());

        bundle.refutations.push(RefutationAttempt {
            method: "random_cause".into(),
            result: RefutationResult::Failed {
                reason: "estimate changed significantly".into(),
                estimate_change: Some(0.45),
            },
            estimator_kind: None,
            parameters: None,
        });
        assert!(!bundle.all_refutations_passed());
        assert!(bundle.has_failed_refutation());
    }

    #[test]
    fn serde_roundtrip() {
        let bundle = EvidenceBundle::new(
            CausalQuestion {
                description: "q".into(),
                unit_definition: "u".into(),
            },
            TreatmentSpec {
                description: "t".into(),
                baseline_description: "b".into(),
                paired_trials: true,
            },
            OutcomeSpec {
                description: "o".into(),
                measurement_method: "m".into(),
                outcome_type: "continuous".into(),
            },
            "iv",
            "2.0.0",
            1.23,
        );

        let json = serde_json::to_string(&bundle).unwrap();
        let back: EvidenceBundle = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id.as_str(), bundle.id.as_str());
        assert_eq!(back.estimate, bundle.estimate);
        assert_eq!(back.estimator_kind, "iv");
    }

    #[test]
    fn verification_fields_roundtrip_as_first_class_artifacts() {
        let mut bundle = EvidenceBundle::new(
            CausalQuestion {
                description: "Does the patch improve benchmark latency?".into(),
                unit_definition: "paired benchmark run".into(),
            },
            TreatmentSpec {
                description: "apply patch-123".into(),
                baseline_description: "baseline checkout".into(),
                paired_trials: true,
            },
            OutcomeSpec {
                description: "benchmark latency drops".into(),
                measurement_method: "criterion benchmark".into(),
                outcome_type: "continuous".into(),
            },
            "before_after",
            "3.1.4",
            0.27,
        );
        bundle.covariates = vec![
            "workload:bench-a".into(),
            "backend:cargo".into(),
            "known_threat:cache_warmup".into(),
        ];
        bundle.identification_rationale = "same workload, flags, and timeout class".into();
        bundle.estimator_meta = Some(crate::estimator::EstimatorMeta {
            kind: crate::estimator::EstimatorKind::Custom(
                "living_memory_phase5_scorevector".into(),
            ),
            version: "3.1.4".into(),
            parameters: serde_json::json!({
                "weighted_total": 0.27,
            }),
            random_seed: Some(7),
            environment: Some(crate::estimator::EnvironmentFingerprint {
                python_version: None,
                package_versions: serde_json::json!({
                    "checks": ["cargo test"],
                }),
                platform: Some("linux".into()),
                env_hash: Some("env-123".into()),
            }),
            timeout_secs: Some(60),
            failure_mode: None,
            request_schema_version: Some("living_memory.phase5.bundle.v1".into()),
            response_schema_version: Some("semantic_memory_forge.evidence_bundle.v3".into()),
        });
        bundle.estimate_uncertainty = Some(0.04);
        bundle.confidence = 0.91;
        bundle.trial_count = 2;
        bundle.variance_aware = true;
        bundle.verification_trials = vec![
            VerificationTrialRecord {
                trial_id: TrialId::new("trial-baseline-1"),
                attempt_id: AttemptId::new("attempt-family-1"),
                side: VerificationTrialSide::Baseline,
                completed: true,
                receipt_handles: vec!["receipt:baseline".into()],
            },
            VerificationTrialRecord {
                trial_id: TrialId::new("trial-patched-1"),
                attempt_id: AttemptId::new("attempt-family-1"),
                side: VerificationTrialSide::Patched,
                completed: true,
                receipt_handles: vec!["receipt:patched".into()],
            },
        ];
        bundle.comparability_snapshot = Some(ComparabilitySnapshot {
            workload_id: "bench-a".into(),
            backend_family: "cargo".into(),
            selected_checks: vec!["cargo test".into()],
            timeout_class: "short".into(),
            config_flags: vec!["--all-features".into()],
            comparable: Some(true),
            violations: vec![],
        });
        bundle.refutations = vec![RefutationAttempt {
            method: "placebo".into(),
            result: RefutationResult::Passed {
                estimate_change: Some(0.01),
            },
            estimator_kind: Some("living_memory_phase5_scorevector".into()),
            parameters: Some(serde_json::json!({
                "artifact_id": "placebo-1",
            })),
        }];
        bundle.refutation_artifacts = vec![RefutationArtifactRecord {
            artifact_id: "placebo-1".into(),
            artifact_type: "placebo".into(),
            trial_id: Some(TrialId::new("trial-baseline-1")),
            attempt_id: Some(AttemptId::new("attempt-family-1")),
            result: RefutationResult::Passed {
                estimate_change: Some(0.01),
            },
            estimate_delta: Some(0.01),
            details: Some("placebo preserved the null effect".into()),
        }];
        bundle.verification_summary = Some(VerificationSummary {
            lifecycle_state: VerificationLifecycleState::Verified,
            promotion_state: PromotionState::Eligible,
            completed_trial_count: 2,
            passed_refutation_count: 1,
            failed_refutation_count: 0,
            comparability_snapshot_version: Some("bench-a:cargo:short".into()),
            notes: vec!["paired verification clean".into()],
        });
        bundle.raw_receipt_handle = Some("receipt:store:receipts:123".into());
        bundle.trace_ctx = Some(TraceCtx::from_trace_id("trace-ver-001"));
        bundle.attempt_id = Some(AttemptId::new("attempt-family-1"));
        bundle.trial_id = Some(TrialId::new("trial-patched-1"));
        bundle.replay_handle = Some("replay://attempt-family-1".into());
        bundle.claim_ids = vec![ClaimId::new("claim-ver-001")];
        bundle.comparability_snapshot_version = Some("bench-a:cargo:short".into());

        let json = serde_json::to_value(&bundle).unwrap();
        assert_eq!(
            json["question"]["unit_definition"],
            serde_json::json!("paired benchmark run")
        );
        assert_eq!(json["treatment"]["paired_trials"], serde_json::json!(true));
        assert_eq!(
            json["verification_trials"]
                .as_array()
                .expect("verification trials should serialize")
                .len(),
            2
        );
        assert_eq!(
            json["refutation_artifacts"]
                .as_array()
                .expect("refutation artifacts should serialize")
                .len(),
            1
        );
        assert_eq!(
            json["verification_summary"]["promotion_state"]["state"],
            serde_json::json!("eligible")
        );
        assert_eq!(
            json["comparability_snapshot"]["workload_id"],
            serde_json::json!("bench-a")
        );

        let back: EvidenceBundle = serde_json::from_value(json).unwrap();
        assert_eq!(back.question.description, bundle.question.description);
        assert_eq!(back.treatment.description, bundle.treatment.description);
        assert_eq!(back.outcome.description, bundle.outcome.description);
        assert_eq!(back.verification_trials.len(), 2);
        assert_eq!(back.refutation_artifacts.len(), 1);
        assert_eq!(
            back.verification_summary
                .expect("verification summary should roundtrip")
                .lifecycle_state,
            VerificationLifecycleState::Verified
        );
        assert_eq!(
            back.claim_ids[0].as_str(),
            "claim:claim-ver-001",
            "claim bundles should remain first-class serialized artifacts"
        );
    }
}
