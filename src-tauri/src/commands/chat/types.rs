//! Receipt, evidence, and disclosure types for the chat pipeline.
//!
//! Extracted from chat/mod.rs to reduce file size and improve cohesion.

use crate::memory::{RetrievalCapabilityDecisionV1, RetrievalOutcome, RetrievalReasonCode};
use crate::retrieval::citations;
use serde::{Deserialize, Serialize};

pub(crate) type ChatAttemptId = String;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatStreamEventV1 {
    pub seq: u64,
    pub attempt_id: ChatAttemptId,
    pub kind: String,
    pub notebook_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub payload: serde_json::Value,
    pub recorded_at: String,
}

/// Acknowledges that the backend accepted a cancellation request. This is not
/// terminal state: the stream task remains the sole owner of the eventual
/// `chat:cancelled`, `chat:error`, or `chat:done` event.
#[derive(Debug, Clone, Serialize)]
pub struct ChatCancellationRequestV1 {
    pub attempt_id: String,
    pub conversation_id: String,
    pub message_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StopChatResponseV1 {
    pub cancellation_requested: bool,
    pub attempts: Vec<ChatCancellationRequestV1>,
}

// ---------------------------------------------------------------------------
// Chat evidence disclosure
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatEvidenceDisclosure {
    pub backend_requested: String,
    pub backend_used: String,
    pub retrieval_mode: String,
    pub fallback_used: bool,
    pub fallback_reason: Option<String>,
    pub fallback_reason_code: Option<RetrievalReasonCode>,
    pub degradation_markers: Vec<String>,
    pub source_scope_mode: String,
    pub requested_source_ids: Vec<String>,
    pub selected_source_ids: Vec<String>,
    pub effective_source_ids: Vec<String>,
    pub invalid_source_ids: Vec<String>,
    pub excluded_source_ids: Vec<String>,
    pub invalid_source_count: usize,
    pub effective_source_count: usize,
    pub excluded_source_count: usize,
    pub context_passage_count: usize,
    pub citation_valid_count: usize,
    pub citation_invalid_count: usize,
    pub citation_anchors: Vec<citations::CitationAnchorV1>,
    pub citation_filter_reasons: Vec<citations::CitationFilterReasonV1>,
    pub omitted_candidate_count: usize,
    pub source_scope_preserved: bool,
    pub index_status: String,
    pub link_status: String,
    pub receipt_id: String,
    pub context_digest: String,
    pub source_context_digest: String,
    pub prompt_digest: Option<String>,
    pub semantic_memory_receipt_id: Option<String>,
    pub candidate_backend: Option<String>,
    pub turbo_quant_generation_id: Option<String>,
    pub vector_artifact_manifest_digest: Option<String>,
    pub exact_rerank: Option<bool>,
    pub exact_rerank_count: Option<usize>,
    pub approximate_candidate_count: Option<usize>,
    pub semantic_memory_fallback_reason: Option<String>,
    pub retrieval_outcome: Option<RetrievalOutcome>,
    pub retrieval_capability_decision: RetrievalCapabilityDecisionV1,
    pub semantic_memory_runtime_truth: SemanticMemoryRuntimeTruthV1,
    pub decoding_settings_receipt: Option<DecodingSettingsReceiptV1>,
    pub prompt_receipt: Option<PromptReceiptV1>,
    pub generation_receipt: Option<GenerationReceiptV1>,
    pub prompt_budget_receipt: Option<PromptBudgetReceiptV1>,
}

// ---------------------------------------------------------------------------
// Semantic-memory runtime truth
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SemanticMemoryRuntimeTruthV1 {
    pub schema: String,
    pub receipt_id: String,
    pub build: serde_json::Value,
    pub settings: serde_json::Value,
    pub projection: serde_json::Value,
    pub turbo_quant: serde_json::Value,
    pub decision: RetrievalCapabilityDecisionV1,
}

// ---------------------------------------------------------------------------
// Source-scope integrity
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct SourceScopeIntegrityV1 {
    pub requested_ids_valid: bool,
    pub effective_ids_match_allowed_set: bool,
    pub no_out_of_scope_context: bool,
    pub no_unanchored_context: bool,
    pub fallback_class_allowed: bool,
    pub projection_links_preserved: bool,
    pub preserved: bool,
}

// ---------------------------------------------------------------------------
// Assistant message evidence
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct AssistantMessageEvidence {
    pub citations: Vec<citations::Citation>,
    pub evidence: ChatEvidenceDisclosure,
}

// ---------------------------------------------------------------------------
// Prompt-budget receipt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PromptBudgetReceiptV1 {
    pub model_context_window: u32,
    pub system_prompt_chars: usize,
    pub message_count: usize,
    pub source_passage_count: usize,
    pub prompt_digest: String,
    pub context_budgeted: bool,
    pub estimated_prompt_tokens: u32,
}

