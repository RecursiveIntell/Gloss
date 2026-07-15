//! Streaming KV-cache encoder for incremental token-by-token construction.
//!
//! [`KvStreamEncoder`] allows building a [`KvEncodedTensorV1`] one token at a
//! time instead of requiring the full tensor upfront. Pages are flushed
//! automatically when they reach `tokens_per_page` from the page geometry.
//!
//! The streaming encoder produces output identical to [`encode_kv_tensor`] for
//! the same input data, shape, layout, and profile. The source digest is
//! accumulated incrementally via [`blake3::Hasher`] and finalized in
//! [`KvStreamEncoder::finish`].
//!
//! # Limitations
//!
//! The current implementation supports `batch = 1, layers = 1, kv_heads = 1`
//! shapes (the common single-stream inference case). Each call to
//! [`KvStreamEncoder::append_token`] produces exactly one encoded block for
//! the matching role.

use crate::{FibQuantError, FibQuantizer, Result};

use super::{
    block::{KvBlockEncodingV1, KvEncodedBlockV1},
    codec::KvEncodedTensorV1,
    layout::KvCacheLayoutV1,
    page::KvEncodedPageV1,
    profile::{KvAxisPolicyV1, KvCompressionProfileV1, KvFallbackModeV1},
    receipt::{now_unix_seconds, KvCompressionReceiptV1, KvOperationKindV1, KV_RECEIPT_SCHEMA},
    shape::{KvRole, KvTensorShapeV1},
};

/// Receipt returned after appending a single token to the stream encoder.
#[derive(Debug, Clone, PartialEq)]
pub struct AppendReceipt {
    /// Global token index that was appended.
    pub token_index: u32,
    /// Encoding used for the key block: `"fib_quant"` or `"raw"`.
    pub key_block_encoding: String,
    /// Encoding used for the value block: `"fib_quant"` or `"raw"`.
    pub value_block_encoding: String,
    /// Compressed byte size of the key block.
    pub key_compressed_bytes: usize,
    /// Compressed byte size of the value block.
    pub value_compressed_bytes: usize,
    /// Fallback reasons (if any) for this token.
    pub fallback_reasons: Vec<String>,
}

/// Incremental/streaming KV-cache encoder.
///
/// Construct with [`KvStreamEncoder::new`], feed tokens one at a time with
/// [`KvStreamEncoder::append_token`], and finalize with
/// [`KvStreamEncoder::finish`] to obtain a [`KvEncodedTensorV1`].
///
/// The encoder accumulates a BLAKE3 source digest over all appended vectors
/// using an incremental [`blake3::Hasher`]. Pages are buffered as pending
/// (blocks + metadata) and materialized into [`KvEncodedPageV1`] objects in
/// `finish()` once the full source digest is available, ensuring that page
/// digests match the batch [`super::codec::encode_kv_tensor`] path.
pub struct KvStreamEncoder {
    /// Logical tensor shape.
    shape: KvTensorShapeV1,
    /// Physical layout.
    layout: KvCacheLayoutV1,
    /// Compression profile.
    profile: KvCompressionProfileV1,
    /// FibQuant quantizer built from the profile.
    quantizer: FibQuantizer,
    /// Blocks being accumulated for the current page.
    current_page_blocks: Vec<KvEncodedBlockV1>,
    /// Token index of the first token in the current page.
    current_page_token_start: u32,
    /// Materialized pages (filled in `finish()`).
    completed_pages: Vec<KvEncodedPageV1>,
    /// Incremental source digest accumulator.
    source_digest_state: blake3::Hasher,

    // ── private tracking state ───────────────────────────────────────────
    /// Next token index to assign (also equals number of tokens appended so far).
    token_index: u32,
    /// Next page id to assign.
    page_id: u32,
    /// Running count of compressed (non-fallback) blocks.
    compressed_blocks: u32,
    /// Running count of raw fallback blocks.
    raw_fallback_blocks: u32,
    /// Deduplicated fallback reasons across all tokens.
    fallback_reasons: Vec<String>,
    /// Pending pages: (page_id, token_start, token_count, blocks).
    /// Materialized into `completed_pages` in `finish()`.
    pending_pages: Vec<(u32, u32, u32, Vec<KvEncodedBlockV1>)>,
}

