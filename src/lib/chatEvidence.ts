import type { ChatEvidenceDisclosure, ChatEvidencePayload } from "./types";

// Decode the persisted JSON envelope once at the IPC boundary. Legacy citation
// arrays retain their references, while uncaptured evidence stays unknown.
export function parseAssistantPayload(raw: unknown): ChatEvidencePayload {
  if (!raw) return { citations: [], evidence: nullEvidence() };
  try {
    const parsed = typeof raw === "string" ? JSON.parse(raw) : raw;
    if (Array.isArray(parsed)) {
      return { citations: parsed, evidence: nullEvidence(parsed.length) };
    }
    if (parsed && typeof parsed === "object" && Array.isArray(parsed.citations)) {
      return {
        citations: parsed.citations,
        evidence: parsed.evidence ?? nullEvidence(parsed.citations.length),
      };
    }
  } catch {
    return { citations: [], evidence: nullEvidence() };
  }
  return { citations: [], evidence: nullEvidence() };
}


function nullEvidence(citationCount = 0): ChatEvidenceDisclosure {
  return {
    backend_requested: "unknown",
    backend_used: "unknown",
    retrieval_mode: "unknown",
    fallback_used: false,
    fallback_reason: null,
    fallback_reason_code: null,
    degradation_markers: [],
    source_scope_mode: "unknown",
    requested_source_ids: [],
    selected_source_ids: [],
    effective_source_ids: [],
    invalid_source_ids: [],
    excluded_source_ids: [],
    invalid_source_count: 0,
    effective_source_count: 0,
    excluded_source_count: 0,
    context_passage_count: 0,
    citation_valid_count: citationCount,
    citation_invalid_count: 0,
    citation_anchors: [],
    citation_filter_reasons: [],
    omitted_candidate_count: 0,
    source_scope_preserved: false,
    index_status: "unknown",
    link_status: "unknown",
    receipt_id: "not recorded",
    context_digest: "",
    source_context_digest: "",
    prompt_digest: null,
    semantic_memory_receipt_id: null,
    candidate_backend: null,
    turbo_quant_generation_id: null,
    vector_artifact_manifest_digest: null,
    exact_rerank: null,
    exact_rerank_count: null,
    approximate_candidate_count: null,
    semantic_memory_fallback_reason: null,
    retrieval_outcome: null,
    retrieval_capability_decision: {
      requested_backend: "unknown",
      effective_backend: "unknown",
      decision_reason: null,
      decision_reason_code: null,
      build_feature_available: false,
      runtime_enabled: false,
      projection_ready: false,
      dense_ready: false,
      fallback_allowed: false,
      degraded: false,
    },
    semantic_memory_runtime_truth: {
      schema: "SemanticMemoryRuntimeTruthV1",
      receipt_id: "not recorded",
      build: {},
      settings: {},
      projection: {},
      turbo_quant: {},
      decision: {
        requested_backend: "unknown",
        effective_backend: "unknown",
        decision_reason: null,
        decision_reason_code: null,
        build_feature_available: false,
        runtime_enabled: false,
        projection_ready: false,
        dense_ready: false,
        fallback_allowed: false,
        degraded: false,
      },
    },
    decoding_settings_receipt: null,
    prompt_receipt: null,
    generation_receipt: null,
    prompt_budget_receipt: null,
  };
}