// ---------------------------------------------------------------------------
// LLM invocation receipt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct LlmInvocationReceiptV1 {
    pub provider: String,
    pub model: String,
    pub request_digest: String,
    pub response_digest: Option<String>,
    pub error: Option<String>,
}

// ---------------------------------------------------------------------------
// Provider decoding capability
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ProviderDecodingCapabilityV1 {
    pub supports_temperature: bool,
    pub supports_top_p: bool,
    pub supports_top_k: bool,
    pub supports_min_p: bool,
    pub supports_repeat_penalty: bool,
}

// ---------------------------------------------------------------------------
// Effective decoding settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct EffectiveDecodingSettingsV1 {
    pub temperature: f32,
    pub top_p: Option<f32>,
    pub top_k: Option<i64>,
    pub min_p: Option<f32>,
    pub repeat_penalty: Option<f32>,
    pub max_tokens: u32,
}

// ---------------------------------------------------------------------------
// Decoding-settings receipt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct DecodingSettingsReceiptV1 {
    pub schema: String,
    pub receipt_id: String,
    pub provider: String,
    pub model: String,
    pub requested: serde_json::Value,
    pub effective: EffectiveDecodingSettingsV1,
    pub unsupported_fields: Vec<String>,
    pub provider_capability: ProviderDecodingCapabilityV1,
    pub recorded_at: String,
}

// ---------------------------------------------------------------------------
// Prompt receipt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct PromptReceiptV1 {
    pub schema: String,
    pub receipt_id: String,
    pub notebook_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub prompt_digest: String,
    pub context_payload_digest: String,
    pub capture_state: String,
    pub redaction_state: String,
    pub system_prompt_digest: String,
    pub system_prompt_text: Option<String>,
    pub user_turn_digest: String,
    pub source_passage_count: usize,
    pub recorded_at: String,
}

// ---------------------------------------------------------------------------
// Generation receipt
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub(crate) struct GenerationReceiptV1 {
    pub schema: String,
    pub receipt_id: String,
    pub notebook_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub provider: String,
    pub model: String,
    pub provider_request_digest: String,
    pub response_digest: Option<String>,
    pub status: String,
    pub error: Option<String>,
    pub terminal_cause: Option<String>,
    pub done_frame_seen: bool,
    pub eof_seen: bool,
    pub partial_persisted: bool,
    pub chunks_seen: usize,
    pub prompt_receipt_id: String,
    pub decoding_settings_receipt_id: String,
    pub recorded_at: String,
}

// ---------------------------------------------------------------------------
// Chat stream result
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub(crate) struct ChatStreamResult {
    pub full_response: String,
    pub decoding_settings_receipt: DecodingSettingsReceiptV1,
    pub prompt_receipt: PromptReceiptV1,
    pub generation_receipt: GenerationReceiptV1,
    pub prompt_budget_receipt: Option<PromptBudgetReceiptV1>,
}

// ---------------------------------------------------------------------------
// Provider done terminal decision
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ProviderDoneTerminalDecision {
    pub terminal_cause: &'static str,
    pub done_frame_seen: bool,
    pub eof_seen: bool,
    pub emit_done_on_current_token: bool,
    pub break_stream_loop: bool,
}

pub(crate) fn provider_done_terminal_decision() -> ProviderDoneTerminalDecision {
    ProviderDoneTerminalDecision {
        terminal_cause: "provider_done_frame",
        done_frame_seen: true,
        eof_seen: false,
        emit_done_on_current_token: false,
        break_stream_loop: true,
    }
}

#[cfg(test)]
mod tests {
    use super::provider_done_terminal_decision;

    #[test]
    fn provider_done_frame_does_not_emit_done_before_persistence() {
        let decision = provider_done_terminal_decision();
        assert!(decision.done_frame_seen);
        assert!(decision.break_stream_loop);
        assert!(!decision.emit_done_on_current_token);
    }
}

// ---------------------------------------------------------------------------
// Projection readiness
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[allow(dead_code)]
pub(crate) struct ProjectionReadiness {
    pub ready: bool,
    pub reason_code: Option<crate::memory::RetrievalReasonCode>,
    pub user_action: Option<String>,
    pub scoped_sources: usize,
    pub scoped_chunks: usize,
    pub healthy_links: usize,
    pub missing_links: usize,
    pub skipped_no_chunks: usize,
}

// ---------------------------------------------------------------------------
// Context budget result
// ---------------------------------------------------------------------------

#[allow(dead_code)]
pub(crate) struct ContextBudgetResult {
    pub num_ctx: u32,
    pub needed: u32,
    pub prompt_tokens: u32,
    pub context_budgeted: bool,
}
