use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::policy::CodecId;

/// A codec compresses and decompresses KV vectors.
pub trait KVecCodec: Send + Sync {
    /// Return the codec identifier ("fib_k4_n32", "turbo_8bit", etc.).
    fn codec_id(&self) -> CodecId;

    /// Encode a vector of f32 values into a compressed byte payload.
    fn encode(&self, vector: &[f32], seed: u64) -> Result<Vec<u8>>;

    /// Encode a batch of vectors in one call.
    ///
    /// The default implementation calls `encode` in a loop. Codecs that can
    /// exploit batch-level parallelism (e.g. fib-quant with `gpu-backend`)
    /// override this to issue a single batched dispatch.
    ///
    /// The returned byte payloads are in the same order as `vectors`.
    fn encode_batch(&self, vectors: &[&[f32]], seed: u64) -> Result<Vec<Vec<u8>>> {
        vectors.iter().map(|v| self.encode(v, seed)).collect()
    }

    /// Decode a compressed byte payload back into a vector of f32 values.
    fn decode(&self, payload: &[u8], seed: u64) -> Result<Vec<f32>>;

    /// Decode a batch of compressed payloads. Default loops over `decode`.
    fn decode_batch(&self, payloads: &[&[u8]], seed: u64) -> Result<Vec<Vec<f32>>> {
        payloads.iter().map(|p| self.decode(p, seed)).collect()
    }

    /// Encode a batch into one amortized binary payload when supported.
    ///
    /// Default `None` preserves legacy per-vector block storage. The fib adapter
    /// overrides this with FB2 so pool storage pays the codec profile/header once
    /// per layer side instead of once per vector.
    fn encode_batch_compact(&self, vectors: &[&[f32]], seed: u64) -> Result<Option<Vec<u8>>> {
        let _ = (vectors, seed);
        Ok(None)
    }

    /// Decode one compact batch payload back into vectors when supported.
    fn decode_batch_compact(&self, payload: &[u8], seed: u64) -> Result<Option<Vec<Vec<f32>>>> {
        let _ = (payload, seed);
        Ok(None)
    }

    /// The expected dimension of input/output vectors.
    fn dim(&self) -> usize;

    /// Expected compression ratio (nominal).
    fn compression_ratio(&self) -> f64;

    /// True if this adapter has access to GPU acceleration at runtime.
    ///
    /// This is distinct from the `gpu` feature being compiled in: a corpus
    /// too small for the GPU's batch threshold will fall through to CPU even
    /// when the feature is on. The pool build receipt uses this to set the
    /// `backend` field honestly.
    ///
    /// The default probes device availability only. Codecs that gate on
    /// batch size / dim should override [`Self::is_gpu_accelerated_for`].
    fn is_gpu_accelerated(&self) -> bool {
        false
    }

    /// True if a batch of `n` vectors at dimension `d` would actually
    /// dispatch to GPU. Default falls back to the device-availability probe;
    /// codecs with a runtime threshold should override.
    fn is_gpu_accelerated_for(&self, n: usize, d: usize) -> bool {
        let _ = (n, d);
        self.is_gpu_accelerated()
    }

    /// Codebook digest (BLAKE3 hex) for provenance receipts.
    /// Returns None if the codec doesn't support this.
    fn codebook_digest(&self, _seed: u64) -> Option<String> {
        None
    }

    /// Rotation digest (BLAKE3 hex) for provenance receipts.
    /// Returns None if the codec doesn't support this.
    fn rotation_digest(&self, _seed: u64) -> Option<String> {
        None
    }
}

/// A serialized compressed block with codec metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompressedBlock {
    /// Codec identifier.
    pub codec: CodecId,
    /// The compressed payload bytes.
    pub encoded_payload: Vec<u8>,
    /// Blake3 digest of the encoded payload.
    pub payload_digest: crate::digest_compat::Digest,
    /// Original (uncompressed) vector dimension.
    pub original_dim: usize,
    /// Size of the compressed payload in bytes.
    pub compressed_bytes: usize,
}

