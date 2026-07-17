use serde::{Deserialize, Serialize};

use crate::digest_compat::Digest;
use crate::ids_compat::AgentId;
use crate::policy::CompressionPolicy;
/// Schema version for receipts.
pub const RECEIPT_SCHEMA: &str = "poly_kv_receipt_v1";
/// Schema version for pool build receipts.
pub const POOL_BUILD_RECEIPT_SCHEMA: &str = "pool_build_receipt_v1";
/// Schema version for shell materialize receipts.
pub const SHELL_MATERIALIZE_RECEIPT_SCHEMA: &str = "shell_materialize_receipt_v1";
/// Schema version for injection receipts.
pub const INJECTION_RECEIPT_SCHEMA: &str = "injection_receipt_v1";
/// Schema version for compressed attention selection receipts.
pub const COMPRESSED_ATTENTION_SELECTION_RECEIPT_SCHEMA: &str =
    "compressed_attention_selection_receipt_v1";

/// Receipt produced when building a SharedKVPool.
///
/// Content-addressed via blake3 digest of canonical JSON.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PoolBuildReceipt {
    /// Stable schema marker.
    pub schema_version: String,
    /// Blake3 digest of the entire pool.
    pub pool_digest: Digest,
    /// Per-layer blake3 digests.
    pub layer_digests: Vec<Digest>,
    /// Digest of the fib-quant codebook.
    pub codebook_digest: Digest,
    /// Digest of the fib-quant rotation.
    pub rotation_digest: Digest,
    /// Total number of tokens in the pool.
    pub total_tokens: u32,
    /// Milliseconds spent on fib-quant build.
    pub fib_build_ms: u64,
    /// Total compressed pool size in bytes.
    pub pool_size_bytes: u64,
    /// Total raw f32 size in bytes.
    pub raw_size_bytes: u64,
    /// Compression ratio: raw / compressed.
    pub compression_ratio: f64,
    /// Snapshot of the policy used.
    pub policy_snapshot: CompressionPolicy,
    /// Seed used for construction.
    pub seeded_with: u64,
    /// Unix timestamp when built.
    pub built_at_unix: i64,
    /// Backend used: "cpu" or "gpu".
    #[serde(default = "default_backend")]
    pub backend: String,
}

fn default_backend() -> String {
    "cpu".to_string()
}

impl PoolBuildReceipt {
    /// Create a new pool build receipt.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool_digest: Digest,
        layer_digests: Vec<Digest>,
        codebook_digest: Digest,
        rotation_digest: Digest,
        total_tokens: u32,
        fib_build_ms: u64,
        pool_size_bytes: u64,
        raw_size_bytes: u64,
        policy_snapshot: CompressionPolicy,
        seeded_with: u64,
        built_at_unix: i64,
    ) -> Self {
        let compression_ratio = if pool_size_bytes > 0 {
            raw_size_bytes as f64 / pool_size_bytes as f64
        } else {
            0.0
        };

        Self {
            schema_version: POOL_BUILD_RECEIPT_SCHEMA.into(),
            pool_digest,
            layer_digests,
            codebook_digest,
            rotation_digest,
            total_tokens,
            fib_build_ms,
            pool_size_bytes,
            raw_size_bytes,
            compression_ratio,
            policy_snapshot,
            seeded_with,
            built_at_unix,
            backend: "cpu".to_string(),
        }
    }

    /// Set the backend used for this build.
    pub fn with_backend(mut self, backend: &str) -> Self {
        self.backend = backend.to_string();
        self
    }

    /// Validates this receipt schema version.
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.schema_version != POOL_BUILD_RECEIPT_SCHEMA {
            return Err(crate::error::PolyKvError::InvalidReceipt(format!(
                "expected schema {}, got {}",
                POOL_BUILD_RECEIPT_SCHEMA, self.schema_version
            )));
        }
        if self.pool_digest.hex().is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "pool_digest is empty".into(),
            ));
        }
        Ok(())
    }

    /// Compute the canonical digest of this receipt.
    pub fn digest(&self) -> crate::error::Result<Digest> {
        crate::digest_compat::compute_json(self)
    }
}

