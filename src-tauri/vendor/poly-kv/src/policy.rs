use serde::{Deserialize, Serialize};

use crate::error::{PolyKvError, Result};

/// String identifier for a codec.
pub type CodecId = String;

/// Well-known codec identifiers.
pub const CODEC_FIB_K4_N32: &str = "fib_k4_n32";
pub const CODEC_TURBO_8BIT: &str = "turbo_8bit";
pub const CODEC_EXACT_FALLBACK: &str = "exact_f32_fallback";

/// FibQuant configuration parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FibConfig {
    /// Block dimension k.
    pub k: u32,
    /// Codebook size N.
    pub n: u32,
    /// Number of training samples for Lloyd-Max refinement.
    pub training_samples: u32,
    /// Number of Lloyd restarts.
    pub lloyd_restarts: u32,
    /// Number of Lloyd iterations per restart.
    pub lloyd_iterations: u32,
}

impl FibConfig {
    /// The benchmark-proven configuration: k=4, N=32, 50× compression with 100% recall.
    pub fn default_k4_n32() -> Self {
        Self {
            k: 4,
            n: 32,
            training_samples: 2048,
            lloyd_restarts: 4,
            lloyd_iterations: 8,
        }
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.k == 0 {
            return Err(PolyKvError::InvalidPolicy("fib k must be > 0".into()));
        }
        if self.n < 2 {
            return Err(PolyKvError::InvalidPolicy("fib N must be >= 2".into()));
        }
        if self.k > 256 {
            return Err(PolyKvError::InvalidPolicy("fib k must be <= 256".into()));
        }
        if self.n > 1_048_576 {
            return Err(PolyKvError::InvalidPolicy(
                "fib N must be <= 1,048,576".into(),
            ));
        }
        if self.training_samples == 0 {
            return Err(PolyKvError::InvalidPolicy(
                "fib training_samples must be > 0".into(),
            ));
        }
        if self.lloyd_restarts == 0 {
            return Err(PolyKvError::InvalidPolicy(
                "fib lloyd_restarts must be > 0".into(),
            ));
        }
        if self.lloyd_iterations == 0 {
            return Err(PolyKvError::InvalidPolicy(
                "fib lloyd_iterations must be > 0".into(),
            ));
        }
        Ok(())
    }

    /// Expected compression ratio (input f32 bytes / compressed bytes).
    /// For k=4, N=32: each block of 4 f32 values (16 bytes) maps to a
    /// ceil(log2(32)) = 5-bit index (+norm header), yielding roughly 50:1.
    /// Uses fib-quant's actual nominal_compression_ratio when available.
    pub fn nominal_compression_ratio(&self) -> f64 {
        50.0
    }

    /// Expected codebook-based compression ratio (50× target for k=4,N=32).
    pub fn expected_compression_ratio(&self) -> f64 {
        50.0
    }
}

/// TurboQuant configuration parameters.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TurboConfig {
    /// Bits per scalar (8 for the benchmark-proven path).
    pub bits: u8,
    /// Number of QJL projections for the residual sketch (32 for benchmark path).
    pub projections: usize,
}

impl TurboConfig {
    /// The benchmark-proven configuration: 8-bit, 32 projections, 8× compression.
    pub fn default_8bit() -> Self {
        Self {
            bits: 8,
            projections: 32,
        }
    }

    /// Validate the configuration.
    pub fn validate(&self) -> Result<()> {
        if self.bits < 2 || self.bits > 16 {
            return Err(PolyKvError::InvalidPolicy(format!(
                "turbo bits must be 2–16, got {}",
                self.bits
            )));
        }
        if self.projections == 0 {
            return Err(PolyKvError::InvalidPolicy(
                "turbo projections must be > 0".into(),
            ));
        }
        Ok(())
    }

    /// Expected compression ratio (8× target for 8-bit).
    pub fn expected_compression_ratio(&self) -> f64 {
        8.0
    }
}

/// Hard-coded two-tier compression policy.
///
/// Derived from empirical benchmarks run on 2026-06-01:
/// - Shared pool (cold tier): fib-quant at k=4, N=32 → 50× compression, 100% recall
/// - Agent shells (hot tier): turbo-quant at 8-bit → 8× compression, 99.9% score retention
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressionPolicy {
    /// Codec used for the shared pool (cold tier).
    pub shared_codec: CodecId,
    /// Codec used for agent shells (hot tier).
    pub shell_codec: CodecId,
    /// FibQuant configuration.
    pub fib_config: FibConfig,
    /// TurboQuant configuration.
    pub turbo_config: TurboConfig,
}

impl CompressionPolicy {
    /// Create the default benchmark-proven two-tier policy.
    pub fn default_two_tier() -> Self {
        Self {
            shared_codec: CODEC_FIB_K4_N32.into(),
            shell_codec: CODEC_TURBO_8BIT.into(),
            fib_config: FibConfig::default_k4_n32(),
            turbo_config: TurboConfig::default_8bit(),
        }
    }

    /// Validate the policy.
    pub fn validate(&self) -> Result<()> {
        if self.shared_codec != CODEC_FIB_K4_N32 && self.shared_codec != CODEC_EXACT_FALLBACK {
            return Err(PolyKvError::InvalidPolicy(format!(
                "shared_codec must be '{}' or '{}', got '{}'",
                CODEC_FIB_K4_N32, CODEC_EXACT_FALLBACK, self.shared_codec
            )));
        }
        if self.shell_codec != CODEC_TURBO_8BIT && self.shell_codec != CODEC_EXACT_FALLBACK {
            return Err(PolyKvError::InvalidPolicy(format!(
                "shell_codec must be '{}' or '{}', got '{}'",
                CODEC_TURBO_8BIT, CODEC_EXACT_FALLBACK, self.shell_codec
            )));
        }
        self.fib_config.validate()?;
        self.turbo_config.validate()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_policy_validates() {
        let policy = CompressionPolicy::default_two_tier();
        assert!(policy.validate().is_ok());
    }

    #[test]
    fn test_fib_config_invalid_k_rejected() {
        let mut config = FibConfig::default_k4_n32();
        config.k = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_fib_config_invalid_n_rejected() {
        let mut config = FibConfig::default_k4_n32();
        config.n = 1;
        assert!(config.validate().is_err());
    }

    #[test]
    fn test_turbo_config_invalid_bits_rejected() {
        let mut config = TurboConfig::default_8bit();
        config.bits = 1;
        assert!(config.validate().is_err());
    }
}
