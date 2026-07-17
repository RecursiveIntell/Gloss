//! KV-cache compression contracts and CPU reference paths.
//!
//! The `kv` feature is experimental and default-off. It provides typed
//! contracts, role-aware policy decisions, and CPU reference encode/decode
//! helpers for KV-cache pages. It is not a production serving backend.

pub mod attention_ref;
pub mod block;
pub mod calibration;
pub mod codec;
pub mod compressed_attention;
pub mod layout;
pub mod page;
pub mod policy;
pub mod profile;
pub mod quality;
pub mod receipt;
pub mod shape;
pub mod stream;

pub use attention_ref::{
    compare_attention_fixture, reference_attention_logits, reference_value_aggregation,
};
pub use block::{KvBlockEncodingV1, KvEncodedBlockV1};
pub use calibration::{calibrate_kv_tensor, KvCalibrationSummaryV1};
pub use codec::{
    decode_kv_pages, decode_kv_slice, encode_kv_tensor, KvDecodedTensorV1, KvEncodedTensorV1,
};
pub use compressed_attention::{
    compressed_attention_logits, compressed_attention_topk, CompressedAttentionOutput,
};
pub use layout::{KvCacheLayoutV1, KvLayoutOrder, KvPageGeometryV1};
pub use page::KvEncodedPageV1;
pub use policy::{
    decide_kv_compression, KvCompressionDecisionV1, KvCompressionPolicyV1, KvCompressionStrategyV1,
    KvDecisionActionV1, KvDecisionReasonV1,
};
pub use profile::{
    KvAxisPolicyV1, KvCompressionProfileV1, KvFallbackModeV1, KvFallbackPolicyV1,
    KvProtectedPolicyV1, KvQualityBudgetV1,
};
pub use quality::{KvAttentionQualityReportV1, KvLayerHeadQualityV1};
pub use receipt::{
    kv_tensor_digest, KvCompressionReceiptV1, KvDecodeReceiptV1, KvEvalReceiptV1, KvOperationKindV1,
};
pub use shape::{KvAttentionKind, KvDType, KvRole, KvRopeState, KvTensorShapeV1};
pub use stream::{AppendReceipt, KvStreamEncoder};
