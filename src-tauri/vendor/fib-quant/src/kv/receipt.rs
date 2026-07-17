use serde::{Deserialize, Serialize};

use crate::{digest::bytes_digest, FibQuantError, Result};

use super::quality::KvAttentionQualityReportV1;

pub const KV_RECEIPT_SCHEMA: &str = "fib_quant_kv_receipt_v1";

/// KV operation kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum KvOperationKindV1 {
    /// Compression/encode.
    Compress,
    /// Decode.
    Decode,
    /// Quality/evaluation.
    Eval,
}

/// Compression receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvCompressionReceiptV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Operation kind.
    pub operation_kind: KvOperationKindV1,
    /// Source tensor digest.
    pub source_digest: String,
    /// KV profile digest.
    pub profile_digest: String,
    /// Shape digest.
    pub shape_digest: String,
    /// Page digests.
    pub page_digests: Vec<String>,
    /// Codebook digest.
    pub codebook_digest: String,
    /// Rotation digest.
    pub rotation_digest: String,
    /// Number of encoded pages.
    pub encoded_pages: u32,
    /// Number of compressed blocks.
    pub compressed_blocks: u32,
    /// Number of raw fallback blocks.
    pub raw_fallback_blocks: u32,
    /// Fallback/degradation reasons.
    pub fallback_reasons: Vec<String>,
    /// Recorded Unix time in seconds.
    pub recorded_unix_seconds: i64,
}

/// Decode receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvDecodeReceiptV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Operation kind.
    pub operation_kind: KvOperationKindV1,
    /// Reconstructed tensor digest.
    pub decoded_digest: String,
    /// KV profile digest.
    pub profile_digest: String,
    /// Shape digest.
    pub shape_digest: String,
    /// Page digests verified during decode.
    pub page_digests: Vec<String>,
    /// Codebook digest.
    pub codebook_digest: String,
    /// Rotation digest.
    pub rotation_digest: String,
    /// Number of pages decoded.
    pub decoded_pages: u32,
    /// Number of raw fallback blocks decoded.
    pub raw_fallback_blocks: u32,
    /// Recorded Unix time in seconds.
    pub recorded_unix_seconds: i64,
}

/// Quality/eval receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvEvalReceiptV1 {
    /// Stable schema marker.
    pub schema_version: String,
    /// Operation kind.
    pub operation_kind: KvOperationKindV1,
    /// Source tensor digest.
    pub source_digest: String,
    /// Decoded tensor digest.
    pub decoded_digest: String,
    /// KV profile digest.
    pub profile_digest: String,
    /// Shape digest.
    pub shape_digest: String,
    /// Optional quality metrics.
    pub quality_report: Option<KvAttentionQualityReportV1>,
    /// Recorded Unix time in seconds.
    pub recorded_unix_seconds: i64,
}

impl KvCompressionReceiptV1 {
    pub(crate) fn validate(&self) -> Result<()> {
        if self.schema_version != KV_RECEIPT_SCHEMA
            || self.operation_kind != KvOperationKindV1::Compress
        {
            return Err(FibQuantError::CorruptPayload(
                "invalid kv compression receipt".into(),
            ));
        }
        Ok(())
    }
}

/// Stable digest for a canonical f32 tensor.
pub fn kv_tensor_digest(values: &[f32]) -> Result<String> {
    if values.iter().any(|value| !value.is_finite()) {
        return Err(FibQuantError::CorruptPayload(
            "kv tensor contains non-finite value".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(32 + std::mem::size_of_val(values));
    bytes.extend_from_slice(b"fib_quant_kv_tensor_f32_v1");
    bytes.push(0);
    bytes.extend_from_slice(&(values.len() as u64).to_le_bytes());
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    Ok(bytes_digest(&bytes))
}

pub(crate) fn now_unix_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
