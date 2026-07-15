use serde::{Deserialize, Serialize};

use crate::{codec::FibCodeV1, digest::json_digest, FibQuantError, Result};

pub const KV_BLOCK_SCHEMA: &str = "fib_quant_kv_encoded_block_v1";

/// Encoded block payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KvBlockEncodingV1 {
    /// Raw f32 little-endian canonical vector values.
    RawF32 { values: Vec<f32> },
    /// Existing FibQuant vector artifact.
    FibQuant { code: Box<FibCodeV1> },
}

/// One fixed-addressable KV encoded block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvEncodedBlockV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Block id within the page.
    pub block_id: u32,
    /// Batch index.
    pub batch: u32,
    /// Layer index.
    pub layer: u32,
    /// KV head index.
    pub kv_head: u32,
    /// Token index.
    pub token: u32,
    /// Number of source vectors in this block.
    pub vector_count: u32,
    /// Fixed encoded byte reservation for random access.
    pub fixed_size_bytes: u32,
    /// Whether this block is raw fallback/protected.
    pub raw_fallback: bool,
    /// Human-readable fallback or compression reason.
    pub reason: String,
    /// Payload.
    pub encoding: KvBlockEncodingV1,
}

impl KvEncodedBlockV1 {
    /// Build a raw block.
    #[allow(clippy::too_many_arguments)]
    pub fn raw(
        block_id: u32,
        batch: u32,
        layer: u32,
        kv_head: u32,
        token: u32,
        values: Vec<f32>,
        fixed_size_bytes: u32,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: KV_BLOCK_SCHEMA.into(),
            block_id,
            batch,
            layer,
            kv_head,
            token,
            vector_count: 1,
            fixed_size_bytes,
            raw_fallback: true,
            reason: reason.into(),
            encoding: KvBlockEncodingV1::RawF32 { values },
        }
    }

    /// Build a compressed block.
    #[allow(clippy::too_many_arguments)]
    pub fn fib_quant(
        block_id: u32,
        batch: u32,
        layer: u32,
        kv_head: u32,
        token: u32,
        code: FibCodeV1,
        fixed_size_bytes: u32,
        reason: impl Into<String>,
    ) -> Self {
        Self {
            schema_version: KV_BLOCK_SCHEMA.into(),
            block_id,
            batch,
            layer,
            kv_head,
            token,
            vector_count: 1,
            fixed_size_bytes,
            raw_fallback: false,
            reason: reason.into(),
            encoding: KvBlockEncodingV1::FibQuant {
                code: Box::new(code),
            },
        }
    }

    /// Validate block invariants.
    pub fn validate(&self, head_dim: u32) -> Result<()> {
        if self.schema_version != KV_BLOCK_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "kv block schema_version {}, expected {KV_BLOCK_SCHEMA}",
                self.schema_version
            )));
        }
        if self.vector_count != 1 {
            return Err(FibQuantError::CorruptPayload(
                "kv block vector_count must be 1".into(),
            ));
        }
        if self.fixed_size_bytes == 0 {
            return Err(FibQuantError::CorruptPayload(
                "kv block fixed_size_bytes must be nonzero".into(),
            ));
        }
        match &self.encoding {
            KvBlockEncodingV1::RawF32 { values } => {
                if !self.raw_fallback {
                    return Err(FibQuantError::CorruptPayload(
                        "raw block must set raw_fallback".into(),
                    ));
                }
                if values.len() != head_dim as usize {
                    return Err(FibQuantError::CorruptPayload(
                        "raw kv block head_dim mismatch".into(),
                    ));
                }
                if values.iter().any(|value| !value.is_finite()) {
                    return Err(FibQuantError::CorruptPayload(
                        "raw kv block contains non-finite value".into(),
                    ));
                }
            }
            KvBlockEncodingV1::FibQuant { code } => {
                if self.raw_fallback {
                    return Err(FibQuantError::CorruptPayload(
                        "compressed block cannot set raw_fallback".into(),
                    ));
                }
                if code.ambient_dim != head_dim {
                    return Err(FibQuantError::CorruptPayload(
                        "fib kv block ambient_dim mismatch".into(),
                    ));
                }
            }
        }
        Ok(())
    }

    /// Stable digest for this block.
    pub fn digest(&self, head_dim: u32) -> Result<String> {
        self.validate(head_dim)?;
        json_digest(KV_BLOCK_SCHEMA, self)
    }
}
