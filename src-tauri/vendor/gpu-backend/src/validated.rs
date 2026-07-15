//! Validated types for GPU quantization configuration (QUANT-001).
//!
//! These newtypes enforce structural invariants at construction time so that
//! downstream GPU/CPU code cannot panic on malformed inputs:
//! - `TensorShape`: checked `n * dim` multiplication, non-zero dimensions
//! - `QuantProfile`: validated `k`, `n_levels` (≤256 for u8 indices), `bits_per_index`
//!
//! All arithmetic uses checked operations; overflow returns `Err(GpuError::InvalidConfig)`.

use crate::error::GpuError;

/// Validated tensor shape: `n` vectors of `dim` dimensions.
///
/// Guarantees:
/// - `n > 0` and `dim > 0`
/// - `n * dim` does not overflow `usize`
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TensorShape {
    pub n: usize,
    pub dim: usize,
    /// Pre-computed `n * dim` (guaranteed non-overflowing)
    pub total: usize,
}

impl TensorShape {
    /// Create a validated tensor shape.
    ///
    /// Returns `Err` if:
    /// - `n == 0` or `dim == 0`
    /// - `n * dim` overflows `usize`
    pub fn new(n: usize, dim: usize) -> Result<Self, GpuError> {
        if n == 0 {
            return Err(GpuError::InvalidConfig("n must be > 0".into()));
        }
        if dim == 0 {
            return Err(GpuError::InvalidConfig("dim must be > 0".into()));
        }
        let total = n
            .checked_mul(dim)
            .ok_or_else(|| GpuError::InvalidConfig(format!("n * dim overflow: {n} * {dim}")))?;
        Ok(Self { n, dim, total })
    }

    /// Validate that a data slice matches this shape.
    pub fn validate_data(&self, data: &[f32]) -> Result<(), GpuError> {
        if data.len() != self.total {
            return Err(GpuError::DimensionMismatch {
                expected: self.total,
                got: data.len(),
            });
        }
        Ok(())
    }

    /// Number of blocks of size `k` per vector.
    /// Returns `Err` if `dim % k != 0` or `k == 0`.
    pub fn blocks_per_vector(&self, k: usize) -> Result<usize, GpuError> {
        if k == 0 {
            return Err(GpuError::InvalidConfig("k must be > 0".into()));
        }
        if self.dim % k != 0 {
            return Err(GpuError::InvalidConfig(format!(
                "dim ({}) must be divisible by k ({})",
                self.dim, k
            )));
        }
        Ok(self.dim / k)
    }
}

/// Validated quantization profile for Lloyd-Max / codebook quantization.
///
/// Guarantees:
/// - `k > 0` (block size)
/// - `n_levels > 0 && n_levels <= 256` (fits in u8 index)
/// - `dim % k == 0` (blocks divide evenly)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QuantProfile {
    pub k: usize,
    pub n_levels: usize,
    pub bits_per_index: usize,
}

impl QuantProfile {
    /// Create a validated quantization profile.
    ///
    /// Returns `Err` if:
    /// - `k == 0`
    /// - `n_levels == 0` or `n_levels > 256` (u8 index overflow)
    /// - `bits_per_index == 0` or `bits_per_index > 8`
    pub fn new(k: usize, n_levels: usize, bits_per_index: usize) -> Result<Self, GpuError> {
        if k == 0 {
            return Err(GpuError::InvalidConfig("k must be > 0".into()));
        }
        if n_levels == 0 {
            return Err(GpuError::InvalidConfig("n_levels must be > 0".into()));
        }
        if n_levels > 256 {
            return Err(GpuError::InvalidConfig(format!(
                "n_levels must be <= 256 (u8 index), got {n_levels}"
            )));
        }
        if bits_per_index == 0 || bits_per_index > 8 {
            return Err(GpuError::InvalidConfig(format!(
                "bits_per_index must be 1-8, got {bits_per_index}"
            )));
        }
        // bits_per_index must be sufficient to represent n_levels
        let min_bits = if n_levels <= 1 {
            1
        } else {
            (n_levels - 1).ilog2() as usize + 1
        };
        if bits_per_index < min_bits {
            return Err(GpuError::InvalidConfig(format!(
                "bits_per_index ({bits_per_index}) too small for n_levels ({n_levels}); need >= {min_bits}"
            )));
        }
        Ok(Self {
            k,
            n_levels,
            bits_per_index,
        })
    }

