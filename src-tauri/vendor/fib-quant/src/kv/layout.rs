use serde::{Deserialize, Serialize};

use crate::{digest::json_digest, FibQuantError, Result};

use super::shape::{KvTensorShapeV1, KV_SHAPE_SCHEMA};

pub const KV_LAYOUT_SCHEMA: &str = "fib_quant_kv_cache_layout_v1";
pub const KV_PAGE_GEOMETRY_SCHEMA: &str = "fib_quant_kv_page_geometry_v1";

/// Canonical physical order for flat tensors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvLayoutOrder {
    /// `[batch][layer][kv_head][token][head_dim]`.
    BatchLayerHeadTokenDim,
}

/// KV cache layout declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvCacheLayoutV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Shape schema bound into this layout.
    pub shape_schema_version: String,
    /// Physical order.
    pub order: KvLayoutOrder,
    /// Scalar stride for batch.
    pub batch_stride: u64,
    /// Scalar stride for layer.
    pub layer_stride: u64,
    /// Scalar stride for KV head.
    pub head_stride: u64,
    /// Scalar stride for token.
    pub token_stride: u64,
    /// Scalar stride for head dimension.
    pub dim_stride: u64,
}

impl KvCacheLayoutV1 {
    /// Build the canonical contiguous layout for a shape.
    pub fn canonical(shape: &KvTensorShapeV1) -> Result<Self> {
        shape.validate()?;
        let dim_stride = 1;
        let token_stride = u64::from(shape.head_dim);
        let head_stride = u64::from(shape.tokens) * token_stride;
        let layer_stride = u64::from(shape.kv_heads) * head_stride;
        let batch_stride = u64::from(shape.layers) * layer_stride;
        Ok(Self {
            schema_version: KV_LAYOUT_SCHEMA.into(),
            shape_schema_version: KV_SHAPE_SCHEMA.into(),
            order: KvLayoutOrder::BatchLayerHeadTokenDim,
            batch_stride,
            layer_stride,
            head_stride,
            token_stride,
            dim_stride,
        })
    }

    /// Validate this layout against a logical shape.
    pub fn validate_for_shape(&self, shape: &KvTensorShapeV1) -> Result<()> {
        shape.validate()?;
        if self.schema_version != KV_LAYOUT_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "kv layout schema_version {}, expected {KV_LAYOUT_SCHEMA}",
                self.schema_version
            )));
        }
        if self.shape_schema_version != KV_SHAPE_SCHEMA {
            return Err(FibQuantError::CorruptPayload(
                "kv layout shape schema mismatch".into(),
            ));
        }
        let expected = Self::canonical(shape)?;
        if self != &expected {
            return Err(FibQuantError::CorruptPayload(
                "only canonical contiguous kv layout is supported by the CPU reference codec"
                    .into(),
            ));
        }
        Ok(())
    }

    /// Stable layout digest.
    pub fn digest(&self, shape: &KvTensorShapeV1) -> Result<String> {
        self.validate_for_shape(shape)?;
        json_digest(KV_LAYOUT_SCHEMA, self)
    }
}

/// Fixed-size page geometry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KvPageGeometryV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Number of tokens per encoded page.
    pub tokens_per_page: u32,
    /// Number of logical vectors per encoded block.
    pub vectors_per_block: u32,
    /// Number of channels in each logical vector.
    pub head_dim: u32,
    /// Fixed encoded bytes reserved for each block.
    pub encoded_block_bytes: u32,
    /// Fixed raw f32 bytes per logical vector.
    pub raw_vector_bytes: u32,
}

impl KvPageGeometryV1 {
    /// Build a page geometry for one-vector blocks.
    pub fn new(tokens_per_page: u32, head_dim: u32, encoded_block_bytes: u32) -> Self {
        Self {
            schema_version: KV_PAGE_GEOMETRY_SCHEMA.into(),
            tokens_per_page,
            vectors_per_block: 1,
            head_dim,
            encoded_block_bytes,
            raw_vector_bytes: head_dim.saturating_mul(4),
        }
    }

    /// Validate page geometry for a shape.
    pub fn validate_for_shape(&self, shape: &KvTensorShapeV1) -> Result<()> {
        shape.validate()?;
        if self.schema_version != KV_PAGE_GEOMETRY_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "kv page geometry schema_version {}, expected {KV_PAGE_GEOMETRY_SCHEMA}",
                self.schema_version
            )));
        }
        if self.tokens_per_page == 0 || self.tokens_per_page > shape.tokens {
            return Err(FibQuantError::CorruptPayload(
                "tokens_per_page must be in 1..=shape.tokens".into(),
            ));
        }
        if self.vectors_per_block != 1 {
            return Err(FibQuantError::DependencyUnsupported(
                "CPU reference codec currently supports one vector per block".into(),
            ));
        }
        if self.head_dim != shape.head_dim {
            return Err(FibQuantError::CorruptPayload(
                "page geometry head_dim must match shape".into(),
            ));
        }
        if self.raw_vector_bytes != self.head_dim.saturating_mul(4) {
            return Err(FibQuantError::CorruptPayload(
                "raw_vector_bytes must equal head_dim * sizeof(f32)".into(),
            ));
        }
        if self.encoded_block_bytes == 0 {
            return Err(FibQuantError::CorruptPayload(
                "encoded_block_bytes must be nonzero".into(),
            ));
        }
        Ok(())
    }

    /// Number of token pages in a shape.
    pub fn page_count(&self, shape: &KvTensorShapeV1) -> Result<u32> {
        self.validate_for_shape(shape)?;
        Ok(shape.tokens.div_ceil(self.tokens_per_page))
    }

    /// Stable digest.
    pub fn digest(&self, shape: &KvTensorShapeV1) -> Result<String> {
        self.validate_for_shape(shape)?;
        json_digest(KV_PAGE_GEOMETRY_SCHEMA, self)
    }
}