impl CompressedBlock {
    /// Create a new CompressedBlock from encoded payload.
    pub fn new(codec: CodecId, encoded_payload: Vec<u8>, original_dim: usize) -> Self {
        let compressed_bytes = encoded_payload.len();
        let payload_digest = crate::digest_compat::compute(&encoded_payload);
        Self {
            codec,
            encoded_payload,
            payload_digest,
            original_dim,
            compressed_bytes,
        }
    }

    /// Compression ratio: original f32 bytes / compressed bytes.
    pub fn compression_ratio(&self) -> f64 {
        let raw_bytes = self.original_dim * 4; // 4 bytes per f32
        if self.compressed_bytes == 0 {
            return f64::INFINITY;
        }
        raw_bytes as f64 / self.compressed_bytes as f64
    }
}

// ── Exact fallback codec (no compression) ──

/// Exact fallback codec: stores raw f32 bytes with no compression.
pub struct ExactFallbackCodec {
    dim: usize,
}

impl ExactFallbackCodec {
    pub fn new(dim: usize) -> Self {
        Self { dim }
    }
}

impl KVecCodec for ExactFallbackCodec {
    fn codec_id(&self) -> CodecId {
        crate::policy::CODEC_EXACT_FALLBACK.into()
    }

    fn encode(&self, vector: &[f32], _seed: u64) -> Result<Vec<u8>> {
        if vector.len() != self.dim {
            return Err(crate::error::PolyKvError::DimensionMismatch {
                expected: self.dim,
                got: vector.len(),
            });
        }
        // Store raw f32 bytes in little-endian
        let bytes: Vec<u8> = vector.iter().flat_map(|v| v.to_le_bytes()).collect();
        Ok(bytes)
    }

    fn decode(&self, payload: &[u8], _seed: u64) -> Result<Vec<f32>> {
        let expected_len = self.dim * 4;
        if payload.len() != expected_len {
            return Err(crate::error::PolyKvError::CorruptPayload(format!(
                "exact fallback payload size {} != expected {}",
                payload.len(),
                expected_len
            )));
        }
        let mut vec = Vec::with_capacity(self.dim);
        for chunk in payload.chunks_exact(4) {
            let arr: [u8; 4] = chunk.try_into().unwrap();
            vec.push(f32::from_le_bytes(arr));
        }
        Ok(vec)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn compression_ratio(&self) -> f64 {
        1.0
    }
}

// ── TurboQuant adapter ──

/// Adapter for the turbo-quant crate (8-bit, 32 projections).
#[cfg(feature = "turbo")]
pub struct TurboQuantAdapter {
    dim: usize,
    bits: u8,
    projections: usize,
}

#[cfg(feature = "turbo")]
impl TurboQuantAdapter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(dim: usize, bits: u8, projections: usize) -> Result<Self> {
        if dim == 0 {
            return Err(crate::error::PolyKvError::InvalidPolicy(
                "turbo dim must be > 0".into(),
            ));
        }
        if dim % 2 != 0 {
            // Pad to even — turbo-quant requires even dimensions
            return Err(crate::error::PolyKvError::InvalidPolicy(format!(
                "turbo requires even dimension, got {}",
                dim
            )));
        }
        Ok(Self {
            dim,
            bits,
            projections,
        })
    }
}

#[cfg(feature = "turbo")]
impl KVecCodec for TurboQuantAdapter {
    fn codec_id(&self) -> CodecId {
        crate::policy::CODEC_TURBO_8BIT.into()
    }

