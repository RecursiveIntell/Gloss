use serde::{Deserialize, Serialize};

use crate::error::{PolyKvError, Result};

/// The type of attention mechanism in the transformer model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AttentionType {
    /// Multi-Head Attention: num_kv_heads == num_heads
    MHA,
    /// Multi-Query Attention: num_kv_heads == 1
    MQA,
    /// Grouped-Query Attention: 1 < num_kv_heads < num_heads
    GQA,
}

/// Describes the tensor shape of a KV cache for a specific model family.
///
/// This is the logical layout, not the physical memory layout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvTensorShape {
    /// The attention mechanism type.
    pub attention_type: AttentionType,
    /// Number of transformer layers.
    pub num_layers: u32,
    /// Number of attention heads.
    pub num_heads: u32,
    /// Number of key/value heads (for GQA: smaller than num_heads).
    pub num_kv_heads: u32,
    /// Dimension of each attention head (usually 64 or 128).
    pub head_dim: usize,
    /// Hidden size / model dimension (d_model).
    pub hidden_size: usize,
}

impl KvTensorShape {
    /// Validate that the shape is internally consistent.
    pub fn validate(&self) -> Result<()> {
        if self.num_layers == 0 {
            return Err(PolyKvError::InvalidShape("num_layers must be > 0".into()));
        }
        if self.num_heads == 0 {
            return Err(PolyKvError::InvalidShape("num_heads must be > 0".into()));
        }
        if self.num_kv_heads == 0 {
            return Err(PolyKvError::InvalidShape("num_kv_heads must be > 0".into()));
        }
        if self.head_dim == 0 {
            return Err(PolyKvError::InvalidShape("head_dim must be > 0".into()));
        }
        if self.hidden_size == 0 {
            return Err(PolyKvError::InvalidShape("hidden_size must be > 0".into()));
        }

        match self.attention_type {
            AttentionType::MHA => {
                if self.num_kv_heads != self.num_heads {
                    return Err(PolyKvError::InvalidShape(format!(
                        "MHA requires num_kv_heads ({}) == num_heads ({})",
                        self.num_kv_heads, self.num_heads
                    )));
                }
            }
            AttentionType::MQA => {
                if self.num_kv_heads != 1 {
                    return Err(PolyKvError::InvalidShape(format!(
                        "MQA requires num_kv_heads == 1, got {}",
                        self.num_kv_heads
                    )));
                }
            }
            AttentionType::GQA => {
                if self.num_kv_heads >= self.num_heads {
                    return Err(PolyKvError::InvalidShape(format!(
                        "GQA requires num_kv_heads ({}) < num_heads ({})",
                        self.num_kv_heads, self.num_heads
                    )));
                }
            }
        }

        // d_model must be a multiple of num_heads
        if self.hidden_size % self.num_heads as usize != 0 {
            return Err(PolyKvError::InvalidShape(format!(
                "hidden_size ({}) must be divisible by num_heads ({})",
                self.hidden_size, self.num_heads
            )));
        }

        Ok(())
    }

    /// Total KV elements per token (key + value) for one layer.
    pub fn kv_elements_per_token_per_layer(&self) -> usize {
        self.num_kv_heads as usize * self.head_dim * 2 // key + value
    }

    /// Total KV bytes per token (f32) for one layer.
    pub fn kv_bytes_per_token_per_layer(&self) -> usize {
        self.kv_elements_per_token_per_layer() * 4
    }

    /// Total KV elements for N tokens across all layers.
    pub fn total_kv_elements(&self, num_tokens: usize) -> usize {
        self.num_layers as usize * num_tokens * self.kv_elements_per_token_per_layer()
    }

    /// Total raw bytes for N tokens across all layers.
    pub fn total_kv_bytes(&self, num_tokens: usize) -> usize {
        self.total_kv_elements(num_tokens) * 4
    }

    /// Dimension for a single key or value vector for one KV head.
    pub fn head_vector_dim(&self) -> usize {
        self.head_dim
    }

    /// Number of KV head vectors per token per layer.
    pub fn kv_vectors_per_token_per_layer(&self) -> usize {
        self.num_kv_heads as usize * 2 // one key + one value per KV head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_mha_shape() -> KvTensorShape {
        KvTensorShape {
            attention_type: AttentionType::MHA,
            num_layers: 32,
            num_heads: 32,
            num_kv_heads: 32,
            head_dim: 128,
            hidden_size: 4096,
        }
    }

    #[test]
    fn test_mha_shape_validates() {
        let shape = valid_mha_shape();
        assert!(shape.validate().is_ok());
    }

    #[test]
    fn test_gqa_shape_validates() {
        let shape = KvTensorShape {
            attention_type: AttentionType::GQA,
            num_layers: 40,
            num_heads: 32,
            num_kv_heads: 8,
            head_dim: 128,
            hidden_size: 4096,
        };
        assert!(shape.validate().is_ok());
    }

    #[test]
    fn test_mqa_shape_validates() {
        let shape = KvTensorShape {
            attention_type: AttentionType::MQA,
            num_layers: 32,
            num_heads: 32,
            num_kv_heads: 1,
            head_dim: 128,
            hidden_size: 4096,
        };
        assert!(shape.validate().is_ok());
    }

    #[test]
    fn test_ragged_shape_rejected() {
        let mut shape = valid_mha_shape();
        shape.num_kv_heads = 16; // MHA mismatch
        assert!(shape.validate().is_err());
    }

    #[test]
    fn test_zero_layers_rejected() {
        let mut shape = valid_mha_shape();
        shape.num_layers = 0;
        assert!(shape.validate().is_err());
    }
}
