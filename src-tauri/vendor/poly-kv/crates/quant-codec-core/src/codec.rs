use crate::{CodecId, CodecProfileDigest, KvSliceRequest, KvTensorShape};

/// INT-001: Canonical codec profile trait for interchangeable backends.
///
/// All codec backends (turbo-quant, fib-quant, raw, sq8) must implement this
/// trait so they can be swapped without domain code changes.
pub trait CodecProfile {
    fn codec_id(&self) -> CodecId;
    fn codec_version(&self) -> &str;
    fn profile_digest(&self) -> CodecProfileDigest;
    fn fixed_rate_bits(&self) -> Option<u16>;
    fn block_dim(&self) -> Option<u16>;
    fn is_lossy(&self) -> bool;
}

/// INT-001: Typed score semantics for codec comparison.
///
/// Different codecs support different scoring methods. This typed enum
/// prevents accidental cross-codec score comparisons.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum ScoreSemantics {
    /// Exact cosine similarity on decoded f32 vectors.
    CosineOnDecodedF32,
    /// Inner product estimate from compressed representation.
    InnerProductEstimate,
    /// L2 distance estimate from compressed representation.
    L2DistanceEstimate,
    /// Dequantized cosine similarity (lossy but deterministic).
    CosineOnDequantizedF32,
}

/// INT-001: Codec capabilities — what operations a codec supports.
///
/// This allows callers to check capabilities before attempting operations
/// and get typed errors for unsupported capabilities instead of silent
/// failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CodecCapabilities {
    /// Codec can encode vectors.
    pub can_encode: bool,
    /// Codec can decode to approximate f32 vectors.
    pub can_decode: bool,
    /// Codec can estimate inner product from compressed form.
    pub can_score_inner_product: bool,
    /// Codec can estimate L2 distance from compressed form.
    pub can_score_l2: bool,
    /// Codec is lossless (raw f32 representation).
    pub is_lossless: bool,
}

/// INT-001: Resource limits for codec operations.
///
/// All codecs must respect these limits. Exceeding limits returns a
/// typed error instead of silently producing incorrect results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CodecResourceLimits {
    /// Maximum input dimensions supported.
    pub max_dim: u32,
    /// Maximum encoded bytes per block.
    pub max_encoded_bytes: u64,
    /// Maximum decode batch size.
    pub max_batch_size: u32,
}

impl Default for CodecResourceLimits {
    fn default() -> Self {
        Self {
            max_dim: 65536,
            max_encoded_bytes: 1 << 28, // 256 MiB
            max_batch_size: 4096,
        }
    }
}

/// INT-001: Canonical vector codec trait for interchangeable backends.
///
/// This is the one canonical contract. All codec backends implement this
/// trait. semantic-memory and scr-runtime-compression consume only this
/// trait, not backend-specific APIs.
pub trait VectorCodec: Send + Sync {
    type EncodedBlock;
    type Error: std::fmt::Display;

    /// Codec profile identity.
    fn profile(&self) -> &dyn CodecProfile;

    /// Codec capabilities.
    fn capabilities(&self) -> CodecCapabilities;

    /// Resource limits for this codec instance.
    fn resource_limits(&self) -> CodecResourceLimits;

    /// Encode a raw f32 vector into a byte block.
    fn encode_block(&self, input: &[f32]) -> Result<Self::EncodedBlock, Self::Error>;

    /// Decode an encoded block back to approximate f32 vectors.
    fn decode_block(&self, block: &Self::EncodedBlock, out: &mut [f32]) -> Result<(), Self::Error>;

    /// Score semantics supported by this codec.
    fn score_semantics(&self) -> ScoreSemantics;
}

/// INT-001: Canonical KV-cache codec trait.
///
/// Extends VectorCodec with KV-cache-specific operations.
pub trait KvCacheCodec: VectorCodec {
    type EncodedCache;

    fn encode_kv_cache(
        &self,
        tensors: &[f32],
        shape: KvTensorShape,
    ) -> Result<Self::EncodedCache, Self::Error>;

    fn decode_slice(
        &self,
        cache: &Self::EncodedCache,
        request: KvSliceRequest,
        out: &mut [f32],
    ) -> Result<(), Self::Error>;
}