    fn encode(&self, vector: &[f32], seed: u64) -> Result<Vec<u8>> {
        let quantizer =
            turbo_quant::TurboQuantizer::new(self.dim, self.bits, self.projections, seed).map_err(
                |e| {
                    crate::error::PolyKvError::CompressionFailed(format!(
                        "turbo quantizer init failed: {}",
                        e
                    ))
                },
            )?;

        let code = quantizer.encode(vector).map_err(|e| {
            crate::error::PolyKvError::CompressionFailed(format!("turbo encode failed: {}", e))
        })?;

        // Use the compact binary wire format from turbo-quant's TurboCodeWireV1
        // instead of the JSON envelope. The JSON envelope was 472 bytes/block
        // around ~26 bytes of actual data for the b=8 / 32-projections path;
        // the compact format is header (46 bytes) + radii (dim/2 × 4 bytes) +
        // packed angles (dim/2 × bits/8 bytes) + packed signs (projections/8 bytes).
        // For head_dim=64, b=8, projections=32: 46 + 128 + 28 + 4 = 206 bytes
        // (vs ~472 bytes JSON). The compact format is 2.3× smaller per block.
        turbo_quant::TurboCodeWireV1::encode(&code, &quantizer).map_err(|e| {
            crate::error::PolyKvError::CompressionFailed(format!("turbo wire encode failed: {}", e))
        })
    }

    fn decode(&self, payload: &[u8], seed: u64) -> Result<Vec<f32>> {
        // Compact binary format is preferred (header starts with TURBO_CODE_WIRE_MAGIC
        // = "TQW1", 4 bytes), but fall back to JSON for backward compat with shells
        // written by older poly-kv versions.
        let code: turbo_quant::TurboCode =
            if payload.len() >= 4 && &payload[0..4] == turbo_quant::TURBO_CODE_WIRE_MAGIC {
                let quantizer =
                    turbo_quant::TurboQuantizer::new(self.dim, self.bits, self.projections, seed)
                        .map_err(|e| {
                        crate::error::PolyKvError::DecompressionFailed(format!(
                            "turbo quantizer init failed: {}",
                            e
                        ))
                    })?;
                turbo_quant::TurboCodeWireV1::decode(payload, &quantizer).map_err(|e| {
                    crate::error::PolyKvError::DecompressionFailed(format!(
                        "turbo wire decode failed: {}",
                        e
                    ))
                })?
            } else {
                serde_json::from_slice(payload).map_err(|e| {
                    crate::error::PolyKvError::DecompressionFailed(format!(
                        "turbo code deserialize failed: {}",
                        e
                    ))
                })?
            };

        // Reconstruct from polar component via independent PolarQuantizer.
        // QJL residual is lossy and not invertible, so we return the polar
        // approximation.
        let polar_quant =
            turbo_quant::PolarQuantizer::new(self.dim, self.bits - 1, seed).map_err(|e| {
                crate::error::PolyKvError::DecompressionFailed(format!(
                    "turbo polar quantizer init failed: {}",
                    e
                ))
            })?;

        let reconstructed = polar_quant.decode(&code.polar_code).map_err(|e| {
            crate::error::PolyKvError::DecompressionFailed(format!("turbo decode failed: {}", e))
        })?;

        Ok(reconstructed)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn compression_ratio(&self) -> f64 {
        8.0
    }
}

// ── FibQuant adapter ──

/// Adapter for the fib-quant crate (k=4, N=32, paper core path).
#[cfg(feature = "fib")]
pub struct FibQuantAdapter {
    dim: usize,
    k: u32,
    n: u32,
    training_samples: u32,
    lloyd_restarts: u32,
    lloyd_iterations: u32,
}

#[cfg(feature = "fib")]
impl FibQuantAdapter {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        dim: usize,
        k: u32,
        n: u32,
        training_samples: u32,
        lloyd_restarts: u32,
        lloyd_iterations: u32,
    ) -> Result<Self> {
        if dim == 0 {
            return Err(crate::error::PolyKvError::InvalidPolicy(
                "fib dim must be > 0".into(),
            ));
        }
        if dim % k as usize != 0 {
            return Err(crate::error::PolyKvError::InvalidPolicy(format!(
                "fib ambient dim ({}) must be divisible by k ({})",
                dim, k
            )));
        }
        Ok(Self {
            dim,
            k,
            n,
            training_samples,
            lloyd_restarts,
            lloyd_iterations,
        })
    }

