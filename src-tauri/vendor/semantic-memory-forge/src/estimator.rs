//! Estimator and sidecar execution metadata.
//!
//! Records full methodological metadata for estimators and refuters,
//! including Python sidecar discipline when applicable.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Kind of estimator used.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EstimatorKind {
    /// Difference-in-differences.
    DiffInDiff,
    /// Propensity score matching.
    PropensityScore,
    /// Instrumental variables.
    InstrumentalVariables,
    /// Ordinary least squares.
    OLS,
    /// Bayesian estimation.
    Bayesian,
    /// Simple before/after comparison.
    BeforeAfter,
    /// Custom estimator.
    Custom(String),
}

impl EstimatorKind {
    /// Returns the stable wire-format label for the estimator kind.
    pub fn as_str(&self) -> &str {
        match self {
            Self::DiffInDiff => "diff_in_diff",
            Self::PropensityScore => "propensity_score",
            Self::InstrumentalVariables => "instrumental_variables",
            Self::OLS => "ols",
            Self::Bayesian => "bayesian",
            Self::BeforeAfter => "before_after",
            Self::Custom(s) => s,
        }
    }
}

/// Metadata about an estimator or refuter invocation.
///
/// Captures everything needed to reproduce or audit the estimation.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EstimatorMeta {
    /// The kind of estimator.
    pub kind: EstimatorKind,
    /// Version of the estimator (semver or commit hash).
    pub version: String,
    /// Parameters passed to the estimator.
    pub parameters: serde_json::Value,
    /// Random seed, if applicable for reproducibility.
    pub random_seed: Option<u64>,
    /// Environment fingerprint for the execution.
    pub environment: Option<EnvironmentFingerprint>,
    /// Timeout applied to the execution.
    pub timeout_secs: Option<u64>,
    /// How the estimator failed, if it did.
    pub failure_mode: Option<String>,
    /// Versioned request schema identifier.
    pub request_schema_version: Option<String>,
    /// Versioned response schema identifier.
    pub response_schema_version: Option<String>,
}

/// Fingerprint of the execution environment.
///
/// Used to detect environment drift that could affect reproducibility.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentFingerprint {
    /// Python version (if sidecar).
    pub python_version: Option<String>,
    /// Key package versions (e.g., {"dowhy": "0.11", "numpy": "1.26"}).
    pub package_versions: serde_json::Value,
    /// OS / platform identifier.
    pub platform: Option<String>,
    /// Hash of the full environment specification (e.g., pip freeze hash).
    pub env_hash: Option<String>,
}

/// Record of a sidecar execution (e.g., Python estimation/refutation).
///
/// Preserves the full request/response chain for audit and replay.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SidecarExecution {
    /// Estimator metadata.
    pub estimator: EstimatorMeta,
    /// The request payload sent to the sidecar.
    pub request: serde_json::Value,
    /// The response payload received from the sidecar.
    pub response: Option<serde_json::Value>,
    /// Duration of the execution in milliseconds.
    pub duration_ms: Option<u64>,
    /// Whether the execution succeeded.
    pub success: bool,
    /// Error message if the execution failed.
    pub error: Option<String>,
    /// When the execution started.
    pub started_at: String,
    /// When the execution completed.
    pub completed_at: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimator_meta_serde() {
        let meta = EstimatorMeta {
            kind: EstimatorKind::DiffInDiff,
            version: "1.0.0".into(),
            parameters: serde_json::json!({"method": "linear"}),
            random_seed: Some(42),
            environment: Some(EnvironmentFingerprint {
                python_version: Some("3.11".into()),
                package_versions: serde_json::json!({"dowhy": "0.11"}),
                platform: Some("linux-x86_64".into()),
                env_hash: None,
            }),
            timeout_secs: Some(300),
            failure_mode: None,
            request_schema_version: Some("v1".into()),
            response_schema_version: Some("v1".into()),
        };

        let json = serde_json::to_string(&meta).unwrap();
        let back: EstimatorMeta = serde_json::from_str(&json).unwrap();
        assert_eq!(back.version, "1.0.0");
        assert_eq!(back.random_seed, Some(42));
    }

    #[test]
    fn sidecar_execution_serde() {
        let exec = SidecarExecution {
            estimator: EstimatorMeta {
                kind: EstimatorKind::PropensityScore,
                version: "2.0.0".into(),
                parameters: serde_json::json!({}),
                random_seed: None,
                environment: None,
                timeout_secs: Some(60),
                failure_mode: None,
                request_schema_version: None,
                response_schema_version: None,
            },
            request: serde_json::json!({"data": [1, 2, 3]}),
            response: Some(serde_json::json!({"estimate": 0.5})),
            duration_ms: Some(1500),
            success: true,
            error: None,
            started_at: "2024-01-01T00:00:00Z".into(),
            completed_at: Some("2024-01-01T00:00:01Z".into()),
        };

        let json = serde_json::to_string(&exec).unwrap();
        let back: SidecarExecution = serde_json::from_str(&json).unwrap();
        assert!(back.success);
        assert_eq!(back.duration_ms, Some(1500));
    }
}