impl KvStreamEncoder {
    /// Create a new streaming encoder.
    ///
    /// Validates the shape, layout, and profile, then builds a quantizer from
    /// the profile. The source digest accumulator is initialized with the
    /// domain tag and expected total element count so that incremental
    /// hashing produces the same result as [`super::receipt::kv_tensor_digest`].
    ///
    /// Currently requires `batch = 1, layers = 1, kv_heads = 1`.
    pub fn new(
        shape: KvTensorShapeV1,
        layout: KvCacheLayoutV1,
        profile: KvCompressionProfileV1,
    ) -> Result<Self> {
        shape.validate()?;
        layout.validate_for_shape(&shape)?;
        profile.validate_for_shape(&shape)?;

        // The streaming encoder produces one block per token (one key vector
        // or one value vector). Multi-batch/multi-layer/multi-head shapes
        // would require multiple vectors per token and are not supported.
        if shape.batch != 1 || shape.layers != 1 || shape.kv_heads != 1 {
            return Err(FibQuantError::DependencyUnsupported(
                "streaming encoder currently supports batch=1, layers=1, kv_heads=1 only".into(),
            ));
        }

        let quantizer = build_quantizer(&profile)?;

        // Initialize the incremental BLAKE3 hasher with the same prefix that
        // `kv_tensor_digest` uses, so the finalized digest matches.
        let mut source_digest_state = blake3::Hasher::new();
        source_digest_state.update(b"fib_quant_kv_tensor_f32_v1");
        source_digest_state.update(&[0]);
        let total_elements = shape.element_count()? as u64;
        source_digest_state.update(&total_elements.to_le_bytes());

        Ok(Self {
            shape,
            layout,
            profile,
            quantizer,
            current_page_blocks: Vec::new(),
            current_page_token_start: 0,
            completed_pages: Vec::new(),
            source_digest_state,
            token_index: 0,
            page_id: 0,
            compressed_blocks: 0,
            raw_fallback_blocks: 0,
            fallback_reasons: Vec::new(),
            pending_pages: Vec::new(),
        })
    }