/// Receipt produced when materializing an AgentShell from a pool.
///
/// Content-addressed via blake3 digest of canonical JSON.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellMaterializeReceipt {
    /// Stable schema marker.
    pub schema_version: String,
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Digest of the parent pool.
    pub pool_digest: Digest,
    /// Digest of the materialized shell.
    pub shell_digest: Digest,
    /// Number of unique tokens in this shell.
    pub num_unique_tokens: u32,
    /// Total compressed shell size in bytes.
    pub shell_size_bytes: u64,
    /// Milliseconds spent on materialization.
    pub materialize_ms: u64,
    /// Unix timestamp when materialized.
    pub materialized_at_unix: i64,
}

impl ShellMaterializeReceipt {
    /// Create a new shell materialize receipt.
    pub fn new(
        agent_id: AgentId,
        pool_digest: Digest,
        shell_digest: Digest,
        num_unique_tokens: u32,
        shell_size_bytes: u64,
        materialize_ms: u64,
        materialized_at_unix: i64,
    ) -> Self {
        Self {
            schema_version: SHELL_MATERIALIZE_RECEIPT_SCHEMA.into(),
            agent_id,
            pool_digest,
            shell_digest,
            num_unique_tokens,
            shell_size_bytes,
            materialize_ms,
            materialized_at_unix,
        }
    }

    /// Validate this receipt.
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.schema_version != SHELL_MATERIALIZE_RECEIPT_SCHEMA {
            return Err(crate::error::PolyKvError::InvalidReceipt(format!(
                "expected schema {}, got {}",
                SHELL_MATERIALIZE_RECEIPT_SCHEMA, self.schema_version
            )));
        }
        if self.agent_id.is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "agent_id is empty".into(),
            ));
        }
        if self.pool_digest.hex().is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "pool_digest is empty".into(),
            ));
        }
        if self.shell_digest.hex().is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "shell_digest is empty".into(),
            ));
        }
        Ok(())
    }

    /// Compute the canonical digest of this receipt.
    pub fn digest(&self) -> crate::error::Result<Digest> {
        crate::digest_compat::compute_json(self)
    }
}

/// Receipt produced when selecting attention candidates from compressed KV artifacts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompressedAttentionSelectionReceipt {
    /// Stable schema marker.
    pub schema_version: String,
    /// Pool digest used for candidate selection.
    pub pool_digest: Digest,
    /// Layer index.
    pub layer: u32,
    /// KV head index.
    pub head: u32,
    /// Number of candidate tokens scored in compressed form.
    pub candidate_count: u32,
    /// Number of selected hits returned.
    pub selected_count: u32,
    /// Number of cold-pool candidates scored.
    #[serde(default)]
    pub pool_candidate_count: u32,
    /// Number of hot-shell candidates scored.
    #[serde(default)]
    pub shell_candidate_count: u32,
    /// Number of selected hits from the cold pool.
    #[serde(default)]
    pub selected_pool_count: u32,
    /// Number of selected hits from the hot shell.
    #[serde(default)]
    pub selected_shell_count: u32,
    /// Number of compressed key codes scored.
    pub compressed_key_scores: u64,
    /// Number of value vectors decoded after top-k selection.
    pub decoded_value_vectors: u64,
    /// True only when the full layer was decoded before scoring.
    pub full_layer_decoded: bool,
    /// Whether downstream model-quality claims require exact/logit/PPL fallback.
    #[serde(default = "default_exact_fallback_required")]
    pub exact_fallback_required: bool,
    /// Human-readable scoring path / claim boundary.
    pub scoring_path: String,
    /// Codec used by the cold shared pool.
    pub codec_id: String,
    /// Optional agent id when the selection includes a hot shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    /// Optional hot-shell digest when the selection includes a hot shell.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell_digest: Option<Digest>,
    /// Explicit claim boundary for this selection receipt.
    #[serde(default = "default_compressed_attention_claim_boundary")]
    pub claim_boundary: String,
    /// Unix timestamp when selected.
    pub selected_at_unix: i64,
}

fn default_exact_fallback_required() -> bool {
    true
}

fn default_compressed_attention_claim_boundary() -> String {
    "compressed candidate selection only; model-quality/KV-cache preservation claims require exact attention and logit/PPL replay receipts".to_string()
}