    /// Build a FibQuantizer for the given seed.
    pub fn build_quantizer(
        &self,
        seed: u64,
    ) -> std::result::Result<fib_quant::FibQuantizer, crate::error::PolyKvError> {
        let mut profile = fib_quant::FibQuantProfileV1::paper_default(
            self.dim,
            self.k as usize,
            self.n as usize,
            seed,
        )
        .map_err(|e| {
            crate::error::PolyKvError::CompressionFailed(format!("fib profile build failed: {}", e))
        })?;

        // Override training parameters
        profile.training_samples = self.training_samples;
        profile.lloyd_restarts = self.lloyd_restarts;
        profile.lloyd_iterations = self.lloyd_iterations;

        fib_quant::FibQuantizer::new(profile).map_err(|e| {
            crate::error::PolyKvError::CompressionFailed(format!(
                "fib quantizer build failed: {}",
                e
            ))
        })
    }

    /// Decode one or more `FibCodeV1` values from a pool/shell payload without
    /// reconstructing the f32 vector values. This is the cold-pool compressed
    /// read path used by query-aware attention selection.
    pub fn decode_codes_payload(
        &self,
        payload: &[u8],
        seed: u64,
    ) -> Result<Vec<fib_quant::FibCodeV1>> {
        let quantizer = self.build_quantizer(seed)?;
        let profile = quantizer.profile().clone();
        let profile_digest = profile.digest().map_err(|e| {
            crate::error::PolyKvError::DecompressionFailed(format!("fib profile digest: {e}"))
        })?;
        let mut codes = if payload.len() >= 4 && payload[0..4] == FIB_WIRE_BATCH_MAGIC {
            decode_fib_batch_payload_wire(payload)?
        } else if payload.len() >= 3 && payload[0..3] == FIB_BATCHED_MAGIC {
            decode_fib_batch_payload(payload, &profile)?
        } else if payload.len() >= 3 && payload[0..3] == fib_quant::COMPACT_MAGIC {
            vec![
                fib_quant::FibCodeV1::from_compact_bytes(payload, &profile).map_err(|e| {
                    crate::error::PolyKvError::DecompressionFailed(format!(
                        "fib compact decode failed: {e}"
                    ))
                })?,
            ]
        } else {
            vec![serde_json::from_slice(payload).map_err(|e| {
                crate::error::PolyKvError::DecompressionFailed(format!(
                    "fib code deserialize failed: {e}"
                ))
            })?]
        };
        for code in &mut codes {
            code.profile_digest = profile_digest.clone();
        }
        Ok(codes)
    }
}

/// Magic for the new self-describing wire batch format: "FBWB" (Fib Wire Batch v1).
#[cfg(feature = "fib")]
const FIB_WIRE_BATCH_MAGIC: [u8; 4] = [b'F', b'B', b'W', b'B'];

// Legacy FB2 constants — kept only for backward-compat fallback decode.
#[cfg(feature = "fib")]
const FIB_BATCHED_MAGIC: [u8; 3] = [b'F', b'B', b'2'];
#[cfg(feature = "fib")]
const FIB_BATCHED_VERSION: u8 = 1;

#[cfg(feature = "fib")]
fn fib_code_indices_len(block_count: u32, wire_index_bits: u8) -> Result<usize> {
    (block_count as usize)
        .checked_mul(wire_index_bits as usize)
        .map(|bits| bits.div_ceil(8))
        .ok_or_else(|| {
            crate::error::PolyKvError::CorruptPayload("FB2 packed index length overflow".into())
        })
}

