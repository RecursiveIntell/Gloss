//! Degradation receipt for quality-based codec transitions.

use serde::{Deserialize, Serialize};

/// Type of degradation applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DegradationType {
    /// Quantization level reduction
    QuantizationStep,
    /// Precision reduction
    Precision,
    /// Speed vs quality trade-off
    SpeedQuality,
    /// Size vs quality trade-off
    SizeQuality,
}

impl std::fmt::Display for DegradationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DegradationType::QuantizationStep => write!(f, "quantization_step"),
            DegradationType::Precision => write!(f, "precision"),
            DegradationType::SpeedQuality => write!(f, "speed_quality"),
            DegradationType::SizeQuality => write!(f, "size_quality"),
        }
    }
}

/// Receipt for degradation decisions (between non-raw profiles).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DegradationReceipt {
    /// Type of degradation applied
    pub degradation_type: DegradationType,

    /// Amount of degradation applied (0.0 to 1.0)
    pub degraded_by: f64,

    /// Estimated bytes saved by degrading
    pub bytes_saved: u64,

    /// Estimated accuracy impact (0.0 to 1.0, higher = more impact)
    pub accuracy_impact: f64,

    /// Source profile before degradation
    pub source_profile: Option<String>,

    /// Target profile after degradation
    pub target_profile: Option<String>,
}

impl DegradationReceipt {
    /// Create a new degradation receipt.
    pub fn new(
        degradation_type: DegradationType,
        degraded_by: f64,
        bytes_saved: u64,
        accuracy_impact: f64,
    ) -> Self {
        Self {
            degradation_type,
            degraded_by,
            bytes_saved,
            accuracy_impact,
            source_profile: None,
            target_profile: None,
        }
    }

    /// Set source and target profiles.
    pub fn with_profiles(mut self, source: impl Into<String>, target: impl Into<String>) -> Self {
        self.source_profile = Some(source.into());
        self.target_profile = Some(target.into());
        self
    }

    /// Returns the benefit-to-cost ratio of this degradation.
    pub fn benefit_cost_ratio(&self) -> f64 {
        if self.accuracy_impact == 0.0 {
            f64::MAX
        } else {
            self.bytes_saved as f64 / (self.accuracy_impact * 1000.0)
        }
    }

    /// Returns true if degradation is within acceptable limits.
    pub fn is_acceptable(&self, max_impact: f64) -> bool {
        self.accuracy_impact <= max_impact
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degradation_receipt_creation() {
        let receipt = DegradationReceipt::new(DegradationType::QuantizationStep, 0.05, 1024, 0.02);

        assert_eq!(receipt.degradation_type, DegradationType::QuantizationStep);
        assert!(receipt.is_acceptable(0.1));
        assert!(!receipt.is_acceptable(0.01));
    }

    #[test]
    fn benefit_cost_ratio() {
        let receipt = DegradationReceipt::new(DegradationType::SizeQuality, 0.1, 5000, 0.05);

        let ratio = receipt.benefit_cost_ratio();
        assert!(ratio > 0.0);
    }
}
