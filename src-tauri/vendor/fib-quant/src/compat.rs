//! quant-codec-core trait implementations for fib-quant.
//!
//! This module implements the `VectorCodec`, `CodecProfile`, and
//! `KvCacheCodec` traits from `quant-codec-core` so that fib-quant
//! can be used through the shared codec trait boundary by `poly-kv`,
//! `quant-governor`, and `scr-runtime-compression`.
//!
//! This is an additive compatibility layer behind the `compat` feature
//! gate. It does not change any existing fib-quant APIs.

use quant_codec_core::{
    CodecId, CodecProfile, CodecProfileDigest, EvalReport, KvCacheCodec, KvSliceRequest,
    KvTensorShape, QuantCodecError, VectorCodec,
};

use crate::{
    codec::FibCodeV1,
    kv::{
        codec::{encode_kv_tensor, KvEncodedTensorV1},
        layout::KvCacheLayoutV1,
        shape::KvTensorShapeV1,
    },
    metrics,
    profile::FibQuantProfileV1,
    FibQuantizer,
};

/// Codec ID string for fib-quant.
pub const FIB_QUANT_CODEC_ID: &str = "fib_quant";

fn map_err(e: crate::FibQuantError) -> QuantCodecError {
    QuantCodecError::ShapeMismatch {
        reason: e.to_string(),
    }
}

// ---- CodecProfile impl ----

impl CodecProfile for FibQuantProfileV1 {
    fn codec_id(&self) -> CodecId {
        CodecId::new(FIB_QUANT_CODEC_ID).expect("valid codec id")
    }

    fn codec_version(&self) -> &str {
        &self.codebook_version
    }

    fn profile_digest(&self) -> CodecProfileDigest {
        match self.digest() {
            Ok(hex) => {
                let bytes = hex_decode(&hex).unwrap_or_else(|_| vec![0u8; 32]);
                let mut arr = [0u8; 32];
                let len = bytes.len().min(32);
                arr[..len].copy_from_slice(&bytes[..len]);
                CodecProfileDigest::from_canonical_bytes(&arr)
            }
            Err(_) => CodecProfileDigest::from_canonical_bytes(b"fib_quant_error"),
        }
    }

    fn fixed_rate_bits(&self) -> Option<u16> {
        Some(self.wire_index_bits as u16)
    }

    fn block_dim(&self) -> Option<u16> {
        Some(self.block_dim as u16)
    }

    fn is_lossy(&self) -> bool {
        true
    }
}

// ---- VectorCodec impl ----

impl VectorCodec for FibQuantizer {
    type EncodedBlock = FibCodeV1;
    type Error = QuantCodecError;

    fn encode_block(&self, input: &[f32]) -> Result<Self::EncodedBlock, Self::Error> {
        self.encode(input).map_err(map_err)
    }

    fn decode_block(&self, block: &Self::EncodedBlock, out: &mut [f32]) -> Result<(), Self::Error> {
        let decoded = self.decode(block).map_err(map_err)?;
        let len = decoded.len().min(out.len());
        out[..len].copy_from_slice(&decoded[..len]);
        Ok(())
    }
}

// ---- KvCacheCodec impl ----

/// KV cache codec wrapping fib-quant's KV compression.
pub struct FibKvCodec {
    quantizer: FibQuantizer,
    kv_profile: crate::kv::profile::KvCompressionProfileV1,
}

impl FibKvCodec {
    /// Build a KV codec from a fib-quant profile and KV compression profile.
    pub fn new(
        fib_profile: FibQuantProfileV1,
        kv_profile: crate::kv::profile::KvCompressionProfileV1,
    ) -> Result<Self, QuantCodecError> {
        let quantizer = FibQuantizer::new(fib_profile).map_err(map_err)?;
        Ok(Self {
            quantizer,
            kv_profile,
        })
    }

    /// Access the underlying quantizer.
    pub fn quantizer(&self) -> &FibQuantizer {
        &self.quantizer
    }
}

impl VectorCodec for FibKvCodec {
    type EncodedBlock = FibCodeV1;
    type Error = QuantCodecError;

