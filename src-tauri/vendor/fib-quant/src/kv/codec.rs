use serde::{Deserialize, Serialize};

use crate::{FibQuantError, FibQuantizer, Result};

use super::{
    block::{KvBlockEncodingV1, KvEncodedBlockV1},
    layout::KvCacheLayoutV1,
    page::KvEncodedPageV1,
    profile::{KvAxisPolicyV1, KvCompressionProfileV1, KvFallbackModeV1},
    receipt::{
        kv_tensor_digest, now_unix_seconds, KvCompressionReceiptV1, KvDecodeReceiptV1,
        KvOperationKindV1, KV_RECEIPT_SCHEMA,
    },
    shape::KvTensorShapeV1,
};

/// Encoded tensor artifact with pages and compression receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvEncodedTensorV1 {
    /// Logical shape.
    pub shape: KvTensorShapeV1,
    /// Physical layout.
    pub layout: KvCacheLayoutV1,
    /// Compression profile.
    pub profile: KvCompressionProfileV1,
    /// Encoded pages.
    pub pages: Vec<KvEncodedPageV1>,
    /// Compression receipt.
    pub receipt: KvCompressionReceiptV1,
}

/// Decoded tensor and receipt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KvDecodedTensorV1 {
    /// Canonical contiguous f32 values.
    pub values: Vec<f32>,
    /// Decode receipt.
    pub receipt: KvDecodeReceiptV1,
}

/// Encode a canonical contiguous f32 KV tensor.
pub fn encode_kv_tensor(
    shape: KvTensorShapeV1,
    layout: KvCacheLayoutV1,
    profile: KvCompressionProfileV1,
    values: &[f32],
) -> Result<KvEncodedTensorV1> {
    shape.validate()?;
    layout.validate_for_shape(&shape)?;
    profile.validate_for_shape(&shape)?;
    if values.len() != shape.element_count()? {
        return Err(FibQuantError::CorruptPayload(format!(
            "kv input has {} values, expected {}",
            values.len(),
            shape.element_count()?
        )));
    }
    if values.iter().any(|value| !value.is_finite()) {
        return Err(FibQuantError::CorruptPayload(
            "kv input contains non-finite value".into(),
        ));
    }

    let quantizer = build_quantizer(&profile)?;
    let source_digest = kv_tensor_digest(values)?;
    let profile_digest = profile.digest(&shape)?;
    let mut pages = Vec::new();
    let mut compressed_blocks = 0u32;
    let mut raw_fallback_blocks = 0u32;
    let mut fallback_reasons = Vec::new();
    let page_count = profile.page_geometry.page_count(&shape)?;

    for page_id in 0..page_count {
        let token_start = page_id * profile.page_geometry.tokens_per_page;
        let token_end = (token_start + profile.page_geometry.tokens_per_page).min(shape.tokens);
        let token_count = token_end - token_start;
        let mut blocks = Vec::new();
        for batch in 0..shape.batch {
            for layer in 0..shape.layers {
                for head in 0..shape.kv_heads {
                    for token in token_start..token_end {
                        let block_id = blocks.len() as u32;
                        let vector = vector_slice(values, &shape, batch, layer, head, token)?;
                        let protected = profile
                            .protected_policy
                            .is_protected(&shape, layer, head, token);
                        let block = if protected {
                            raw_block(
                                block_id,
                                batch,
                                layer,
                                head,
                                token,
                                vector,
                                profile.page_geometry.encoded_block_bytes,
                                "protected_region",
                            )
                        } else {
                            encode_vector_block(
                                &quantizer, &profile, block_id, batch, layer, head, token, vector,
                            )?
                        };
                        if block.raw_fallback {
                            raw_fallback_blocks += 1;
                            if !fallback_reasons.contains(&block.reason) {
                                fallback_reasons.push(block.reason.clone());
                            }
                        } else {
                            compressed_blocks += 1;
                        }
                        blocks.push(block);
                    }
                }
            }
        }
        pages.push(KvEncodedPageV1::new(
            page_id,
            token_start,
            token_count,
            source_digest.clone(),
            profile_digest.clone(),
            &shape,
            profile.page_geometry.clone(),
            blocks,
        )?);
    }

    let page_digests = pages.iter().map(|page| page.page_digest.clone()).collect();
    let receipt = KvCompressionReceiptV1 {
        schema_version: KV_RECEIPT_SCHEMA.into(),
        operation_kind: KvOperationKindV1::Compress,
        source_digest,
        profile_digest,
        shape_digest: shape.digest()?,
        page_digests,
        codebook_digest: profile.codebook_digest.clone(),
        rotation_digest: profile.rotation_digest.clone(),
        encoded_pages: pages.len() as u32,
        compressed_blocks,
        raw_fallback_blocks,
        fallback_reasons,
        recorded_unix_seconds: now_unix_seconds(),
    };
    Ok(KvEncodedTensorV1 {
        shape,
        layout,
        profile,
        pages,
        receipt,
    })
}

