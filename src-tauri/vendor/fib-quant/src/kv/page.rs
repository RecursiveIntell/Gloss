use serde::{Deserialize, Serialize};

use crate::{digest::json_digest, FibQuantError, Result};

use super::{
    block::KvEncodedBlockV1,
    layout::KvPageGeometryV1,
    shape::{KvTensorShapeV1, KV_SHAPE_SCHEMA},
};

pub const KV_PAGE_SCHEMA: &str = "fib_quant_kv_encoded_page_v1";

/// Fixed-size random-access encoded KV page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvEncodedPageV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Page id in token-page order.
    pub page_id: u32,
    /// First token covered by this page.
    pub token_start: u32,
    /// Number of tokens represented.
    pub token_count: u32,
    /// Source tensor digest.
    pub source_tensor_digest: String,
    /// KV compression profile digest.
    pub profile_digest: String,
    /// Shape digest.
    pub shape_digest: String,
    /// Shape schema marker.
    pub shape_schema_version: String,
    /// Page geometry.
    pub page_geometry: KvPageGeometryV1,
    /// Encoded blocks.
    pub encoded_blocks: Vec<KvEncodedBlockV1>,
    /// Count of raw fallback blocks.
    pub raw_fallback_blocks: u32,
    /// Stable digest/checksum for this page.
    pub page_digest: String,
}

impl KvEncodedPageV1 {
    /// Build a page and compute its digest.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        page_id: u32,
        token_start: u32,
        token_count: u32,
        source_tensor_digest: String,
        profile_digest: String,
        shape: &KvTensorShapeV1,
        page_geometry: KvPageGeometryV1,
        encoded_blocks: Vec<KvEncodedBlockV1>,
    ) -> Result<Self> {
        let raw_fallback_blocks = encoded_blocks
            .iter()
            .filter(|block| block.raw_fallback)
            .count() as u32;
        let mut page = Self {
            schema_version: KV_PAGE_SCHEMA.into(),
            page_id,
            token_start,
            token_count,
            source_tensor_digest,
            profile_digest,
            shape_digest: shape.digest()?,
            shape_schema_version: KV_SHAPE_SCHEMA.into(),
            page_geometry,
            encoded_blocks,
            raw_fallback_blocks,
            page_digest: String::new(),
        };
        page.page_digest = page.compute_digest(shape)?;
        Ok(page)
    }

    /// Validate page fields and digest.
    pub fn validate(&self, shape: &KvTensorShapeV1) -> Result<()> {
        if self.schema_version != KV_PAGE_SCHEMA {
            return Err(FibQuantError::CorruptPayload(format!(
                "kv page schema_version {}, expected {KV_PAGE_SCHEMA}",
                self.schema_version
            )));
        }
        shape.validate()?;
        if self.shape_schema_version != KV_SHAPE_SCHEMA || self.shape_digest != shape.digest()? {
            return Err(FibQuantError::CorruptPayload(
                "kv page shape digest mismatch".into(),
            ));
        }
        self.page_geometry.validate_for_shape(shape)?;
        if self.token_count == 0 || self.token_start >= shape.tokens {
            return Err(FibQuantError::CorruptPayload(
                "invalid kv page token span".into(),
            ));
        }
        if self.token_start + self.token_count > shape.tokens {
            return Err(FibQuantError::CorruptPayload(
                "kv page token span exceeds shape tokens".into(),
            ));
        }
        if self.token_count > self.page_geometry.tokens_per_page {
            return Err(FibQuantError::CorruptPayload(
                "kv page token_count exceeds geometry".into(),
            ));
        }
        let expected_raw = self
            .encoded_blocks
            .iter()
            .filter(|block| block.raw_fallback)
            .count() as u32;
        if self.raw_fallback_blocks != expected_raw {
            return Err(FibQuantError::CorruptPayload(
                "kv page raw fallback count mismatch".into(),
            ));
        }
        for (idx, block) in self.encoded_blocks.iter().enumerate() {
            block.validate(shape.head_dim)?;
            if block.block_id as usize != idx {
                return Err(FibQuantError::CorruptPayload(
                    "kv page block ids must be contiguous".into(),
                ));
            }
            if block.token < self.token_start || block.token >= self.token_start + self.token_count
            {
                return Err(FibQuantError::CorruptPayload(
                    "kv page block token outside page span".into(),
                ));
            }
        }
        let expected_digest = self.compute_digest(shape)?;
        if self.page_digest != expected_digest {
            return Err(FibQuantError::CorruptPayload(
                "kv page digest mismatch".into(),
            ));
        }
        Ok(())
    }

    /// Compute page digest excluding the digest field itself.
    pub fn compute_digest(&self, shape: &KvTensorShapeV1) -> Result<String> {
        self.page_geometry.validate_for_shape(shape)?;
        #[derive(Serialize)]
        struct DigestView<'a> {
            schema_version: &'a str,
            page_id: u32,
            token_start: u32,
            token_count: u32,
            source_tensor_digest: &'a str,
            profile_digest: &'a str,
            shape_digest: &'a str,
            shape_schema_version: &'a str,
            page_geometry: &'a KvPageGeometryV1,
            encoded_blocks: &'a [KvEncodedBlockV1],
            raw_fallback_blocks: u32,
        }
        json_digest(
            KV_PAGE_SCHEMA,
            &DigestView {
                schema_version: &self.schema_version,
                page_id: self.page_id,
                token_start: self.token_start,
                token_count: self.token_count,
                source_tensor_digest: &self.source_tensor_digest,
                profile_digest: &self.profile_digest,
                shape_digest: &self.shape_digest,
                shape_schema_version: &self.shape_schema_version,
                page_geometry: &self.page_geometry,
                encoded_blocks: &self.encoded_blocks,
                raw_fallback_blocks: self.raw_fallback_blocks,
            },
        )
    }
}