/// Encode a batch using the FBWB shared-header format.
///
/// Layout:
///   [FBWB magic: 4][count: u32][first_wire_len: u32][first_code_wire_bytes: ...]
///   [remaining codes: (norm_payload + indices)] × (count - 1)
///
/// The first code is a complete FibCodeWireV1 blob (self-describing). Subsequent
/// codes share the same profile shape and store only their payload bytes, keeping
/// batch overhead proportional to number of codes rather than per-code header cost.
#[cfg(feature = "fib")]
fn encode_fib_batch_payload(
    codes: &[fib_quant::FibCodeV1],
    profile: &fib_quant::FibQuantProfileV1,
) -> Result<Vec<u8>> {
    if codes.is_empty() {
        return Err(crate::error::PolyKvError::CompressionFailed(
            "cannot encode empty wire batch".into(),
        ));
    }
    let first_wire = fib_quant::FibCodeWireV1::to_wire_bytes(&codes[0], profile).map_err(|e| {
        crate::error::PolyKvError::CompressionFailed(format!("fib wire encode failed: {e}"))
    })?;
    let norm_len = codes[0].norm_payload.len();
    let indices_len = codes[0].indices.len();
    let mut out = Vec::with_capacity(
        4 + 4 + 4 + first_wire.len() + (codes.len() - 1) * (norm_len + indices_len),
    );
    out.extend_from_slice(&FIB_WIRE_BATCH_MAGIC);
    out.extend_from_slice(&(codes.len() as u32).to_le_bytes());
    out.extend_from_slice(&(first_wire.len() as u32).to_le_bytes());
    out.extend_from_slice(&first_wire);
    for code in &codes[1..] {
        if code.norm_payload.len() != norm_len || code.indices.len() != indices_len {
            return Err(crate::error::PolyKvError::CompressionFailed(
                "wire batch contains heterogeneous FibCodeV1 shapes".into(),
            ));
        }
        out.extend_from_slice(&code.norm_payload);
        out.extend_from_slice(&code.indices);
    }
    Ok(out)
}

/// Decode the FBWB shared-header batch format. No external profile needed.
/// Subsequent codes are reconstructed using the shape inferred from the first wire code.
#[cfg(feature = "fib")]
fn decode_fib_batch_payload_wire(payload: &[u8]) -> Result<Vec<fib_quant::FibCodeV1>> {
    // payload[0..4] = FBWB magic (already checked by caller)
    if payload.len() < 12 {
        return Err(crate::error::PolyKvError::CorruptPayload(format!(
            "wire batch too short: {} bytes",
            payload.len()
        )));
    }
    let count = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
    if count == 0 {
        return Ok(Vec::new());
    }
    let first_len = u32::from_le_bytes([payload[8], payload[9], payload[10], payload[11]]) as usize;
    let first_end = 12 + first_len;
    if first_end > payload.len() {
        return Err(crate::error::PolyKvError::CorruptPayload(format!(
            "wire batch first code truncated: need {} bytes, have {}",
            first_end,
            payload.len()
        )));
    }
    let (first_code, _profile) = fib_quant::FibCodeWireV1::from_wire_bytes(&payload[12..first_end])
        .map_err(|e| {
            crate::error::PolyKvError::DecompressionFailed(format!(
                "fib wire decode failed at code 0: {e}"
            ))
        })?;
    let norm_len = first_code.norm_payload.len();
    let indices_len = first_code.indices.len();
    let per_code = norm_len + indices_len;
    let remaining = count - 1;
    if payload.len() < first_end + remaining * per_code {
        return Err(crate::error::PolyKvError::CorruptPayload(format!(
            "wire batch body truncated: need {} bytes for {} remaining codes, have {}",
            remaining * per_code,
            remaining,
            payload.len() - first_end
        )));
    }
    let mut codes = Vec::with_capacity(count);
    codes.push(first_code.clone());
    let mut cursor = first_end;
    for _ in 1..count {
        let norm_payload = payload[cursor..cursor + norm_len].to_vec();
        cursor += norm_len;
        let indices = payload[cursor..cursor + indices_len].to_vec();
        cursor += indices_len;
        codes.push(fib_quant::FibCodeV1 {
            schema_version: first_code.schema_version.clone(),
            profile_digest: first_code.profile_digest.clone(),
            codebook_digest: first_code.codebook_digest.clone(),
            rotation_digest: first_code.rotation_digest.clone(),
            ambient_dim: first_code.ambient_dim,
            block_dim: first_code.block_dim,
            norm_format: first_code.norm_format.clone(),
            norm_payload,
            wire_index_bits: first_code.wire_index_bits,
            block_count: first_code.block_count,
            indices,
        });
    }
    Ok(codes)
}

