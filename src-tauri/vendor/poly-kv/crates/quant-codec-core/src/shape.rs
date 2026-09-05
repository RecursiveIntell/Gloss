use crate::{DType, QuantCodecError};

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum KvRole {
    Key,
    Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum KvLayout {
    LayersHeadsTokensDim,
    LayersTokensHeadsDim,
    RuntimeSpecific(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct LayerId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct HeadId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct TokenSpan {
    pub start: u64,
    pub end: u64,
}

impl TokenSpan {
    pub fn new(start: u64, end: u64) -> Result<Self, QuantCodecError> {
        if start >= end {
            return Err(QuantCodecError::InvalidTokenSpan { start, end });
        }
        Ok(Self { start, end })
    }

    pub fn len(self) -> u64 {
        self.end - self.start
    }

    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub enum KvAttentionKind {
    Mha,
    Mqa,
    Gqa,
    Unsupported(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct KvCacheShapeV2 {
    pub batch: u32,
    pub layers: u32,
    pub num_q_heads: u32,
    pub num_kv_heads: u32,
    pub seq_len: u64,
    pub head_dim: u32,
    pub layout: KvLayout,
    pub dtype: DType,
    pub attention_kind: KvAttentionKind,
}

impl KvCacheShapeV2 {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        batch: u32,
        layers: u32,
        num_q_heads: u32,
        num_kv_heads: u32,
        seq_len: u64,
        head_dim: u32,
        layout: KvLayout,
        dtype: DType,
        attention_kind: KvAttentionKind,
    ) -> Result<Self, QuantCodecError> {
        let shape = Self {
            batch,
            layers,
            num_q_heads,
            num_kv_heads,
            seq_len,
            head_dim,
            layout,
            dtype,
            attention_kind,
        };
        shape.validate()?;
        Ok(shape)
    }

    pub fn mha(
        batch: u32,
        layers: u32,
        heads: u32,
        seq_len: u64,
        head_dim: u32,
        layout: KvLayout,
        dtype: DType,
    ) -> Result<Self, QuantCodecError> {
        Self::new(
            batch,
            layers,
            heads,
            heads,
            seq_len,
            head_dim,
            layout,
            dtype,
            KvAttentionKind::Mha,
        )
    }

    pub fn mqa(
        batch: u32,
        layers: u32,
        num_q_heads: u32,
        seq_len: u64,
        head_dim: u32,
        layout: KvLayout,
        dtype: DType,
    ) -> Result<Self, QuantCodecError> {
        Self::new(
            batch,
            layers,
            num_q_heads,
            1,
            seq_len,
            head_dim,
            layout,
            dtype,
            KvAttentionKind::Mqa,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn gqa(
        batch: u32,
        layers: u32,
        num_q_heads: u32,
        num_kv_heads: u32,
        seq_len: u64,
        head_dim: u32,
        layout: KvLayout,
        dtype: DType,
    ) -> Result<Self, QuantCodecError> {
        Self::new(
            batch,
            layers,
            num_q_heads,
            num_kv_heads,
            seq_len,
            head_dim,
            layout,
            dtype,
            KvAttentionKind::Gqa,
        )
    }

    pub fn validate(&self) -> Result<(), QuantCodecError> {
        if self.batch == 0 {
            return Err(invalid_shape("batch must be greater than zero"));
        }
        if self.layers == 0 {
            return Err(invalid_shape("layers must be greater than zero"));
        }
        if self.num_q_heads == 0 {
            return Err(invalid_shape("num_q_heads must be greater than zero"));
        }
        if self.num_kv_heads == 0 {
            return Err(invalid_shape("num_kv_heads must be greater than zero"));
        }
        if self.seq_len == 0 {
            return Err(invalid_shape("seq_len must be greater than zero"));
        }
        if self.head_dim == 0 {
            return Err(invalid_shape("head_dim must be greater than zero"));
        }
        if matches!(&self.layout, KvLayout::RuntimeSpecific(s) if s.is_empty()) {
            return Err(invalid_shape(
                "runtime-specific layout label cannot be empty",
            ));
        }
        match &self.attention_kind {
            KvAttentionKind::Mha => {
                if self.num_q_heads != self.num_kv_heads {
                    return Err(invalid_shape(
                        "MHA requires num_q_heads equal to num_kv_heads",
                    ));
                }
            }
            KvAttentionKind::Mqa => {
                if self.num_kv_heads != 1 || self.num_q_heads <= 1 {
                    return Err(invalid_shape(
                        "MQA requires num_kv_heads == 1 and num_q_heads > 1",
                    ));
                }
            }
            KvAttentionKind::Gqa => {
                if self.num_q_heads <= self.num_kv_heads || self.num_kv_heads <= 1 {
                    return Err(invalid_shape(
                        "GQA requires num_q_heads > num_kv_heads and num_kv_heads > 1",
                    ));
                }
                if self.num_q_heads % self.num_kv_heads != 0 {
                    return Err(invalid_shape(
                        "GQA requires num_q_heads divisible by num_kv_heads",
                    ));
                }
            }
            KvAttentionKind::Unsupported(label) => {
                if label.trim().is_empty() {
                    return Err(invalid_shape("unsupported attention label cannot be empty"));
                }
                return Err(invalid_shape(format!(
                    "unsupported attention kind {label} is not adapter-owned"
                )));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct KvTensorShape {
    pub layers: u32,
    pub key_heads: u32,
    pub value_heads: u32,
    pub seq_len: u64,
    pub head_dim: u32,
    pub layout: KvLayout,
    pub dtype: DType,
}

impl KvTensorShape {
    pub fn gqa(
        layers: u32,
        key_heads: u32,
        value_heads: u32,
        seq_len: u64,
        head_dim: u32,
        layout: KvLayout,
        dtype: DType,
    ) -> Result<Self, QuantCodecError> {
        let shape = Self {
            layers,
            key_heads,
            value_heads,
            seq_len,
            head_dim,
            layout,
            dtype,
        };
        shape.validate()?;
        Ok(shape)
    }

    pub fn validate(&self) -> Result<(), QuantCodecError> {
        if self.layers == 0 {
            return Err(invalid_shape("layers must be greater than zero"));
        }
        if self.key_heads == 0 {
            return Err(invalid_shape("key_heads must be greater than zero"));
        }
        if self.value_heads == 0 {
            return Err(invalid_shape("value_heads must be greater than zero"));
        }
        if self.seq_len == 0 {
            return Err(invalid_shape("seq_len must be greater than zero"));
        }
        if self.head_dim == 0 {
            return Err(invalid_shape("head_dim must be greater than zero"));
        }
        if matches!(&self.layout, KvLayout::RuntimeSpecific(s) if s.is_empty()) {
            return Err(invalid_shape(
                "runtime-specific layout label cannot be empty",
            ));
        }
        Ok(())
    }

    pub fn heads_for(&self, role: KvRole) -> u32 {
        match role {
            KvRole::Key => self.key_heads,
            KvRole::Value => self.value_heads,
        }
    }

    pub fn layer_element_count(&self, role: KvRole) -> Result<usize, QuantCodecError> {
        checked_usize_product(
            &[
                self.heads_for(role) as u64,
                self.seq_len,
                self.head_dim as u64,
            ],
            "layer element count",
        )
    }

    pub fn total_element_count(&self, role: KvRole) -> Result<usize, QuantCodecError> {
        checked_usize_product(
            &[
                self.layers as u64,
                self.heads_for(role) as u64,
                self.seq_len,
                self.head_dim as u64,
            ],
            "total element count",
        )
    }

    pub fn validate_span(&self, span: TokenSpan) -> Result<(), QuantCodecError> {
        if span.is_empty() || span.end > self.seq_len {
            return Err(QuantCodecError::InvalidTokenSpan {
                start: span.start,
                end: span.end,
            });
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct KvSliceRequest {
    pub layer: LayerId,
    pub role: KvRole,
    pub token_span: TokenSpan,
    pub head: Option<HeadId>,
}

impl KvSliceRequest {
    pub fn layer_span(layer: LayerId, token_span: TokenSpan) -> Self {
        Self {
            layer,
            role: KvRole::Key,
            token_span,
            head: None,
        }
    }

    pub fn for_role(mut self, role: KvRole) -> Self {
        self.role = role;
        self
    }

    pub fn for_head(mut self, head: HeadId) -> Self {
        self.head = Some(head);
        self
    }

    pub fn validate_for_shape(&self, shape: &KvTensorShape) -> Result<(), QuantCodecError> {
        shape.validate()?;
        if self.layer.0 >= shape.layers {
            return Err(QuantCodecError::ShapeMismatch {
                reason: format!(
                    "requested layer {} but shape has {} layers",
                    self.layer.0, shape.layers
                ),
            });
        }
        shape.validate_span(self.token_span)?;
        if let Some(head) = self.head {
            let heads = shape.heads_for(self.role);
            if head.0 >= heads {
                return Err(QuantCodecError::ShapeMismatch {
                    reason: format!("requested head {} but role has {} heads", head.0, heads),
                });
            }
        }
        Ok(())
    }
}

fn checked_usize_product(values: &[u64], context: &'static str) -> Result<usize, QuantCodecError> {
    let mut product = 1u64;
    for value in values {
        product = product
            .checked_mul(*value)
            .ok_or(QuantCodecError::IntegerOverflow { context })?;
    }
    usize::try_from(product).map_err(|_| QuantCodecError::IntegerOverflow { context })
}

fn invalid_shape(reason: impl Into<String>) -> QuantCodecError {
    QuantCodecError::InvalidShape {
        reason: reason.into(),
    }
}
