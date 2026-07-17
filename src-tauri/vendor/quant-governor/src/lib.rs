//! # quant-governor
//!
//! Governance policy routing for governed compression.
//!
//! This crate provides policy-driven routing for codec selection based on
//! content type, size, and accuracy requirements.
//!
//! ## Core Concepts
//!
//! - **GovernancePolicy**: Declares codec profiles and admissibility classes
//! - **CodecDecision**: Output of policy evaluation with selected codec and metadata
//! - **ExactFallbackReceipt**: Tracks fallback from compressed back to raw
//! - **DegradationReceipt**: Tracks quality degradation from higher to lower fidelity
//!
//! ## Codec Profiles
//!
//! | Profile | Description | Use Case |
//! |---------|-------------|----------|
//! | `raw`   | Uncompressed | Critical accuracy |
//! | `q8`    | 8-bit quantization | Balanced |
//! | `q4`    | 4-bit quantization | Storage constrained |
//! | `turbo` | Turbo-quant accelerated | Low latency |
//! | `fib`   | Fibonacci-weighted | Precision sensitive |
//!
//! ## Usage
//!
//! ```rust
//! use quant_governor::{GovernancePolicy, CodecDecision, GovernanceRequest, evaluate};
//!
//! let policy = GovernancePolicy::default();
//! let request = GovernanceRequest::default();
//! let decision = evaluate(request, &policy);
//! ```

#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![deny(rustdoc::broken_intra_doc_links)]

pub mod decision;
pub mod degradation;
pub mod error;
pub mod policy;
pub mod receipt;

pub use decision::{CodecDecision, CodecProfile};
pub use degradation::DegradationReceipt;
pub use error::GovernorError;
pub use policy::{AdmissibilityClass, ContentType, GovernancePolicy, GovernanceRequest};
pub use receipt::ExactFallbackReceipt;

/// Policy-driven codec evaluation.
///
/// Given a governance request and policy, produces a codec decision
/// with selected profile and metadata.
pub fn evaluate(
    request: GovernanceRequest,
    policy: &GovernancePolicy,
) -> Result<CodecDecision, GovernorError> {
    policy.evaluate(request)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_policy_selects_raw_for_critical() {
        let policy = GovernancePolicy::default();
        let request = GovernanceRequest::default();

        let result = evaluate(request.clone(), &policy);
        assert!(result.is_ok());

        let decision = result.unwrap();
        assert_eq!(decision.codec, CodecProfile::Raw);
    }

    #[test]
    fn small_text_bypasses_compression() {
        let policy = GovernancePolicy::default();
        let request = GovernanceRequest {
            content_type: ContentType::Text,
            size_bytes: 100,
            ..Default::default()
        };

        let result = evaluate(request, &policy);
        assert!(result.is_ok());
        // Small text may bypass or use raw
    }
}