    /// Append one token's key and value vectors.
    ///
    /// Both vectors must have length `head_dim` and contain only finite values.
    /// Only the vector matching `shape.role` is encoded into a block; the other
    /// is reported as `"raw"` in the receipt with zero compressed bytes (it is
    /// not stored in this encoder's output).
    ///
    /// When the current page reaches `tokens_per_page` blocks, the blocks are
    /// flushed to a pending page buffer (materialized in `finish()`).
    pub fn append_token(
        &mut self,
        key_vector: &[f32],
        value_vector: &[f32],
    ) -> Result<AppendReceipt> {
        let head_dim = self.shape.head_dim as usize;

        // Validate vector lengths.
        if key_vector.len() != head_dim {
            return Err(FibQuantError::CorruptPayload(format!(
                "key_vector has {} elements, expected head_dim={}",
                key_vector.len(),
                head_dim
            )));
        }
        if value_vector.len() != head_dim {
            return Err(FibQuantError::CorruptPayload(format!(
                "value_vector has {} elements, expected head_dim={}",
                value_vector.len(),
                head_dim
            )));
        }

        // Check for non-finite values.
        if key_vector.iter().any(|v| !v.is_finite()) {
            return Err(FibQuantError::CorruptPayload(
                "key_vector contains non-finite value".into(),
            ));
        }
        if value_vector.iter().any(|v| !v.is_finite()) {
            return Err(FibQuantError::CorruptPayload(
                "value_vector contains non-finite value".into(),
            ));
        }

        // Guard against appending more tokens than the shape declares.
        if self.token_index >= self.shape.tokens {
            return Err(FibQuantError::CorruptPayload(format!(
                "stream encoder received token {} but shape only has {} tokens",
                self.token_index, self.shape.tokens
            )));
        }

        let token = self.token_index;

        // Select the vector matching the encoder's role and feed it to the
        // incremental source digest.
        let (encode_vector, other_vector) = match self.shape.role {
            KvRole::Key => (key_vector, value_vector),
            KvRole::Value => (value_vector, key_vector),
        };
        for v in encode_vector {
            self.source_digest_state.update(&v.to_le_bytes());
        }
        // The non-matching vector is NOT included in the source digest — the
        // batch encoder only hashes the single-role tensor values.
        let _ = other_vector; // suppress unused warning

        // Encode the matching vector into a block.
        let block_id = self.current_page_blocks.len() as u32;
        let protected = self
            .profile
            .protected_policy
            .is_protected(&self.shape, 0, 0, token);

        let block = if protected {
            KvEncodedBlockV1::raw(
                block_id,
                0,
                0,
                0,
                token,
                encode_vector.to_vec(),
                self.profile.page_geometry.encoded_block_bytes,
                "protected_region",
            )
        } else {
            self.encode_vector_block(block_id, token, encode_vector)?
        };

        // Track stats.
        let mut token_fallback_reasons = Vec::new();
        if block.raw_fallback {
            self.raw_fallback_blocks += 1;
            if !self.fallback_reasons.contains(&block.reason) {
                self.fallback_reasons.push(block.reason.clone());
            }
            token_fallback_reasons.push(block.reason.clone());
        } else {
            self.compressed_blocks += 1;
        }

        // Determine encoding type and compressed bytes for the receipt.
        let (encoding_type, compressed_bytes) = match &block.encoding {
            KvBlockEncodingV1::FibQuant { code } => ("fib_quant", code.compact_size()),
            KvBlockEncodingV1::RawF32 { values } => {
                ("raw", values.len() * std::mem::size_of::<f32>())
            }
        };

        self.current_page_blocks.push(block);
        self.token_index += 1;

        // Flush page when full.
        let tokens_per_page = self.profile.page_geometry.tokens_per_page;
        let tokens_in_current_page = self.token_index - self.current_page_token_start;
        if tokens_in_current_page >= tokens_per_page {
            self.flush_page();
        }

        // Build the receipt, reporting both key and value status.
        let (key_encoding, key_bytes, value_encoding, value_bytes) = match self.shape.role {
            KvRole::Key => (encoding_type, compressed_bytes, "raw", 0),
            KvRole::Value => ("raw", 0, encoding_type, compressed_bytes),
        };

        Ok(AppendReceipt {
            token_index: token,
            key_block_encoding: key_encoding.to_string(),
            value_block_encoding: value_encoding.to_string(),
            key_compressed_bytes: key_bytes,
            value_compressed_bytes: value_bytes,
            fallback_reasons: token_fallback_reasons,
        })
    }

