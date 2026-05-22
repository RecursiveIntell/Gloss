export interface Notebook {
  id: string;
  name: string;
  description?: string;
  directory: string;
  source_count: number;
  last_accessed?: string;
  created_at: string;
  updated_at: string;
}

export interface Source {
  id: string;
  source_type: string;
  title: string;
  original_filename?: string;
  file_hash?: string;
  url?: string;
  file_path?: string;
  content_text?: string;
  word_count?: number;
  metadata?: string;
  summary?: string;
  summary_model?: string;
  status: string;
  error_message?: string;
  selected: boolean;
  created_at: string;
  updated_at: string;
}

export interface Conversation {
  id: string;
  title?: string;
  style: string;
  custom_goal?: string;
  created_at: string;
  updated_at: string;
}

export interface Message {
  id: string;
  conversation_id: string;
  role: "user" | "assistant";
  content: string;
  citations?: Citation[] | ChatEvidencePayload | string;
  model_used?: string;
  tokens_prompt?: number;
  tokens_response?: number;
  created_at: string;
}

export interface Citation {
  chunk_id: string;
  source_id: string;
  source_title: string;
  quote?: string;
  page?: number;
  section?: string;
}

export interface ChatEvidenceDisclosure {
  backend_requested: string;
  backend_used: string;
  retrieval_mode: string;
  fallback_used: boolean;
  fallback_reason?: string | null;
  degradation_markers: string[];
  source_scope_mode: string;
  requested_source_ids: string[];
  selected_source_ids: string[];
  effective_source_ids: string[];
  invalid_source_ids: string[];
  excluded_source_ids: string[];
  invalid_source_count: number;
  effective_source_count: number;
  excluded_source_count: number;
  context_passage_count: number;
  citation_valid_count: number;
  citation_invalid_count: number;
  omitted_candidate_count: number;
  source_scope_preserved: boolean;
  index_status: string;
  link_status: string;
  receipt_id: string;
  semantic_memory_receipt_id?: string | null;
  candidate_backend?: string | null;
  turbo_quant_generation_id?: string | null;
  vector_artifact_manifest_digest?: string | null;
  exact_rerank?: boolean | null;
  exact_rerank_count?: number | null;
  approximate_candidate_count?: number | null;
  semantic_memory_fallback_reason?: string | null;
  retrieval_outcome?: RetrievalOutcome | null;
}

export interface ChatEvidencePayload {
  citations: Citation[];
  evidence: ChatEvidenceDisclosure;
}

export interface ChatEvidenceEventPayload extends ChatEvidencePayload {
  notebook_id: string;
  conversation_id: string;
  message_id: string;
}

export interface Note {
  id: string;
  title?: string;
  content: string;
  note_type: "manual" | "saved_response";
  citations?: Citation[] | string;
  pinned: boolean;
  source_id?: string;
  created_at: string;
  updated_at: string;
}

export interface StudioOutput {
  id: string;
  output_type: string;
  title?: string;
  prompt_used: string;
  raw_content?: string;
  config?: Record<string, unknown>;
  source_ids: string[];
  file_path?: string;
  status: string;
  error_message?: string;
  created_at: string;
}

export interface ModelInfo {
  id: string;
  provider: string;
  display_name: string;
  parameter_size?: string;
  context_window?: number;
}

export interface ModelRecord {
  id: string;
  provider_id: string;
  display_name: string;
  parameter_size?: string;
  context_window?: number;
  capabilities?: string;
  available: boolean;
  stale: boolean;
  last_error?: string;
}

export interface Provider {
  id: string;
  enabled: boolean;
  base_url?: string;
  has_api_key: boolean;
  last_refreshed?: string;
}

export interface FeatureFlagStatus {
  id: string;
  label: string;
  section: string;
  description: string;
  enabled: boolean;
  active: boolean;
  available: boolean;
  stable: boolean;
  default_enabled: boolean;
  requires_experimental: boolean;
  unavailable_reason?: string | null;
}

export interface SourceContent {
  content_text?: string;
  word_count?: number;
}

export type SourceScope =
  | { kind: "all" }
  | { kind: "explicit"; ids: string[] }
  | { kind: "none" };

export interface ChatTokenPayload {
  notebook_id: string;
  conversation_id: string;
  message_id: string;
  token: string;
  done: boolean;
}

export interface ChatStatusPayload {
  notebook_id: string;
  conversation_id: string;
  message_id: string;
  phase: string;
  message: string;
  provider?: string | null;
  model?: string | null;
  gate?: string | null;
  owner?: string | null;
  owner_detail?: string | null;
  elapsed_ms: number;
  timeout_ms?: number | null;
  truncated: boolean;
  error?: string | null;
  vector_artifact_receipt?: Record<string, unknown> | null;
}

export interface ChatAttemptTraceEvent {
  phase: string;
  recorded_at: string;
  elapsed_ms?: number | null;
  detail?: string | null;
  error?: string | null;
}