/// Legacy FB2 fallback decode — used only when the payload starts with the old FB2 magic.
#[cfg(feature = "fib")]
fn decode_fib_batch_payload(
    payload: &[u8],
    profile: &fib_quant::FibQuantProfileV1,
) -> Result<Vec<fib_quant::FibCodeV1>> {
    if payload.len() < 15 {
        return Err(crate::error::PolyKvError::CorruptPayload(format!(
            "FB2 payload too short: {} bytes",
            payload.len()
        )));
    }
    if payload[0..3] != FIB_BATCHED_MAGIC {
        return Err(crate::error::PolyKvError::CorruptPayload(
            "FB2 payload bad magic".into(),
        ));
    }
    if payload[3] != FIB_BATCHED_VERSION {
        return Err(crate::error::PolyKvError::CorruptPayload(format!(
            "FB2 version {} not supported",
            payload[3]
        )));
    }
    let count = u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]) as usize;
    let wire_index_bits = payload[8];
    let block_count = u32::from_le_bytes([payload[9], payload[10], payload[11], payload[12]]);
    let norm_len = u16::from_le_bytes([payload[13], payload[14]]) as usize;
    let indices_len = fib_code_indices_len(block_count, wire_index_bits)?;
    let per_code = norm_len.checked_add(indices_len).ok_or_else(|| {
        crate::error::PolyKvError::CorruptPayload("FB2 per-code length overflow".into())
    })?;
    let expected = 15usize
        .checked_add(count.checked_mul(per_code).ok_or_else(|| {
            crate::error::PolyKvError::CorruptPayload("FB2 payload length overflow".into())
        })?)
        .ok_or_else(|| {
            crate::error::PolyKvError::CorruptPayload("FB2 payload length overflow".into())
        })?;
    if payload.len() != expected {
        return Err(crate::error::PolyKvError::CorruptPayload(format!(
            "FB2 payload length {} != expected {} (count={count}, per_code={per_code})",
            payload.len(),
            expected
        )));
    }
    let profile_digest = profile.digest().map_err(|e| {
        crate::error::PolyKvError::DecompressionFailed(format!("fib profile digest failed: {e}"))
    })?;
    let mut codes = Vec::with_capacity(count);
    let mut cursor = 15;
    for _ in 0..count {
        let norm_payload = payload[cursor..cursor + norm_len].to_vec();
        cursor += norm_len;
        let indices = payload[cursor..cursor + indices_len].to_vec();
        cursor += indices_len;
        codes.push(fib_quant::FibCodeV1 {
            schema_version: fib_quant::codec::CODE_SCHEMA.into(),
            profile_digest: profile_digest.clone(),
            codebook_digest: String::new(),
            rotation_digest: String::new(),
            ambient_dim: profile.ambient_dim,
            block_dim: profile.block_dim,
            norm_format: profile.norm_format.clone(),
            norm_payload,
            wire_index_bits,
            block_count,
            indices,
        });
    }
    Ok(codes)
}

#[cfg(feature = "fib")]
impl KVecCodec for FibQuantAdapter {
    fn codec_id(&self) -> CodecId {
        crate::policy::CODEC_FIB_K4_N32.into()
    }

    fn encode(&self, vector: &[f32], seed: u64) -> Result<Vec<u8>> {
        let quantizer = self.build_quantizer(seed)?;
        let code = quantizer.encode(vector).map_err(|e| {
            crate::error::PolyKvError::CompressionFailed(format!("fib encode failed: {}", e))
        })?;

        // Use the compact binary wire format (23 bytes vs 472 bytes JSON
        // for fib_k4_n32 with head_dim=64 — 20.5x smaller). The compact
        // format drops profile_digest, codebook_digest, rotation_digest,
        // ambient_dim, block_dim, norm_format — all of which the decoder
        // re-derives from its own profile. See fib_quant::FibCodeV1::to_compact_bytes.
        Ok(code.to_compact_bytes())
    }

    fn encode_batch(&self, vectors: &[&[f32]], seed: u64) -> Result<Vec<Vec<u8>>> {
        // Build a single quantizer shared across the batch. The codebook and
        // rotation are deterministic functions of the profile, so a single
        // quantizer is byte-identical to one built per-vector.
        let quantizer = self.build_quantizer(seed)?;
        let codes = quantizer.encode_batch(vectors).map_err(|e| {
            crate::error::PolyKvError::CompressionFailed(format!("fib encode_batch failed: {}", e))
        })?;
        let mut out = Vec::with_capacity(codes.len());
        for code in codes {
            out.push(code.to_compact_bytes());
        }
        Ok(out)
    }

