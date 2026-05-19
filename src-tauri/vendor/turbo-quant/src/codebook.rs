//! Small scalar codebook utilities used by quantizer profiles and tests.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::{Result, TurboQuantError};

/// A deterministic uniform scalar codebook over a closed interval.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ScalarCodebook {
    pub bits: u8,
    pub min: f32,
    pub max: f32,
}

impl ScalarCodebook {
    pub fn uniform(bits: u8, min: f32, max: f32) -> Result<Self> {
        if bits == 0 || bits > 16 {
            return Err(TurboQuantError::InvalidBitWidth { got: bits });
        }
        if !min.is_finite() || !max.is_finite() || min >= max {
            return Err(TurboQuantError::MalformedCode {
                reason: "invalid uniform codebook range".into(),
            });
        }
        Ok(Self { bits, min, max })
    }

    pub fn levels(&self) -> u32 {
        1u32 << self.bits
    }

    pub fn quantize(&self, value: f32) -> Result<u16> {
        if !value.is_finite() {
            return Err(TurboQuantError::NonFiniteInput { index: 0 });
        }
        let levels = self.levels();
        let clipped = value.clamp(self.min, self.max);
        let normalized = (clipped - self.min) / (self.max - self.min);
        let index = (normalized * levels as f32).floor() as u32;
        Ok(index.min(levels - 1) as u16)
    }

    pub fn dequantize_midpoint(&self, index: u16) -> Result<f32> {
        let levels = self.levels();
        if u32::from(index) >= levels {
            return Err(TurboQuantError::MalformedCode {
                reason: format!("codebook index {index} is outside [0, {levels})"),
            });
        }
        let width = (self.max - self.min) / levels as f32;
        Ok(self.min + (f32::from(index) + 0.5) * width)
    }
}