impl CompressedAttentionSelectionReceipt {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        pool_digest: Digest,
        layer: u32,
        head: u32,
        candidate_count: u32,
        selected_count: u32,
        compressed_key_scores: u64,
        decoded_value_vectors: u64,
        full_layer_decoded: bool,
        scoring_path: impl Into<String>,
        codec_id: impl Into<String>,
        selected_at_unix: i64,
    ) -> Self {
        Self {
            schema_version: COMPRESSED_ATTENTION_SELECTION_RECEIPT_SCHEMA.into(),
            pool_digest,
            layer,
            head,
            candidate_count,
            selected_count,
            pool_candidate_count: candidate_count,
            shell_candidate_count: 0,
            selected_pool_count: selected_count,
            selected_shell_count: 0,
            compressed_key_scores,
            decoded_value_vectors,
            full_layer_decoded,
            exact_fallback_required: true,
            scoring_path: scoring_path.into(),
            codec_id: codec_id.into(),
            agent_id: None,
            shell_digest: None,
            claim_boundary: default_compressed_attention_claim_boundary(),
            selected_at_unix,
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn with_shell_source_counts(
        mut self,
        agent_id: impl Into<String>,
        shell_digest: Digest,
        pool_candidate_count: u32,
        shell_candidate_count: u32,
        selected_pool_count: u32,
        selected_shell_count: u32,
    ) -> Self {
        self.agent_id = Some(agent_id.into());
        self.shell_digest = Some(shell_digest);
        self.pool_candidate_count = pool_candidate_count;
        self.shell_candidate_count = shell_candidate_count;
        self.selected_pool_count = selected_pool_count;
        self.selected_shell_count = selected_shell_count;
        self
    }

    pub fn validate(&self) -> crate::error::Result<()> {
        if self.schema_version != COMPRESSED_ATTENTION_SELECTION_RECEIPT_SCHEMA {
            return Err(crate::error::PolyKvError::InvalidReceipt(format!(
                "expected schema {}, got {}",
                COMPRESSED_ATTENTION_SELECTION_RECEIPT_SCHEMA, self.schema_version
            )));
        }
        if self.pool_digest.hex().is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "pool_digest is empty".into(),
            ));
        }
        if self.full_layer_decoded {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "compressed attention selection receipt must not claim full_layer_decoded".into(),
            ));
        }
        if self.selected_count as u64 != self.decoded_value_vectors {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "selected_count must equal decoded_value_vectors for top-k value decode".into(),
            ));
        }
        if self.pool_candidate_count + self.shell_candidate_count != self.candidate_count {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "source candidate counts must sum to candidate_count".into(),
            ));
        }
        if self.selected_pool_count + self.selected_shell_count != self.selected_count {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "selected source counts must sum to selected_count".into(),
            ));
        }
        if self.agent_id.is_some() != self.shell_digest.is_some() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "agent_id and shell_digest must be present together".into(),
            ));
        }
        if self.claim_boundary.trim().is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "claim_boundary is empty".into(),
            ));
        }
        Ok(())
    }

    pub fn digest(&self) -> crate::error::Result<Digest> {
        crate::digest_compat::compute_json(self)
    }
}

/// Receipt produced when injecting a shell into a KV cache.
///
/// Traces the source of every injected block.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InjectionReceipt {
    /// Stable schema marker.
    pub schema_version: String,
    /// Agent identifier.
    pub agent_id: AgentId,
    /// Digest of the parent pool.
    pub pool_digest: Digest,
    /// Digest of the injected shell.
    pub shell_digest: Digest,
    /// Number of injected blocks.
    pub blocks_injected: u32,
    /// Per-block digest traces (source -> target).
    pub block_traces: Vec<BlockInjectionTrace>,
    /// Unix timestamp when injection occurred.
    pub injected_at_unix: i64,
    /// Trace context for cross-boundary correlation (requires typed-ids).
    #[cfg(feature = "typed-ids")]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trace_ctx: Option<stack_ids::TraceCtx>,
}

/// Traces one block's injection from source (pool or shell) to target cache.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockInjectionTrace {
    /// Layer index.
    pub layer: u32,
    /// Source: "pool" or "shell".
    pub source: String,
    /// Digest of the source block.
    pub source_digest: Digest,
    /// Position in the target cache.
    pub target_position: u32,
}