/// Decode encoded pages into canonical contiguous f32 values.
pub fn decode_kv_pages(encoded: &KvEncodedTensorV1) -> Result<KvDecodedTensorV1> {
    encoded.shape.validate()?;
    encoded.layout.validate_for_shape(&encoded.shape)?;
    encoded.profile.validate_for_shape(&encoded.shape)?;
    encoded.receipt.validate()?;
    let profile_digest = encoded.profile.digest(&encoded.shape)?;
    if encoded.receipt.profile_digest != profile_digest {
        return Err(FibQuantError::ProfileDigestMismatch {
            expected: profile_digest,
            actual: encoded.receipt.profile_digest.clone(),
        });
    }
    let quantizer = build_quantizer(&encoded.profile)?;
    let mut values = vec![0.0; encoded.shape.element_count()?];
    let mut page_digests = Vec::with_capacity(encoded.pages.len());
    let mut raw_fallback_blocks = 0u32;
    for page in &encoded.pages {
        page.validate(&encoded.shape)?;
        if page.profile_digest != encoded.receipt.profile_digest {
            return Err(FibQuantError::ProfileDigestMismatch {
                expected: encoded.receipt.profile_digest.clone(),
                actual: page.profile_digest.clone(),
            });
        }
        page_digests.push(page.page_digest.clone());
        for block in &page.encoded_blocks {
            if block.batch >= encoded.shape.batch
                || block.layer >= encoded.shape.layers
                || block.kv_head >= encoded.shape.kv_heads
                || block.token >= encoded.shape.tokens
            {
                return Err(FibQuantError::CorruptPayload(
                    "kv block index outside shape".into(),
                ));
            }
            let decoded = match &block.encoding {
                KvBlockEncodingV1::RawF32 { values } => {
                    raw_fallback_blocks += 1;
                    values.clone()
                }
                KvBlockEncodingV1::FibQuant { code } => quantizer.decode(code)?,
            };
            if decoded.len() != encoded.shape.head_dim as usize {
                return Err(FibQuantError::CorruptPayload(
                    "decoded kv vector head_dim mismatch".into(),
                ));
            }
            let out = vector_slice_mut(
                &mut values,
                &encoded.shape,
                block.batch,
                block.layer,
                block.kv_head,
                block.token,
            )?;
            out.copy_from_slice(&decoded);
        }
    }
    let decoded_digest = kv_tensor_digest(&values)?;
    Ok(KvDecodedTensorV1 {
        values,
        receipt: KvDecodeReceiptV1 {
            schema_version: KV_RECEIPT_SCHEMA.into(),
            operation_kind: KvOperationKindV1::Decode,
            decoded_digest,
            profile_digest: encoded.receipt.profile_digest.clone(),
            shape_digest: encoded.shape.digest()?,
            page_digests,
            codebook_digest: encoded.profile.codebook_digest.clone(),
            rotation_digest: encoded.profile.rotation_digest.clone(),
            decoded_pages: encoded.pages.len() as u32,
            raw_fallback_blocks,
            recorded_unix_seconds: now_unix_seconds(),
        },
    })
}

