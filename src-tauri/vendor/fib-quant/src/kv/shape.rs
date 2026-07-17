use serde::{Deserialize, Serialize};

use crate::{digest::json_digest, FibQuantError, Result};

pub const KV_SHAPE_SCHEMA: &str = "fib_quant_kv_tensor_shape_v1";

/// KV tensor role.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvRole {
    /// Attention key cache.
    Key,
    /// Attention value cache.
    Value,
}

/// Key RoPE state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvRopeState {
    /// Key tensor captured before RoPE.
    PreRope,
    /// Key tensor captured after RoPE.
    PostRope,
    /// Value tensors and non-RoPE tensors.
    NotApplicable,
}

/// Attention geometry family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvAttentionKind {
    /// Multi-head attention.
    Mha,
    /// Multi-query attention.
    Mqa,
    /// Grouped-query attention.
    Gqa,
}

/// Source tensor dtype.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvDType {
    /// IEEE fp16 source.
    F16,
    /// bfloat16 source.
    Bf16,
    /// f32 source.
    F32,
}

/// Logical KV tensor shape.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvTensorShapeV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Key or value role.
    pub role: KvRole,
    /// Attention head sharing geometry.
    pub attention_kind: KvAttentionKind,
    /// Batch count.
    pub batch: u32,
    /// Layer count.
    pub layers: u32,
    /// KV head count.
    pub kv_heads: u32,
    /// Query head count.
    pub query_heads: u32,
    /// Token count.
    pub tokens: u32,
    /// Per-head channel dimension.
    pub head_dim: u32,
    /// Source dtype.
    pub dtype: KvDType,
    /// Key RoPE state or not-applicable for values.
    pub rope_state: KvRopeState,
}

impl KvTensorShapeV1 {
    /// Create a shape with the v1 schema marker.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        role: KvRole,
        attention_kind: KvAttentionKind,
        batch: u32,
        layers: u32,
        kv_heads: u32,
        query_heads: u32,
        tokens: u32,
        head_dim: u32,
        dtype: KvDType,
        rope_state: KvRopeState,
    ) -> Self {
        Self {
            schema_version: KV_SHAPE_SCHEMA.into(),
            role,
            attention_kind,
            batch,
            layers,
            kv_heads,
            query_heads,
            tokens,
            head_dim,
            dtype,
            rope_state,
        }
    }

    /// Validate shape invariants that are independent of compression profile.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != KV_SHAPE_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "kv shape schema_version {}, expected {KV_SHAPE_SCHEMA}",
                self.schema_version
            )));
        }
        for (name, value) in [
            ("batch", self.batch),
            ("layers", self.layers),
            ("kv_heads", self.kv_heads),
            ("query_heads", self.query_heads),
            ("tokens", self.tokens),
            ("head_dim", self.head_dim),
        ] {
            if value == 0 {
                return Err(FibQuantError::CorruptPayload(format!(
                    "kv shape {name} must be > 0"
                )));
            }
        }
        match self.attention_kind {
            KvAttentionKind::Mha if self.query_heads != self.kv_heads => {
                return Err(FibQuantError::CorruptPayload(
                    "MHA requires query_heads == kv_heads".into(),
                ));
            }
            KvAttentionKind::Mqa if self.kv_heads != 1 => {
                return Err(FibQuantError::CorruptPayload(
                    "MQA requires kv_heads == 1".into(),
                ));
            }
            KvAttentionKind::Gqa | KvAttentionKind::Mqa
                if self.query_heads % self.kv_heads != 0 =>
            {
                return Err(FibQuantError::CorruptPayload(
                    "query_heads must be divisible by kv_heads".into(),
                ));
            }
            _ => {}
        }
        match (self.role, self.rope_state) {
            (KvRole::Key, KvRopeState::NotApplicable) => {
                return Err(FibQuantError::CorruptPayload(
                    "key tensors must declare pre_rope or post_rope".into(),
                ));
            }
            (KvRole::Value, KvRopeState::PreRope | KvRopeState::PostRope) => {
                return Err(FibQuantError::CorruptPayload(
                    "value tensors must use not_applicable rope state".into(),
                ));
            }
            _ => {}
        }
        let _ = self.element_count()?;
        Ok(())
    }

    /// Validate that the head dimension can be directly compressed by a FibQuant block.
    pub fn validate_block_dim(&self, block_dim: u32) -> Result<()> {
        self.validate()?;
        if block_dim == 0 || self.head_dim % block_dim != 0 {
            return Err(FibQuantError::DimensionNotDivisible {
                ambient_dim: self.head_dim as usize,
                block_dim: block_dim as usize,
            });
        }
        Ok(())
    }

    /// Number of `[batch, layer, kv_head, token]` vectors.
    pub fn vector_count(&self) -> Result<usize> {
        checked_product(&[
            self.batch as usize,
            self.layers as usize,
            self.kv_heads as usize,
            self.tokens as usize,
        ])
    }

    /// Number of f32 scalar values in canonical contiguous form.
    pub fn element_count(&self) -> Result<usize> {
        checked_product(&[self.vector_count()?, self.head_dim as usize])
    }

    /// Stable digest over the explicit shape.
    pub fn digest(&self) -> Result<String> {
        self.validate()?;
        json_digest(KV_SHAPE_SCHEMA, self)
    }
}

pub(crate) fn checked_product(values: &[usize]) -> Result<usize> {
    values.iter().try_fold(1usize, |acc, value| {
        acc.checked_mul(*value)
            .ok_or_else(|| FibQuantError::ResourceLimitExceeded("kv shape size overflow".into()))
    })
}
