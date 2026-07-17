//! Claim-bounded receipt types and invariant scoring for the hostile memory benchmark.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricThresholds {
    pub minimum_pass_rate: f64,
    pub maximum_stale_retrievals: u64,
    pub maximum_unsupported_admissions: u64,
    pub maximum_namespace_leakage: u64,
    pub require_replay_equivalence: bool,
}

impl MetricThresholds {
    /// Thresholds declared before the benchmark event stream is executed.
    pub fn declared() -> Self {
        Self {
            minimum_pass_rate: 1.0,
            maximum_stale_retrievals: 0,
            maximum_unsupported_admissions: 0,
            maximum_namespace_leakage: 0,
            require_replay_equivalence: true,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    Pass,
    Fail,
    NotTested,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioReceipt {
    pub name: String,
    pub status: ScenarioStatus,
    pub latency_us: u64,
    pub detail: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MetricCounts {
    pub stale_retrievals: u64,
    pub unsupported_admissions: u64,
    pub contradictions_preserved: u64,
    pub temporal_correct: u64,
    pub namespace_leakage: u64,
    pub replay_equivalent: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReceiptSummary {
    pub tested: u64,
    pub passed: u64,
    pub failed: u64,
    pub not_tested: u64,
    pub pass_rate: f64,
    pub thresholds_met: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchmarkReceipt {
    pub schema_version: String,
    pub subject: String,
    pub thresholds: MetricThresholds,
    pub scenarios: Vec<ScenarioReceipt>,
    pub metrics: MetricCounts,
    pub summary: ReceiptSummary,
}

impl BenchmarkReceipt {
    pub fn new(subject: impl Into<String>, thresholds: MetricThresholds) -> Self {
        Self {
            schema_version: "hostile-memory-benchmark-v1".into(),
            subject: subject.into(),
            thresholds,
            scenarios: Vec::new(),
            metrics: MetricCounts::default(),
            summary: ReceiptSummary::default(),
        }
    }
}

/// Recomputes summary fields; `not_tested` is never counted as a pass.
pub fn evaluate_receipt(receipt: &mut BenchmarkReceipt) {
    let passed = receipt
        .scenarios
        .iter()
        .filter(|s| s.status == ScenarioStatus::Pass)
        .count() as u64;
    let failed = receipt
        .scenarios
        .iter()
        .filter(|s| s.status == ScenarioStatus::Fail)
        .count() as u64;
    let not_tested = receipt
        .scenarios
        .iter()
        .filter(|s| s.status == ScenarioStatus::NotTested)
        .count() as u64;
    let tested = passed + failed;
    let pass_rate = if tested == 0 {
        0.0
    } else {
        passed as f64 / tested as f64
    };
    receipt.summary = ReceiptSummary {
        tested,
        passed,
        failed,
        not_tested,
        pass_rate,
        thresholds_met: tested > 0
            && pass_rate >= receipt.thresholds.minimum_pass_rate
            && receipt.metrics.stale_retrievals <= receipt.thresholds.maximum_stale_retrievals
            && receipt.metrics.unsupported_admissions
                <= receipt.thresholds.maximum_unsupported_admissions
            && receipt.metrics.namespace_leakage <= receipt.thresholds.maximum_namespace_leakage
            && (!receipt.thresholds.require_replay_equivalence
                || receipt.metrics.replay_equivalent),
    };
}