fn build_quantizer(profile: &KvCompressionProfileV1) -> Result<FibQuantizer> {
    let quantizer = FibQuantizer::new(profile.fib_profile.clone())?;
    if quantizer.codebook().codebook_digest != profile.codebook_digest {
        return Err(FibQuantError::CodebookDigestMismatch {
            expected: quantizer.codebook().codebook_digest.clone(),
            actual: profile.codebook_digest.clone(),
        });
    }
    Ok(quantizer)
}

#[allow(clippy::too_many_arguments)]
fn encode_vector_block(
    quantizer: &FibQuantizer,
    profile: &KvCompressionProfileV1,
    block_id: u32,
    batch: u32,
    layer: u32,
    head: u32,
    token: u32,
    vector: &[f32],
) -> Result<KvEncodedBlockV1> {
    match profile.axis_policy {
        KvAxisPolicyV1::Raw => Ok(raw_block(
            block_id,
            batch,
            layer,
            head,
            token,
            vector,
            profile.page_geometry.encoded_block_bytes,
            "raw_axis_policy",
        )),
        KvAxisPolicyV1::PerToken => match quantizer.encode(vector) {
            Ok(code) => Ok(KvEncodedBlockV1::fib_quant(
                block_id,
                batch,
                layer,
                head,
                token,
                code,
                profile.page_geometry.encoded_block_bytes,
                "fib_quant_per_token",
            )),
            Err(err) if profile.fallback_policy.mode == KvFallbackModeV1::KeepRaw => Ok(raw_block(
                block_id,
                batch,
                layer,
                head,
                token,
                vector,
                profile.page_geometry.encoded_block_bytes,
                format!("encode_fallback:{err}"),
            )),
            Err(err) => Err(err),
        },
        KvAxisPolicyV1::PerChannel | KvAxisPolicyV1::RoleAwareKiviStyle => {
            if profile.fallback_policy.mode == KvFallbackModeV1::KeepRaw {
                Ok(raw_block(
                    block_id,
                    batch,
                    layer,
                    head,
                    token,
                    vector,
                    profile.page_geometry.encoded_block_bytes,
                    "unsupported_axis_raw_fallback",
                ))
            } else {
                Err(FibQuantError::DependencyUnsupported(
                    "CPU reference codec supports per-token FibQuant compression only".into(),
                ))
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn raw_block(
    block_id: u32,
    batch: u32,
    layer: u32,
    head: u32,
    token: u32,
    vector: &[f32],
    fixed_size_bytes: u32,
    reason: impl Into<String>,
) -> KvEncodedBlockV1 {
    KvEncodedBlockV1::raw(
        block_id,
        batch,
        layer,
        head,
        token,
        vector.to_vec(),
        fixed_size_bytes,
        reason,
    )
}

/// Decode a random-access slice of the KV tensor for a single layer, head,
/// and token range without decoding the entire tensor.
///
/// Only pages overlapping `[token_start, token_end)` are visited, and within
/// each page only blocks matching `layer`, `head`, and the token range are
/// decoded. The decoded vectors are concatenated in ascending token order.
///
/// Currently decodes batch 0 (the common single-batch inference case).
pub fn decode_kv_slice(
    encoded: &KvEncodedTensorV1,
    layer: u32,
    head: u32,
    token_start: u32,
    token_end: u32,
) -> Result<Vec<f32>> {
    // ── validate tensor-level invariants (same as decode_kv_pages) ──────────
    encoded.shape.validate()?;
    encoded.layout.validate_for_shape(&encoded.shape)?;
    encoded.profile.validate_for_shape(&encoded.shape)?;
    encoded.receipt.validate()?;
    let profile_digest = encoded.profile.digest(&encoded.shape)?;
    if encoded.receipt.profile_digest != profile_digest {
        return Err(FibQuantError::ProfileDigestMismatch {
            expected: profile_digest,
            actual: encoded.receipt.profile_digest.clone(),
        });
    }

    // ── validate requested slice bounds ──────────────────────────────────────
    if layer >= encoded.shape.layers {
        return Err(FibQuantError::CorruptPayload(format!(
            "decode_kv_slice: layer {layer} >= shape.layers {}",
            encoded.shape.layers
        )));
    }
    if head >= encoded.shape.kv_heads {
        return Err(FibQuantError::CorruptPayload(format!(
            "decode_kv_slice: head {head} >= shape.kv_heads {}",
            encoded.shape.kv_heads
        )));
    }
    if token_start >= token_end {
        return Err(FibQuantError::CorruptPayload(format!(
            "decode_kv_slice: token_start {token_start} >= token_end {token_end}"
        )));
    }
    if token_start >= encoded.shape.tokens {
        return Err(FibQuantError::CorruptPayload(format!(
            "decode_kv_slice: token_start {token_start} >= shape.tokens {}",
            encoded.shape.tokens
        )));
    }
    if token_end > encoded.shape.tokens {
        return Err(FibQuantError::CorruptPayload(format!(
            "decode_kv_slice: token_end {token_end} > shape.tokens {}",
            encoded.shape.tokens
        )));
    }

    let quantizer = build_quantizer(&encoded.profile)?;
    let head_dim = encoded.shape.head_dim as usize;

    // Collect decoded vectors keyed by token so we can emit in ascending order
    // regardless of page/block traversal order.
    let mut decoded_map: std::collections::BTreeMap<u32, Vec<f32>> =
        std::collections::BTreeMap::new();

    for page in &encoded.pages {
        let page_token_end = page.token_start + page.token_count;

        // Skip pages entirely outside the requested token range.
        if page.token_start >= token_end || page_token_end <= token_start {
            continue;
        }

        // Validate the page (same checks as decode_kv_pages applies per-page).
        page.validate(&encoded.shape)?;
        if page.profile_digest != encoded.receipt.profile_digest {
            return Err(FibQuantError::ProfileDigestMismatch {
                expected: encoded.receipt.profile_digest.clone(),
                actual: page.profile_digest.clone(),
            });
        }

        for block in &page.encoded_blocks {
            // Only batch 0 for the single-batch slice path.
            if block.batch != 0 {
                continue;
            }
            if block.layer != layer || block.kv_head != head {
                continue;
            }
            if block.token < token_start || block.token >= token_end {
                continue;
            }

            // Bounds-check block indices against shape.
            if block.batch >= encoded.shape.batch
                || block.layer >= encoded.shape.layers
                || block.kv_head >= encoded.shape.kv_heads
                || block.token >= encoded.shape.tokens
            {
                return Err(FibQuantError::CorruptPayload(
                    "kv block index outside shape".into(),
                ));
            }

            let decoded = match &block.encoding {
                KvBlockEncodingV1::RawF32 { values } => values.clone(),
                KvBlockEncodingV1::FibQuant { code } => quantizer.decode(code)?,
            };

            if decoded.len() != head_dim {
                return Err(FibQuantError::CorruptPayload(
                    "decoded kv vector head_dim mismatch".into(),
                ));
            }

            decoded_map.insert(block.token, decoded);
        }
    }

    // Concatenate decoded vectors in ascending token order.
    let token_count = (token_end - token_start) as usize;
    let mut result = Vec::with_capacity(token_count * head_dim);
    for token in token_start..token_end {
        match decoded_map.remove(&token) {
            Some(vec) => result.extend_from_slice(&vec),
            None => {
                return Err(FibQuantError::CorruptPayload(format!(
                    "decode_kv_slice: missing block for token {token} (layer {layer}, head {head})"
                )));
            }
        }
    }

    Ok(result)
}

fn vector_offset(
    shape: &KvTensorShapeV1,
    batch: u32,
    layer: u32,
    head: u32,
    token: u32,
) -> Result<usize> {
    if batch >= shape.batch
        || layer >= shape.layers
        || head >= shape.kv_heads
        || token >= shape.tokens
    {
        return Err(FibQuantError::CorruptPayload(
            "kv vector index outside shape".into(),
        ));
    }
    let vectors_before = (((batch as usize * shape.layers as usize + layer as usize)
        * shape.kv_heads as usize
        + head as usize)
        * shape.tokens as usize)
        + token as usize;
    vectors_before
        .checked_mul(shape.head_dim as usize)
        .ok_or_else(|| FibQuantError::ResourceLimitExceeded("kv vector offset overflow".into()))
}

fn vector_slice<'a>(
    values: &'a [f32],
    shape: &KvTensorShapeV1,
    batch: u32,
    layer: u32,
    head: u32,
    token: u32,
) -> Result<&'a [f32]> {
    let start = vector_offset(shape, batch, layer, head, token)?;
    let end = start + shape.head_dim as usize;
    values
        .get(start..end)
        .ok_or_else(|| FibQuantError::CorruptPayload("kv vector slice out of bounds".into()))
}

fn vector_slice_mut<'a>(
    values: &'a mut [f32],
    shape: &KvTensorShapeV1,
    batch: u32,
    layer: u32,
    head: u32,
    token: u32,
) -> Result<&'a mut [f32]> {
    let start = vector_offset(shape, batch, layer, head, token)?;
    let end = start + shape.head_dim as usize;
    values
        .get_mut(start..end)
        .ok_or_else(|| FibQuantError::CorruptPayload("kv vector slice out of bounds".into()))
}