export interface ChatAttemptTraceV1 {
  schema: "ChatAttemptTraceV1";
  attempt_id: string;
  notebook_id: string;
  conversation_id: string;
  message_id: string;
  model: string;
  provider: string;
  provider_base_url?: string | null;
  memory_backend?: string | null;
  memory_backend_fallback?: boolean | null;
  source_scope_mode?: string | null;
  first_token_seen: boolean;
  done_seen: boolean;
  assistant_persisted: boolean;
  error?: string | null;
  retrieval_trace_ref?: string | null;
  retrieval_outcome?: RetrievalOutcome | null;
  events: ChatAttemptTraceEvent[];
}

export type RetrievalMode =
  | "bm25_only"
  | "dense_only"
  | "hybrid_rrf"
  | "semantic_memory"
  | "source_order_fallback"
  | "raw_content_fallback"
  | "unavailable";

export type RetrievalReasonCode =
  | "native_indexing_disabled"
  | "dense_engine_unavailable"
  | "embedder_unavailable"
  | "index_missing"
  | "no_embedded_chunks"
  | "partial_embedding_coverage"
  | "scope_has_missing_embeddings"
  | "semantic_memory_feature_disabled"
  | "semantic_memory_build_feature_missing"
  | "semantic_memory_links_missing"
  | "semantic_memory_links_degraded"
  | "semantic_memory_timeout"
  | "bm25_query_sanitized_empty"
  | "bm25_no_matches"
  | "source_order_fallback"
  | "raw_content_fallback"
  | "no_retrieval_context";

export interface RetrievalEngineStatus {
  engine: string;
  attempted: boolean;
  available: boolean;
  contributed: boolean;
  candidate_count: number;
  elapsed_ms: number;
  reason_code?: RetrievalReasonCode | null;
  detail?: string | null;
}

export interface RetrievalCoverage {
  selected_sources: number;
  total_chunks: number;
  fts_indexed_chunks: number;
  embedded_chunks: number;
  missing_embeddings: number;
  semantic_links_total: number;
  semantic_links_healthy: number;
  semantic_links_degraded: number;
  dense_coverage_ratio: number;
}

export interface RetrievalResult {
  chunk_id?: string | null;
  source_id: string;
  title?: string | null;
  content: string;
  score: number;
  engine: string;
}

export interface RetrievalOutcome {
  mode: RetrievalMode;
  results: RetrievalResult[];
  engines: RetrievalEngineStatus[];
  coverage: RetrievalCoverage;
  degraded: boolean;
  fallback_chain: RetrievalReasonCode[];
  user_visible_summary: string;
  trace_ref: string;
}

export interface SourceStatusPayload {
  notebook_id: string;
  source_id: string;
  status: string;
  error_message?: string;
}

export interface NotebookStats {
  source_count: number;
  ready_count: number;
  error_count: number;
  missing_summaries: number;
  chunk_count: number;
  sources_with_chunks: number;
  total_words: number;
}

export interface EmbeddingModelPayload {
  state: "downloading" | "ready";
  message: string;
}

export interface JobCompletedPayload {
  jobId: string;
  output: string | null;
}

export interface ChatErrorPayload {
  notebook_id: string;
  conversation_id: string;
  message_id: string;
  error: string;
}

export interface QueueStatus {
  paused: boolean;
  mode: string;
  pending: number;
  processing: number;
  completed: number;
  failed: number;
  gate_owners: RuntimeGateOwner[];
  summary_backend: BackgroundBackendStatus;
  vision_backend: BackgroundBackendStatus;
}

export interface RuntimeGateOwner {
  gate: string;
  owner: string;
  detail: string;
  since_ms: number;
}

export interface SourcesBatchCreatedPayload {
  notebook_id: string;
  count: number;
}

export interface BatchIngestionCompletePayload {
  notebook_id: string;
  count: number;
}

export interface BackgroundBackendStatus {
  ready: boolean;
  provider_id?: string | null;
  model?: string | null;
  diagnostic?: string | null;
}

export interface QueueSummariesResult {
  queued: number;
  diagnostics: string[];
}

export interface MemoryBackendStatus {
  backend_id: string;
  default_backend: string;
  active_backend: string;
  backend_used: string;
  available: boolean;
  semantic_memory_feature_enabled: boolean;
  semantic_memory_available: boolean;
  semantic_memory_path?: string | null;
  index_sync_status: string;
  sync_status: string;
  last_sync_at?: string | null;
  last_sync_error?: string | null;
  last_retrieval_receipt_id?: string | null;
  last_receipt_ref?: string | null;
  fallback_reason?: string | null;
  degradation_markers: string[];
  backend_version_or_digest?: string | null;
  degraded: boolean;
  diagnostic?: string | null;
}

export interface SemanticMemoryLinkStatus {
  notebook_id: string;
  total_links: number;
  synced_links: number;
  stale_links: number;
  failed_links: number;
  missing_document_links: number;
  degraded_links: number;
  reason_codes: string[];
  last_sync_error?: string | null;
}

export interface IndexSourceReceipt {
  backend_id: string;
  notebook_id: string;
  source_id: string;
  receipt_id: string;
  indexed_chunks: number;
  sync_status: string;
  error?: string | null;
}
