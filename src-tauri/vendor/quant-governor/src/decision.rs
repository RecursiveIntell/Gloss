//! Codec decision types and profile definitions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::degradation::DegradationReceipt;
use crate::receipt::ExactFallbackReceipt;

/// Codec profiles available for governance selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CodecProfile {
    /// Uncompressed representation
    Raw,
    /// 8-bit quantization
    Q8,
    /// 4-bit quantization
    Q4,
    /// Turbo-quant accelerated codec (symmetric; reconstructs original vector)
    Turbo,
    /// Fibonacci-weighted precision codec (symmetric; reconstructs original vector)
    Fib,
    /// Polar-only quantization (asymmetric; score_inner_product / score_l2 only)
    Polar,
    /// QJL random-projection sketch (asymmetric; fixed-size inner-product estimator)
    Qjl,
}

impl CodecProfile {
    /// Returns the default degradation threshold for this profile.
    pub fn default_degradation_threshold(&self) -> f64 {
        match self {
            CodecProfile::Raw => 0.0,
            CodecProfile::Q8 => 0.05,
            CodecProfile::Q4 => 0.10,
            CodecProfile::Turbo => 0.08,
            CodecProfile::Fib => 0.03,
            CodecProfile::Polar => 0.06,
            CodecProfile::Qjl => 0.04,
        }
    }

    /// Returns true if this is a high-fidelity profile.
    pub fn is_high_fidelity(&self) -> bool {
        matches!(self, CodecProfile::Raw | CodecProfile::Fib)
    }

    /// Returns true if this codec is asymmetric (cannot reconstruct the
    /// original vector from compressed bytes alone). For asymmetric
    /// codecs the wire format is a sketch/code, and the caller must
    /// use score_inner_product / score_l2 against a known query.
    pub fn is_asymmetric(&self) -> bool {
        matches!(self, CodecProfile::Polar | CodecProfile::Qjl)
    }

    /// Returns estimated compression ratio for this profile.
    pub fn estimated_compression_ratio(&self) -> f64 {
        match self {
            CodecProfile::Raw => 1.0,
            CodecProfile::Q8 => 2.0,
            CodecProfile::Q4 => 4.0,
            CodecProfile::Turbo => 3.0,
            CodecProfile::Fib => 2.5,
            // Polar: dim * 2 bytes for radii + angle_indices, plus JSON
            // envelope overhead; rough estimate is dim * 2 / (dim * 4)
            // = 0.5. Use 1.0 because it's not a storage win.
            CodecProfile::Polar => 1.0,
            // QJL: 32 projections * ~3 bytes each ≈ 120 bytes regardless
            // of dim. So compression ratio scales linearly with dim.
            // For 768-dim raw = 3072, qjl = 120, ratio ≈ 25.6.
            // We report a conservative 8.0 to avoid overpromising.
            CodecProfile::Qjl => 8.0,
        }
    }
}

impl std::fmt::Display for CodecProfile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CodecProfile::Raw => write!(f, "raw"),
            CodecProfile::Q8 => write!(f, "q8"),
            CodecProfile::Q4 => write!(f, "q4"),
            CodecProfile::Turbo => write!(f, "turbo"),
            CodecProfile::Fib => write!(f, "fib"),
            CodecProfile::Polar => write!(f, "polar"),
            CodecProfile::Qjl => write!(f, "qjl"),
        }
    }
}

/// Outcome of policy evaluation containing selected codec and metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodecDecision {
    /// Selected codec profile
    pub codec: CodecProfile,

    /// True if exact fallback was triggered (compressed to raw)
    pub exact_fallback: bool,

    /// Allowed degradation budget for this decision
    pub degradation_budget: f64,

    /// Receipt containing detailed tracking information
    pub receipt: CodecReceipt,
}

/// Receipt variants for codec decisions.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CodecReceipt {
    /// Exact fallback receipt when degrading from compressed to raw
    ExactFallback(ExactFallbackReceipt),

    /// Degradation receipt when moving between non-raw profiles
    Degradation(DegradationReceipt),

    /// Direct encoding without fallback
    Direct {
        /// Timestamp of decision
        timestamp: DateTime<Utc>,
        /// Profile selected
        profile: CodecProfile,
    },
}

impl CodecDecision {
    /// Create a direct codec decision without fallback.
    pub fn direct(codec: CodecProfile, degradation_budget: f64) -> Self {
        Self {
            codec,
            exact_fallback: false,
            degradation_budget,
            receipt: CodecReceipt::Direct {
                timestamp: Utc::now(),
                profile: codec,
            },
        }
    }

    /// Create a codec decision with exact fallback.
    pub fn with_exact_fallback(
        codec: CodecProfile,
        degradation_budget: f64,
        receipt: ExactFallbackReceipt,
    ) -> Self {
        Self {
            codec,
            exact_fallback: true,
            degradation_budget,
            receipt: CodecReceipt::ExactFallback(receipt),
        }
    }

    /// Create a codec decision with degradation between profiles.
    pub fn with_degradation(
        codec: CodecProfile,
        degradation_budget: f64,
        receipt: DegradationReceipt,
    ) -> Self {
        Self {
            codec,
            exact_fallback: false,
            degradation_budget,
            receipt: CodecReceipt::Degradation(receipt),
        }
    }

    /// Returns true if this decision involved any form of fallback.
    pub fn had_fallback(&self) -> bool {
        self.exact_fallback || matches!(self.receipt, CodecReceipt::Degradation(_))
    }

    /// Returns the effective profile after any fallback.
    pub fn effective_profile(&self) -> CodecProfile {
        self.codec
    }
}

impl Default for CodecDecision {
    fn default() -> Self {
        Self::direct(CodecProfile::Raw, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codec_profile_ordering() {
        assert!(CodecProfile::Raw.is_high_fidelity());
        assert!(CodecProfile::Fib.is_high_fidelity());
        assert!(!CodecProfile::Q4.is_high_fidelity());
    }

    #[test]
    fn codec_decision_direct() {
        let decision = CodecDecision::direct(CodecProfile::Q8, 0.05);
        assert_eq!(decision.codec, CodecProfile::Q8);
        assert!(!decision.exact_fallback);
        assert!(!decision.had_fallback());
    }

    #[test]
    fn compression_ratios() {
        assert_eq!(CodecProfile::Raw.estimated_compression_ratio(), 1.0);
        assert_eq!(CodecProfile::Q4.estimated_compression_ratio(), 4.0);
        assert_eq!(CodecProfile::Turbo.estimated_compression_ratio(), 3.0);
    }
}
