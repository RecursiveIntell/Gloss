use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Shared publication-status primitive for thin governance/profile surfaces.
///
/// Domain crates may only use the variants they need, but they should not
/// redefine the status family locally because that creates schema drift bait.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceStatus {
    AdvisoryOnly,
    NonAdmitted,
    Degraded,
    HorizonOnly,
}

impl std::fmt::Display for SurfaceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::AdvisoryOnly => "advisory_only",
            Self::NonAdmitted => "non_admitted",
            Self::Degraded => "degraded",
            Self::HorizonOnly => "horizon_only",
        };
        f.write_str(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_wire_status_label() {
        assert_eq!(SurfaceStatus::AdvisoryOnly.to_string(), "advisory_only");
        assert_eq!(SurfaceStatus::NonAdmitted.to_string(), "non_admitted");
        assert_eq!(SurfaceStatus::Degraded.to_string(), "degraded");
        assert_eq!(SurfaceStatus::HorizonOnly.to_string(), "horizon_only");
    }
}