// ────────────────────────────────────────────────────────────────────────────
//  Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::FibQuantProfileV1;

    use super::super::layout::{KvCacheLayoutV1, KvPageGeometryV1};
    use super::super::profile::KvAxisPolicyV1;
    use super::super::shape::{KvAttentionKind, KvDType, KvRole, KvRopeState, KvTensorShapeV1};

    /// Build a small encoded tensor suitable for slice tests.
    ///
    /// Shape: batch=1, layers=2, kv_heads=2, tokens=6, head_dim=8
    /// Page geometry: tokens_per_page=3 (so 2 pages — boundary at token 3)
    /// FibQuant profile: k=4, N=32, ambient_dim=8
    fn build_test_tensor() -> KvEncodedTensorV1 {
        let shape = KvTensorShapeV1::new(
            KvRole::Key,
            KvAttentionKind::Mha,
            1, // batch
            2, // layers
            2, // kv_heads
            2, // query_heads (== kv_heads for MHA)
            6, // tokens
            8, // head_dim
            KvDType::F32,
            KvRopeState::PreRope,
        );
        let layout = KvCacheLayoutV1::canonical(&shape).expect("canonical layout");
        let fib_profile =
            FibQuantProfileV1::paper_default(8, 4, 32, 42).expect("build fib profile");
        let quantizer = FibQuantizer::new(fib_profile.clone()).expect("build quantizer");
        let page_geometry = KvPageGeometryV1::new(3, 8, 64); // tokens_per_page=3, head_dim=8
        let profile = KvCompressionProfileV1::from_parts(
            "test-profile",
            &shape,
            fib_profile,
            quantizer.codebook().codebook_digest.clone(),
            KvAxisPolicyV1::PerToken,
            page_geometry,
        )
        .expect("build kv profile");

        // Create deterministic input values.
        let total = shape.element_count().expect("element count");
        let values: Vec<f32> = (0..total).map(|i| (i as f32) * 0.1).collect();

        encode_kv_tensor(shape, layout, profile, &values).expect("encode tensor")
    }

    /// Extract a single (layer, head) slice from the full decoded tensor
    /// so we can compare against `decode_kv_slice`.
    fn full_decode_slice(
        full: &[f32],
        shape: &KvTensorShapeV1,
        layer: u32,
        head: u32,
        token_start: u32,
        token_end: u32,
    ) -> Vec<f32> {
        let head_dim = shape.head_dim as usize;
        let mut result = Vec::with_capacity((token_end - token_start) as usize * head_dim);
        for token in token_start..token_end {
            let offset = vector_offset(shape, 0, layer, head, token).expect("offset");
            result.extend_from_slice(&full[offset..offset + head_dim]);
        }
        result
    }

    #[test]
    fn slice_matches_full_decode() {
        let encoded = build_test_tensor();
        let full = decode_kv_pages(&encoded).expect("full decode");

        // Request the entire token range for layer 1, head 1.
        let slice = decode_kv_slice(&encoded, 1, 1, 0, 6).expect("slice decode");
        let expected = full_decode_slice(&full.values, &encoded.shape, 1, 1, 0, 6);
        assert_eq!(slice.len(), expected.len());
        // Values may not be bit-identical after quant→dequant round-trip, but
        // the slice should match the full decode exactly since both decode
        // the same blocks.
        for (i, (a, b)) in slice.iter().zip(expected.iter()).enumerate() {
            assert_eq!(a, b, "mismatch at index {i}: slice={a}, full={b}");
        }
    }

    #[test]
    fn slice_single_token() {
        let encoded = build_test_tensor();
        let full = decode_kv_pages(&encoded).expect("full decode");

        let slice = decode_kv_slice(&encoded, 0, 0, 2, 3).expect("single-token slice");
        assert_eq!(slice.len(), encoded.shape.head_dim as usize);

        let expected = full_decode_slice(&full.values, &encoded.shape, 0, 0, 2, 3);
        assert_eq!(slice, expected);
    }

    #[test]
    fn slice_spans_page_boundary() {
        let encoded = build_test_tensor();
        // Page 0 covers tokens 0..3, page 1 covers tokens 3..6.
        // Request tokens 2..4 — spanning the boundary.
        let full = decode_kv_pages(&encoded).expect("full decode");

        let slice = decode_kv_slice(&encoded, 0, 1, 2, 4).expect("boundary-spanning slice");
        assert_eq!(slice.len(), 2 * encoded.shape.head_dim as usize);

        let expected = full_decode_slice(&full.values, &encoded.shape, 0, 1, 2, 4);
        assert_eq!(slice, expected);
    }

    #[test]
    fn slice_only_visits_overlapping_pages() {
        let encoded = build_test_tensor();
        // Request tokens 0..2 — entirely in page 0.
        let slice = decode_kv_slice(&encoded, 0, 0, 0, 2).expect("partial slice");
        assert_eq!(slice.len(), 2 * encoded.shape.head_dim as usize);

        let full = decode_kv_pages(&encoded).expect("full decode");
        let expected = full_decode_slice(&full.values, &encoded.shape, 0, 0, 0, 2);
        assert_eq!(slice, expected);
    }

    #[test]
    fn slice_invalid_layer_returns_error() {
        let encoded = build_test_tensor();
        let err = decode_kv_slice(&encoded, 99, 0, 0, 1).unwrap_err();
        assert!(matches!(err, FibQuantError::CorruptPayload(ref msg)
            if msg.contains("layer")));
    }

    #[test]
    fn slice_invalid_head_returns_error() {
        let encoded = build_test_tensor();
        let err = decode_kv_slice(&encoded, 0, 99, 0, 1).unwrap_err();
        assert!(matches!(err, FibQuantError::CorruptPayload(ref msg)
            if msg.contains("head")));
    }

    #[test]
    fn slice_invalid_token_range_returns_error() {
        let encoded = build_test_tensor();
        // token_start >= token_end
        let err = decode_kv_slice(&encoded, 0, 0, 3, 3).unwrap_err();
        assert!(matches!(err, FibQuantError::CorruptPayload(ref msg)
            if msg.contains("token_start")));

        // token_start beyond shape tokens
        let err = decode_kv_slice(&encoded, 0, 0, 99, 100).unwrap_err();
        assert!(matches!(err, FibQuantError::CorruptPayload(ref msg)
            if msg.contains("token_start")));

        // token_end beyond shape tokens
        let err = decode_kv_slice(&encoded, 0, 0, 0, 100).unwrap_err();
        assert!(matches!(err, FibQuantError::CorruptPayload(ref msg)
            if msg.contains("token_end")));
    }
}
