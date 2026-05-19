//! Stable codec profiles, compression policies, and receipts.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Stable codec profile schema shared across TurboQuant-family receipts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CodecProfileV1 {
    pub schema: String,
    pub crate_name: String,
    pub crate_version: String,
    pub codec_kind: String,
    pub dim: usize,
    pub bits: u8,
    pub projections: Option<usize>,
    pub seed: u64,
    pub rotation_kind: String,
    pub storage_layout: String,
    pub codebook_kind: Option<String>,
    pub qjl_enabled: bool,
    pub score_semantics: String,
    pub profile_digest: Option<String>,
    pub limitations: Vec<String>,
}

impl CodecProfileV1 {
    pub fn turbo(
        dim: usize,
        bits: u8,
        projections: usize,
        seed: u64,
        qjl_enabled: bool,
        rotation_kind: impl Into<String>,
    ) -> Self {
        let mut profile = Self {
            schema: "CodecProfileV1".into(),
            crate_name: "turbo-quant".into(),
            crate_version: env!("CARGO_PKG_VERSION").into(),
            codec_kind: "TurboQuant".into(),
            dim,
            bits,
            projections: if qjl_enabled { Some(projections) } else { None },
            seed,
            rotation_kind: rotation_kind.into(),
            storage_layout: if qjl_enabled {
                "polar_radii_f32_angles_bitpacked_qjl_signs_bitpacked".into()
            } else {
                "polar_radii_f32_angles_bitpacked".into()
            },
            codebook_kind: Some("uniform_angle_grid".into()),
            qjl_enabled,
            score_semantics: if qjl_enabled {
                "approximate_inner_product_polar_plus_qjl_residual".into()
            } else {
                "approximate_inner_product_polar_only".into()
            },
            profile_digest: None,
            limitations: vec![
                "compressed codes are derived sidecars, not canonical vectors".into(),
                "quality is workload-dependent and must be benchmarked".into(),
                "KV-cache use is experimental shadow-mode only".into(),
            ],
        };
        profile.profile_digest = Some(profile.compute_digest());
        profile
    }

    /// Deterministic non-cryptographic digest for profile identity.
    pub fn compute_digest(&self) -> String {
        let projections = self
            .projections
            .map_or_else(|| "none".to_string(), |value| value.to_string());
        let body = format!(
            "{}|{}|{}|{}|{}|{}|{}|{}|{}|{}|{}",
            self.schema,
            self.crate_name,
            self.crate_version,
            self.codec_kind,
            self.dim,
            self.bits,
            projections,
            self.seed,
            self.rotation_kind,
            self.storage_layout,
            self.qjl_enabled
        );
        format!("fnv1a64:{:016x}", fnv1a64(body.as_bytes()))
    }
}

/// Stable compression policy record for sidecar integrations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CompressionPolicyV1 {
    pub schema: String,
    pub profile: CodecProfileV1,
    pub canonical_vectors_required: bool,
    pub lossy_default_allowed: bool,
    pub exact_fallback_required: bool,
    pub benchmark_gate_required: bool,
}

impl CompressionPolicyV1 {
    pub fn sidecar_shadow(profile: CodecProfileV1) -> Self {
        Self {
            schema: "CompressionPolicyV1".into(),
            profile,
            canonical_vectors_required: true,
            lossy_default_allowed: false,
            exact_fallback_required: true,
            benchmark_gate_required: true,
        }
    }
}

/// Validation state bound into a compression receipt.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum ValidationState {
    Validated,
    Rejected,
}

/// Receipt emitted for a single derived compressed code.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct CompressionReceiptV1 {
    pub schema: String,
    pub profile: CodecProfileV1,
    pub source_digest: Option<String>,
    pub input_dim: usize,
    pub validation_state: ValidationState,
    pub encoded_bytes: usize,
    pub fp16_baseline_bytes: usize,
    pub fp32_baseline_bytes: usize,
    pub compression_ratio_vs_fp16: Option<f32>,
    pub compression_ratio_vs_fp32: f32,
    pub warnings: Vec<String>,
}

impl CompressionReceiptV1 {
    pub fn new(
        profile: CodecProfileV1,
        source_digest: Option<String>,
        input_dim: usize,
        encoded_bytes: usize,
        validation_state: ValidationState,
    ) -> Self {
        let fp16_baseline_bytes = input_dim * 2;
        let fp32_baseline_bytes = input_dim * 4;
        Self {
            schema: "CompressionReceiptV1".into(),
            profile,
            source_digest,
            input_dim,
            validation_state,
            encoded_bytes,
            fp16_baseline_bytes,
            fp32_baseline_bytes,
            compression_ratio_vs_fp16: if encoded_bytes > 0 {
                Some(fp16_baseline_bytes as f32 / encoded_bytes as f32)
            } else {
                None
            },
            compression_ratio_vs_fp32: if encoded_bytes > 0 {
                fp32_baseline_bytes as f32 / encoded_bytes as f32
            } else {
                0.0
            },
            warnings: vec![
                "receipt describes a derived sidecar code, not a replacement for source vectors"
                    .into(),
            ],
        }
    }
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