    fn encode_batch_compact(&self, vectors: &[&[f32]], seed: u64) -> Result<Option<Vec<u8>>> {
        let quantizer = self.build_quantizer(seed)?;
        let codes = quantizer.encode_batch(vectors).map_err(|e| {
            crate::error::PolyKvError::CompressionFailed(format!("fib encode_batch failed: {}", e))
        })?;
        Ok(Some(encode_fib_batch_payload(&codes, quantizer.profile())?))
    }

    fn decode(&self, payload: &[u8], seed: u64) -> Result<Vec<f32>> {
        let quantizer = self.build_quantizer(seed)?;
        // Compact binary format is preferred (the pool always writes this
        // now), but fall back to JSON for backward compat with pools written
        // by older poly-kv versions.
        let code = if payload.len() >= 3 && payload[0..3] == fib_quant::COMPACT_MAGIC {
            let profile = quantizer.profile().clone();
            fib_quant::FibCodeV1::from_compact_bytes(payload, &profile).map_err(|e| {
                crate::error::PolyKvError::DecompressionFailed(format!(
                    "fib compact decode failed: {}",
                    e
                ))
            })?
        } else {
            serde_json::from_slice(payload).map_err(|e| {
                crate::error::PolyKvError::DecompressionFailed(format!(
                    "fib code deserialize failed: {}",
                    e
                ))
            })?
        };

        let decoded = quantizer.decode(&code).map_err(|e| {
            crate::error::PolyKvError::DecompressionFailed(format!("fib decode failed: {}", e))
        })?;

        Ok(decoded)
    }

    fn decode_batch(&self, payloads: &[&[u8]], seed: u64) -> Result<Vec<Vec<f32>>> {
        let quantizer = self.build_quantizer(seed)?;
        let profile = quantizer.profile().clone();
        let mut codes = Vec::with_capacity(payloads.len());
        for p in payloads {
            let code = if p.len() >= 3 && p[0..3] == fib_quant::COMPACT_MAGIC {
                fib_quant::FibCodeV1::from_compact_bytes(p, &profile).map_err(|e| {
                    crate::error::PolyKvError::DecompressionFailed(format!(
                        "fib compact decode failed: {}",
                        e
                    ))
                })?
            } else {
                serde_json::from_slice(p).map_err(|e| {
                    crate::error::PolyKvError::DecompressionFailed(format!(
                        "fib code deserialize failed: {}",
                        e
                    ))
                })?
            };
            codes.push(code);
        }
        quantizer.decode_batch_fast(&codes).map_err(|e| {
            crate::error::PolyKvError::DecompressionFailed(format!(
                "fib decode_batch_fast failed: {}",
                e
            ))
        })
    }

    fn decode_batch_compact(&self, payload: &[u8], seed: u64) -> Result<Option<Vec<Vec<f32>>>> {
        if payload.len() >= 4 && payload[0..4] == FIB_WIRE_BATCH_MAGIC {
            // New self-describing wire batch. from_wire_bytes reconstructs the
            // profile via paper_default (no training-param overrides), so its
            // profile_digest differs from our adapter's. Patch it to the adapter's
            // digest before calling decode_batch_fast which validates the field.
            let mut codes = decode_fib_batch_payload_wire(payload)?;
            let quantizer = self.build_quantizer(seed)?;
            let profile_digest = quantizer.profile().digest().map_err(|e| {
                crate::error::PolyKvError::DecompressionFailed(format!("fib profile digest: {e}"))
            })?;
            for code in &mut codes {
                code.profile_digest = profile_digest.clone();
            }
            let decoded = quantizer.decode_batch_fast(&codes).map_err(|e| {
                crate::error::PolyKvError::DecompressionFailed(format!(
                    "fib wire batch decode_batch_fast failed: {e}"
                ))
            })?;
            return Ok(Some(decoded));
        }
        if payload.len() >= 3 && payload[0..3] == FIB_BATCHED_MAGIC {
            // Legacy FB2 fallback for payloads written by older poly-kv versions.
            let quantizer = self.build_quantizer(seed)?;
            let codes = decode_fib_batch_payload(payload, quantizer.profile())?;
            let decoded = quantizer.decode_batch_fast(&codes).map_err(|e| {
                crate::error::PolyKvError::DecompressionFailed(format!(
                    "fib FB2 decode_batch_fast failed: {e}"
                ))
            })?;
            return Ok(Some(decoded));
        }
        Ok(None)
    }