    fn encode_block(&self, input: &[f32]) -> Result<Self::EncodedBlock, Self::Error> {
        self.quantizer.encode(input).map_err(map_err)
    }

    fn decode_block(&self, block: &Self::EncodedBlock, out: &mut [f32]) -> Result<(), Self::Error> {
        let decoded = self.quantizer.decode(block).map_err(map_err)?;
        let len = decoded.len().min(out.len());
        out[..len].copy_from_slice(&decoded[..len]);
        Ok(())
    }
}

impl KvCacheCodec for FibKvCodec {
    type EncodedCache = KvEncodedTensorV1;

    fn encode_kv_cache(
        &self,
        tensors: &[f32],
        shape: KvTensorShape,
    ) -> Result<Self::EncodedCache, Self::Error> {
        let fib_shape = convert_to_fib_shape(&shape)?;
        let layout = KvCacheLayoutV1::canonical(&fib_shape).map_err(map_err)?;
        let encoded = encode_kv_tensor(fib_shape, layout, self.kv_profile.clone(), tensors)
            .map_err(map_err)?;
        Ok(encoded)
    }

    fn decode_slice(
        &self,
        cache: &Self::EncodedCache,
        request: KvSliceRequest,
        out: &mut [f32],
    ) -> Result<(), Self::Error> {
        // Random-access decode — only decodes blocks matching the
        // requested layer, head, and token range.
        let decoded = crate::kv::codec::decode_kv_slice(
            cache,
            request.layer.0,
            request.head.map(|h| h.0).unwrap_or(0),
            request.token_span.start as u32,
            request.token_span.end as u32,
        )
        .map_err(map_err)?;
        let len = decoded.len().min(out.len());
        out[..len].copy_from_slice(&decoded[..len]);
        Ok(())
    }
}

fn convert_to_fib_shape(shape: &KvTensorShape) -> Result<KvTensorShapeV1, QuantCodecError> {
    Ok(KvTensorShapeV1::new(
        crate::kv::shape::KvRole::Key,
        crate::kv::shape::KvAttentionKind::Mha,
        1,
        shape.layers,
        shape.key_heads,
        shape.key_heads, // query_heads = key_heads for Mha
        shape.seq_len as u32,
        shape.head_dim,
        crate::kv::shape::KvDType::F32,
        crate::kv::shape::KvRopeState::NotApplicable,
    ))
}

// ---- Eval adapter ----

/// Run a compression eval using the quant-codec-core EvalReport type.
pub fn eval_compress(
    quantizer: &FibQuantizer,
    vectors: &[&[f32]],
) -> Result<EvalReport, QuantCodecError> {
    if vectors.is_empty() {
        return Ok(EvalReport {
            mse: None,
            cosine_similarity: None,
            max_abs_error: None,
            bytes_exact: 0,
            bytes_encoded: 0,
            passed: true,
            notes: vec!["empty input".to_string()],
        });
    }
    let mut total_mse = 0.0f64;
    let mut total_cos = 0.0f64;
    let mut total_bytes_exact = 0u64;
    let mut total_bytes_encoded = 0u64;
    let mut max_abs_error = 0.0f64;
    let count = vectors.len() as f64;

    for v in vectors {
        let code = quantizer.encode(v).map_err(map_err)?;
        let decoded = quantizer.decode(&code).map_err(map_err)?;
        let mse = metrics::mse(v, &decoded).map_err(map_err)?;
        let cos = metrics::cosine_similarity(v, &decoded).map_err(map_err)?;
        total_mse += mse;
        total_cos += cos;
        total_bytes_exact += (v.len() * 4) as u64;
        total_bytes_encoded += code.compact_size() as u64;
        for (a, b) in v.iter().zip(decoded.iter()) {
            let err = (f64::from(*a) - f64::from(*b)).abs();
            if err > max_abs_error {
                max_abs_error = err;
            }
        }
    }

    Ok(EvalReport {
        mse: Some(total_mse / count),
        cosine_similarity: Some(total_cos / count),
        max_abs_error: Some(max_abs_error),
        bytes_exact: total_bytes_exact,
        bytes_encoded: total_bytes_encoded,
        passed: true,
        notes: vec![format!("{} vectors", vectors.len())],
    })
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}