    /// Finalize the stream and produce the encoded tensor.
    ///
    /// Flushes any remaining blocks as the final page, computes the source
    /// digest, materializes all pending pages, and builds the compression
    /// receipt.
    ///
    /// Returns an error if the number of appended tokens does not match
    /// `shape.tokens`.
    pub fn finish(mut self) -> Result<KvEncodedTensorV1> {
        // Validate token count.
        if self.token_index == 0 {
            return Err(FibQuantError::CorruptPayload(
                "stream encoder finished without any appended tokens".into(),
            ));
        }
        if self.token_index != self.shape.tokens {
            return Err(FibQuantError::CorruptPayload(format!(
                "stream encoder finished with {} tokens, expected {}",
                self.token_index, self.shape.tokens
            )));
        }

        // Flush remaining blocks as the last page.
        if !self.current_page_blocks.is_empty() {
            self.flush_page();
        }

        // Finalize the incremental source digest.
        let source_digest = format!("blake3:{}", self.source_digest_state.finalize().to_hex());
        let profile_digest = self.profile.digest(&self.shape)?;

        // Materialize all pending pages.
        for (page_id, token_start, token_count, blocks) in self.pending_pages.drain(..) {
            let page = KvEncodedPageV1::new(
                page_id,
                token_start,
                token_count,
                source_digest.clone(),
                profile_digest.clone(),
                &self.shape,
                self.profile.page_geometry.clone(),
                blocks,
            )?;
            self.completed_pages.push(page);
        }

        // Build the compression receipt.
        let page_digests = self
            .completed_pages
            .iter()
            .map(|p| p.page_digest.clone())
            .collect();

        let receipt = KvCompressionReceiptV1 {
            schema_version: KV_RECEIPT_SCHEMA.into(),
            operation_kind: KvOperationKindV1::Compress,
            source_digest,
            profile_digest,
            shape_digest: self.shape.digest()?,
            page_digests,
            codebook_digest: self.profile.codebook_digest.clone(),
            rotation_digest: self.profile.rotation_digest.clone(),
            encoded_pages: self.completed_pages.len() as u32,
            compressed_blocks: self.compressed_blocks,
            raw_fallback_blocks: self.raw_fallback_blocks,
            fallback_reasons: std::mem::take(&mut self.fallback_reasons),
            recorded_unix_seconds: now_unix_seconds(),
        };

        Ok(KvEncodedTensorV1 {
            shape: self.shape,
            layout: self.layout,
            profile: self.profile,
            pages: std::mem::take(&mut self.completed_pages),
            receipt,
        })
    }

    /// Flush the current page blocks to the pending buffer.
    fn flush_page(&mut self) {
        if self.current_page_blocks.is_empty() {
            return;
        }
        let token_start = self.current_page_token_start;
        let token_count = self.token_index - token_start;
        let blocks = std::mem::take(&mut self.current_page_blocks);
        self.pending_pages
            .push((self.page_id, token_start, token_count, blocks));
        self.page_id += 1;
        self.current_page_token_start = self.token_index;
    }