    /// Validate that a norms slice has the correct length for `shape`.
    pub fn validate_norms(&self, shape: &TensorShape, norms: &[f32]) -> Result<(), GpuError> {
        let blocks = shape.blocks_per_vector(self.k)?;
        let expected = shape.n * blocks;
        if norms.len() != expected {
            return Err(GpuError::DimensionMismatch {
                expected,
                got: norms.len(),
            });
        }
        Ok(())
    }

    /// Validate that an indices slice has the correct length for `shape`.
    pub fn validate_indices(&self, shape: &TensorShape, indices: &[u8]) -> Result<(), GpuError> {
        let blocks = shape.blocks_per_vector(self.k)?;
        let expected = shape.n * blocks * self.k;
        if indices.len() != expected {
            return Err(GpuError::DimensionMismatch {
                expected,
                got: indices.len(),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tensor_shape_valid() {
        let s = TensorShape::new(100, 128).unwrap();
        assert_eq!(s.n, 100);
        assert_eq!(s.dim, 128);
        assert_eq!(s.total, 12800);
    }

    #[test]
    fn tensor_shape_zero_n_rejected() {
        assert!(TensorShape::new(0, 128).is_err());
    }

    #[test]
    fn tensor_shape_zero_dim_rejected() {
        assert!(TensorShape::new(100, 0).is_err());
    }

    #[test]
    fn tensor_shape_overflow_rejected() {
        assert!(TensorShape::new(usize::MAX, 2).is_err());
    }

    #[test]
    fn tensor_shape_blocks_per_vector() {
        let s = TensorShape::new(10, 128).unwrap();
        assert_eq!(s.blocks_per_vector(4).unwrap(), 32);
    }

    #[test]
    fn tensor_shape_blocks_k_zero_rejected() {
        let s = TensorShape::new(10, 128).unwrap();
        assert!(s.blocks_per_vector(0).is_err());
    }

    #[test]
    fn tensor_shape_blocks_not_divisible_rejected() {
        let s = TensorShape::new(10, 127).unwrap();
        assert!(s.blocks_per_vector(4).is_err());
    }

    #[test]
    fn quant_profile_valid() {
        let p = QuantProfile::new(4, 16, 4).unwrap();
        assert_eq!(p.k, 4);
        assert_eq!(p.n_levels, 16);
        assert_eq!(p.bits_per_index, 4);
    }

    #[test]
    fn quant_profile_k_zero_rejected() {
        assert!(QuantProfile::new(0, 16, 4).is_err());
    }

    #[test]
    fn quant_profile_n_levels_zero_rejected() {
        assert!(QuantProfile::new(4, 0, 4).is_err());
    }

    #[test]
    fn quant_profile_n_levels_over_256_rejected() {
        assert!(QuantProfile::new(4, 257, 8).is_err());
    }

    #[test]
    fn quant_profile_bits_zero_rejected() {
        assert!(QuantProfile::new(4, 16, 0).is_err());
    }

    #[test]
    fn quant_profile_bits_over_8_rejected() {
        assert!(QuantProfile::new(4, 256, 9).is_err());
    }

    #[test]
    fn quant_profile_bits_insufficient_for_n_levels_rejected() {
        // 16 levels need 4 bits; 3 bits is insufficient
        assert!(QuantProfile::new(4, 16, 3).is_err());
    }

    #[test]
    fn quant_profile_256_levels_ok() {
        assert!(QuantProfile::new(4, 256, 8).is_ok());
    }
}