impl InjectionReceipt {
    /// Create a new injection receipt.
    pub fn new(
        agent_id: AgentId,
        pool_digest: Digest,
        shell_digest: Digest,
        blocks_injected: u32,
        block_traces: Vec<BlockInjectionTrace>,
        injected_at_unix: i64,
    ) -> Self {
        Self {
            schema_version: INJECTION_RECEIPT_SCHEMA.into(),
            agent_id,
            pool_digest,
            shell_digest,
            blocks_injected,
            block_traces,
            injected_at_unix,
            #[cfg(feature = "typed-ids")]
            trace_ctx: None,
        }
    }

    /// Validate this receipt.
    pub fn validate(&self) -> crate::error::Result<()> {
        if self.schema_version != INJECTION_RECEIPT_SCHEMA {
            return Err(crate::error::PolyKvError::InvalidReceipt(format!(
                "expected schema {}, got {}",
                INJECTION_RECEIPT_SCHEMA, self.schema_version
            )));
        }
        if self.agent_id.is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "agent_id is empty".into(),
            ));
        }
        if self.pool_digest.hex().is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "pool_digest is empty".into(),
            ));
        }
        if self.shell_digest.hex().is_empty() {
            return Err(crate::error::PolyKvError::InvalidReceipt(
                "shell_digest is empty".into(),
            ));
        }
        Ok(())
    }

    /// Compute the canonical digest of this receipt.
    pub fn digest(&self) -> crate::error::Result<Digest> {
        crate::digest_compat::compute_json(self)
    }

    /// Builder: set the trace context (requires typed-ids).
    #[cfg(feature = "typed-ids")]
    pub fn with_trace_ctx(mut self, ctx: stack_ids::TraceCtx) -> Self {
        self.trace_ctx = Some(ctx);
        self
    }
}

/// Get the current unix timestamp as i64.
pub fn now_unix() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::CompressionPolicy;

    #[test]
    fn test_pool_build_receipt_round_trip() {
        let receipt = PoolBuildReceipt::new(
            Digest::from_hex_unchecked("abc123"),
            vec![
                Digest::from_hex_unchecked("layer0_digest"),
                Digest::from_hex_unchecked("layer1_digest"),
            ],
            Digest::from_hex_unchecked("codebook_digest"),
            Digest::from_hex_unchecked("rotation_digest"),
            100,
            42,
            10_000,
            500_000,
            CompressionPolicy::default_two_tier(),
            42,
            now_unix(),
        );
        assert!(receipt.validate().is_ok());

        let json = serde_json::to_string(&receipt).unwrap();
        let deser: PoolBuildReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(receipt.pool_digest, deser.pool_digest);
        assert_eq!(receipt.layer_digests, deser.layer_digests);
        assert_eq!(receipt.compression_ratio, deser.compression_ratio);
    }

    #[test]
    fn test_shell_receipt_round_trip() {
        let receipt = ShellMaterializeReceipt::new(
            AgentId::new("agent_1"),
            Digest::from_hex_unchecked("pool_abc"),
            Digest::from_hex_unchecked("shell_xyz"),
            50,
            5_000,
            10,
            now_unix(),
        );
        assert!(receipt.validate().is_ok());

        let json = serde_json::to_string(&receipt).unwrap();
        let deser: ShellMaterializeReceipt = serde_json::from_str(&json).unwrap();
        assert_eq!(receipt.shell_digest, deser.shell_digest);
    }

    #[test]
    fn test_injection_receipt_traces() {
        let traces = vec![
            BlockInjectionTrace {
                layer: 0,
                source: "pool".into(),
                source_digest: Digest::from_hex_unchecked("abc"),
                target_position: 0,
            },
            BlockInjectionTrace {
                layer: 0,
                source: "shell".into(),
                source_digest: Digest::from_hex_unchecked("def"),
                target_position: 1,
            },
        ];
        let receipt = InjectionReceipt::new(
            AgentId::new("agent_1"),
            Digest::from_hex_unchecked("pool_abc"),
            Digest::from_hex_unchecked("shell_xyz"),
            2,
            traces,
            now_unix(),
        );
        assert!(receipt.validate().is_ok());
        assert_eq!(receipt.blocks_injected, 2);
    }
}