    fn dim(&self) -> usize {
        self.dim
    }

    fn compression_ratio(&self) -> f64 {
        50.0
    }

    fn is_gpu_accelerated(&self) -> bool {
        // Device-level probe — kept for trait compatibility. The pool build
        // uses is_gpu_accelerated_for(n, d) for honest per-batch reporting.
        match self.build_quantizer(0) {
            Ok(q) => q.is_gpu_accelerated(),
            Err(_) => false,
        }
    }

    fn is_gpu_accelerated_for(&self, n: usize, d: usize) -> bool {
        match self.build_quantizer(0) {
            Ok(q) => q.is_gpu_accelerated_for(n, d),
            Err(_) => false,
        }
    }

    fn codebook_digest(&self, seed: u64) -> Option<String> {
        self.build_quantizer(seed)
            .ok()
            .map(|q| q.codebook_digest().to_string())
    }

    fn rotation_digest(&self, seed: u64) -> Option<String> {
        self.build_quantizer(seed)
            .ok()
            .map(|q| q.rotation_digest().to_string())
    }
}

/// Create a codec from a policy and vector dimension.
///
/// Returns the appropriate codec based on the codec_id in the policy.
/// If the required compression crate is unavailable, returns an error.
#[allow(clippy::too_many_arguments)]
pub fn create_codec(
    codec_id: &str,
    dim: usize,
    fib_config: Option<&crate::policy::FibConfig>,
    #[cfg_attr(not(feature = "turbo"), allow(unused_variables))] turbo_config: Option<
        &crate::policy::TurboConfig,
    >,
) -> Result<Box<dyn KVecCodec>> {
    match codec_id {
        crate::policy::CODEC_FIB_K4_N32 => {
            #[cfg(feature = "fib")]
            {
                let fc = fib_config.ok_or_else(|| {
                    crate::error::PolyKvError::InvalidPolicy("fib codec requires fib_config".into())
                })?;
                let adapter = FibQuantAdapter::new(
                    dim,
                    fc.k,
                    fc.n,
                    fc.training_samples,
                    fc.lloyd_restarts,
                    fc.lloyd_iterations,
                )?;
                Ok(Box::new(adapter))
            }
            #[cfg(not(feature = "fib"))]
            {
                Err(crate::error::PolyKvError::CodecUnavailable {
                    codec: crate::policy::CODEC_FIB_K4_N32.into(),
                    feature: "fib".into(),
                })
            }
        }
        crate::policy::CODEC_TURBO_8BIT => {
            #[cfg(feature = "turbo")]
            {
                let tc = turbo_config.ok_or_else(|| {
                    crate::error::PolyKvError::InvalidPolicy(
                        "turbo codec requires turbo_config".into(),
                    )
                })?;
                let adapter = TurboQuantAdapter::new(dim, tc.bits, tc.projections)?;
                Ok(Box::new(adapter))
            }
            #[cfg(not(feature = "turbo"))]
            {
                Err(crate::error::PolyKvError::CodecUnavailable {
                    codec: crate::policy::CODEC_TURBO_8BIT.into(),
                    feature: "turbo".into(),
                })
            }
        }
        crate::policy::CODEC_EXACT_FALLBACK => Ok(Box::new(ExactFallbackCodec::new(dim))),
        other => Err(crate::error::PolyKvError::InvalidPolicy(format!(
            "unknown codec id: {}",
            other
        ))),
    }
}