    /// Encode a single vector block, replicating the per-token logic from
    /// `codec::encode_vector_block`.
    fn encode_vector_block(
        &self,
        block_id: u32,
        token: u32,
        vector: &[f32],
    ) -> Result<KvEncodedBlockV1> {
        match self.profile.axis_policy {
            KvAxisPolicyV1::Raw => Ok(KvEncodedBlockV1::raw(
                block_id,
                0,
                0,
                0,
                token,
                vector.to_vec(),
                self.profile.page_geometry.encoded_block_bytes,
                "raw_axis_policy",
            )),
            KvAxisPolicyV1::PerToken => match self.quantizer.encode(vector) {
                Ok(code) => Ok(KvEncodedBlockV1::fib_quant(
                    block_id,
                    0,
                    0,
                    0,
                    token,
                    code,
                    self.profile.page_geometry.encoded_block_bytes,
                    "fib_quant_per_token",
                )),
                Err(err) if self.profile.fallback_policy.mode == KvFallbackModeV1::KeepRaw => {
                    Ok(KvEncodedBlockV1::raw(
                        block_id,
                        0,
                        0,
                        0,
                        token,
                        vector.to_vec(),
                        self.profile.page_geometry.encoded_block_bytes,
                        format!("encode_fallback:{err}"),
                    ))
                }
                Err(err) => Err(err),
            },
            KvAxisPolicyV1::PerChannel | KvAxisPolicyV1::RoleAwareKiviStyle => {
                if self.profile.fallback_policy.mode == KvFallbackModeV1::KeepRaw {
                    Ok(KvEncodedBlockV1::raw(
                        block_id,
                        0,
                        0,
                        0,
                        token,
                        vector.to_vec(),
                        self.profile.page_geometry.encoded_block_bytes,
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
}

/// Build a quantizer from a compression profile, verifying the codebook digest.
///
/// Replicates the private `build_quantizer` in `codec.rs` since that function
/// is not exported.
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

// ────────────────────────────────────────────────────────────────────────────
//  Tests
// ────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::super::codec::encode_kv_tensor;
    use super::super::layout::KvPageGeometryV1;
    use super::super::profile::KvAxisPolicyV1;
    use super::super::shape::{KvAttentionKind, KvDType, KvRopeState};
    use super::*;
    use crate::profile::FibQuantProfileV1;

    /// Build a shape/layout/profile suitable for the streaming encoder.
    ///
    /// Shape: batch=1, layers=1, kv_heads=1, tokens=3, head_dim=8
    /// Page geometry: tokens_per_page=2 (so 2 pages — boundary at token 2)
    /// FibQuant profile: k=4, N=32, ambient_dim=8
    fn build_test_parts() -> (
        KvTensorShapeV1,
        KvCacheLayoutV1,
        KvCompressionProfileV1,
        Vec<f32>,
    ) {
        let shape = KvTensorShapeV1::new(
            KvRole::Key,
            KvAttentionKind::Mha,
            1, // batch
            1, // layers
            1, // kv_heads
            1, // query_heads (== kv_heads for MHA)
            3, // tokens
            8, // head_dim
            KvDType::F32,
            KvRopeState::PreRope,
        );
        let layout = KvCacheLayoutV1::canonical(&shape).expect("canonical layout");
        let fib_profile =
            FibQuantProfileV1::paper_default(8, 4, 32, 42).expect("build fib profile");
        let quantizer = FibQuantizer::new(fib_profile.clone()).expect("build quantizer");
        let page_geometry = KvPageGeometryV1::new(2, 8, 64); // tokens_per_page=2
        let profile = KvCompressionProfileV1::from_parts(
            "test-stream-profile",
            &shape,
            fib_profile,
            quantizer.codebook().codebook_digest.clone(),
            KvAxisPolicyV1::PerToken,
            page_geometry,
        )
        .expect("build kv profile");

        // Deterministic input values: 3 tokens * 8 head_dim = 24 values.
        let total = shape.element_count().expect("element count");
        let values: Vec<f32> = (0..total).map(|i| (i as f32) * 0.1).collect();

        (shape, layout, profile, values)
    }

    #[test]
    fn stream_matches_batch_encode() {
        let (shape, layout, profile, values) = build_test_parts();

        // Batch encode.
        let batch_result =
            encode_kv_tensor(shape.clone(), layout.clone(), profile.clone(), &values)
                .expect("batch encode");

        // Stream encode: one token at a time.
        let mut encoder = KvStreamEncoder::new(shape.clone(), layout.clone(), profile.clone())
            .expect("build stream encoder");
        let head_dim = shape.head_dim as usize;
        for token in 0..shape.tokens {
            let start = token as usize * head_dim;
            let key_slice = &values[start..start + head_dim];
            // Pass the same slice as value_vector; only key is encoded for
            // a Key-role shape.
            encoder
                .append_token(key_slice, key_slice)
                .expect("append token");
        }
        let stream_result = encoder.finish().expect("stream finish");

        // Compare pages and blocks (page digests depend on source digest
        // which should match since we hash the same values in the same order).
        assert_eq!(stream_result.pages.len(), batch_result.pages.len());
        for (stream_page, batch_page) in stream_result.pages.iter().zip(batch_result.pages.iter()) {
            assert_eq!(stream_page.page_id, batch_page.page_id);
            assert_eq!(stream_page.token_start, batch_page.token_start);
            assert_eq!(stream_page.token_count, batch_page.token_count);
            assert_eq!(
                stream_page.source_tensor_digest,
                batch_page.source_tensor_digest
            );
            assert_eq!(
                stream_page.page_digest, batch_page.page_digest,
                "page digest mismatch for page {}",
                stream_page.page_id
            );
            assert_eq!(
                stream_page.encoded_blocks.len(),
                batch_page.encoded_blocks.len()
            );
            for (sb, bb) in stream_page
                .encoded_blocks
                .iter()
                .zip(batch_page.encoded_blocks.iter())
            {
                assert_eq!(sb.block_id, bb.block_id);
                assert_eq!(sb.token, bb.token);
                assert_eq!(sb.raw_fallback, bb.raw_fallback);
                assert_eq!(
                    sb.encoding, bb.encoding,
                    "block encoding mismatch for block {} (token {})",
                    sb.block_id, sb.token
                );
            }
        }

        // Compare receipt fields (except recorded_unix_seconds which depends
        // on wall-clock time).
        assert_eq!(
            stream_result.receipt.source_digest,
            batch_result.receipt.source_digest
        );
        assert_eq!(
            stream_result.receipt.profile_digest,
            batch_result.receipt.profile_digest
        );
        assert_eq!(
            stream_result.receipt.shape_digest,
            batch_result.receipt.shape_digest
        );
        assert_eq!(
            stream_result.receipt.page_digests,
            batch_result.receipt.page_digests
        );
        assert_eq!(
            stream_result.receipt.codebook_digest,
            batch_result.receipt.codebook_digest
        );
        assert_eq!(
            stream_result.receipt.rotation_digest,
            batch_result.receipt.rotation_digest
        );
        assert_eq!(
            stream_result.receipt.encoded_pages,
            batch_result.receipt.encoded_pages
        );
        assert_eq!(
            stream_result.receipt.compressed_blocks,
            batch_result.receipt.compressed_blocks
        );
        assert_eq!(
            stream_result.receipt.raw_fallback_blocks,
            batch_result.receipt.raw_fallback_blocks
        );
        assert_eq!(
            stream_result.receipt.fallback_reasons,
            batch_result.receipt.fallback_reasons
        );
    }

    #[test]
    fn append_receipt_fields_correct() {
        let (shape, layout, profile, values) = build_test_parts();
        let mut encoder =
            KvStreamEncoder::new(shape, layout, profile).expect("build stream encoder");
        let head_dim = 8;

        // Token 0 — should be in page 0.
        let r0 = encoder
            .append_token(&values[0..head_dim], &values[0..head_dim])
            .expect("append token 0");
        assert_eq!(r0.token_index, 0);
        // Key role → key_block_encoding should be "fib_quant" (if compression
        // succeeds) or "raw" (if fallback). Value should be "raw" with 0 bytes.
        assert!(
            r0.key_block_encoding == "fib_quant" || r0.key_block_encoding == "raw",
            "unexpected key_block_encoding: {}",
            r0.key_block_encoding
        );
        assert_eq!(r0.value_block_encoding, "raw");
        assert_eq!(r0.value_compressed_bytes, 0);
        if r0.key_block_encoding == "fib_quant" {
            assert!(r0.key_compressed_bytes > 0);
        } else {
            assert_eq!(
                r0.key_compressed_bytes,
                head_dim * std::mem::size_of::<f32>()
            );
        }

        // Token 1 — fills page 0 (tokens_per_page=2).
        let r1 = encoder
            .append_token(
                &values[head_dim..2 * head_dim],
                &values[head_dim..2 * head_dim],
            )
            .expect("append token 1");
        assert_eq!(r1.token_index, 1);

        // Token 2 — starts page 1.
        let r2 = encoder
            .append_token(
                &values[2 * head_dim..3 * head_dim],
                &values[2 * head_dim..3 * head_dim],
            )
            .expect("append token 2");
        assert_eq!(r2.token_index, 2);

        // Fallback reasons should be empty if all tokens compressed.
        if r0.key_block_encoding == "fib_quant"
            && r1.key_block_encoding == "fib_quant"
            && r2.key_block_encoding == "fib_quant"
        {
            assert!(r0.fallback_reasons.is_empty());
            assert!(r1.fallback_reasons.is_empty());
            assert!(r2.fallback_reasons.is_empty());
        }
    }

    #[test]
    fn empty_stream_finish_returns_error() {
        let (shape, layout, profile, _) = build_test_parts();
        let encoder = KvStreamEncoder::new(shape, layout, profile).expect("build encoder");
        let err = encoder.finish().unwrap_err();
        assert!(
            matches!(err, FibQuantError::CorruptPayload(ref msg)
                if msg.contains("without any appended tokens")),
            "expected empty-stream error, got: {err:?}"
        );
    }

    #[test]
    fn stream_decode_roundtrip() {
        let (shape, layout, profile, values) = build_test_parts();

        // Batch encode for reference.
        let batch_encoded =
            encode_kv_tensor(shape.clone(), layout.clone(), profile.clone(), &values)
                .expect("batch encode");
        let batch_decoded =
            super::super::codec::decode_kv_pages(&batch_encoded).expect("batch decode");

        // Stream encode.
        let mut encoder = KvStreamEncoder::new(shape.clone(), layout.clone(), profile.clone())
            .expect("build stream encoder");
        let head_dim = shape.head_dim as usize;
        for token in 0..shape.tokens {
            let start = token as usize * head_dim;
            encoder
                .append_token(
                    &values[start..start + head_dim],
                    &values[start..start + head_dim],
                )
                .expect("append token");
        }
        let encoded = encoder.finish().expect("stream finish");

        // Decode the stream output.
        let decoded = super::super::codec::decode_kv_pages(&encoded).expect("decode");
        assert_eq!(decoded.values.len(), values.len());

        // The stream-decoded values should match the batch-decoded values
        // exactly (same encoder, same quantizer → same codes).
        assert_eq!(
            decoded.values, batch_decoded.values,
            "stream decode must match batch decode"
        );
    }

    #[test]
    fn too_many_tokens_returns_error() {
        let (shape, layout, profile, values) = build_test_parts();
        let mut encoder = KvStreamEncoder::new(shape, layout, profile).expect("build encoder");
        let head_dim = 8;
        // Append all 3 tokens.
        for token in 0..3 {
            let start = token as usize * head_dim;
            encoder
                .append_token(
                    &values[start..start + head_dim],
                    &values[start..start + head_dim],
                )
                .expect("append token");
        }
        // Try appending a 4th token.
        let extra = vec![0.0f32; head_dim];
        let err = encoder.append_token(&extra, &extra).unwrap_err();
        assert!(
            matches!(err, FibQuantError::CorruptPayload(ref msg)
                if msg.contains("but shape only has")),
            "expected too-many-tokens error, got: {err:?}"
        );
    }

    #[test]
    fn partial_stream_returns_error() {
        let (shape, layout, profile, values) = build_test_parts();
        let mut encoder = KvStreamEncoder::new(shape, layout, profile).expect("build encoder");
        let head_dim = 8;
        // Append only 2 of 3 tokens.
        for token in 0..2 {
            let start = token as usize * head_dim;
            encoder
                .append_token(
                    &values[start..start + head_dim],
                    &values[start..start + head_dim],
                )
                .expect("append token");
        }
        let err = encoder.finish().unwrap_err();
        assert!(
            matches!(err, FibQuantError::CorruptPayload(ref msg)
                if msg.contains("finished with 2 tokens, expected 3")),
            "expected partial-stream error, got: {err:?}"
        );
    }

    #[test]
    fn wrong_vector_length_returns_error() {
        let (shape, layout, profile, _) = build_test_parts();
        let mut encoder = KvStreamEncoder::new(shape, layout, profile).expect("build encoder");
        let short = vec![0.0f32; 4]; // wrong length (should be 8)
        let err = encoder.append_token(&short, &short).unwrap_err();
        assert!(
            matches!(err, FibQuantError::CorruptPayload(ref msg)
                if msg.contains("key_vector has 4 elements")),
            "expected wrong-length error, got: {err:?}"
        );
    }

    #[test]
    fn non_finite_value_returns_error() {
        let (shape, layout, profile, _) = build_test_parts();
        let mut encoder = KvStreamEncoder::new(shape, layout, profile).expect("build encoder");
        let nan_vec = vec![f32::NAN; 8];
        let err = encoder.append_token(&nan_vec, &nan_vec).unwrap_err();
        assert!(
            matches!(err, FibQuantError::CorruptPayload(ref msg)
                if msg.contains("non-finite")),
            "expected non-finite error, got: {err:?}"
        );
    }
}
