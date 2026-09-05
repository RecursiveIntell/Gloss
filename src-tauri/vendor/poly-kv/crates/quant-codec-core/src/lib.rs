#![doc = "Shared codec/profile/shape traits for governed compression experiments."]

pub mod codec;
pub mod digest;
pub mod dtype;
pub mod error;
pub mod eval;
pub mod ids;
pub mod shape;

pub use codec::{
    CodecCapabilities, CodecProfile, CodecResourceLimits, KvCacheCodec, ScoreSemantics, VectorCodec,
};
pub use digest::{ArtifactDigest, CodecProfileDigest};
pub use dtype::DType;
pub use error::QuantCodecError;
pub use eval::EvalReport;
pub use ids::{CodecId, ModelFingerprint, TokenizerFingerprint};
pub use shape::{
    HeadId, KvAttentionKind, KvCacheShapeV2, KvLayout, KvRole, KvSliceRequest, KvTensorShape,
    LayerId, TokenSpan,
};
