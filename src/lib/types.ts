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

export interface DbDoctorFinding {
  notebook_id: string;
  code: string;
  severity: "info" | "warning" | "error";
  count: number;
  repaired: boolean;
  detail: string;
}

export interface DbDoctorNotebookReport {
  notebook_id: string;
  notebook_db_present: boolean;
  source_count_recorded: number;
  source_count_actual?: number | null;
  orphan_source_processing_state_rows: number;
  orphan_projection_status_rows: number;
  orphan_semantic_memory_link_rows: number;
  failed_import_sources: number;
  quarantined_failed_import_sources: number;
  receipt_id?: string | null;
  supersedes_receipt_id?: string | null;
}

export interface DbDoctorReceipt {
  schema: "DbDoctorReceiptV1";
  receipt_id: string;
  repair: boolean;
  recorded_utc: string;
  notebooks_checked: number;
  findings: DbDoctorFinding[];
  notebook_reports: DbDoctorNotebookReport[];
  repaired_source_count_mismatches: number;
  repaired_orphan_rows: number;
  failed_import_sources: number;
  quarantined_failed_import_sources: number;
  queue_jobs_checked: number;
  stale_queue_jobs: number;
  repaired_stale_queue_jobs: number;
}

export interface PortableFileManifestEntry {
  path: string;
  sha256: string;
  byte_len: number;
}

export interface NotebookPortableManifest {
  schema: "NotebookPortableManifestV1";
  package_id: string;
  exported_utc: string;
  source_notebook_id: string;
  notebook_name: string;
  files: PortableFileManifestEntry[];
  manifest_digest: string;
}

export interface NotebookExportReceipt {
  schema: "NotebookExportReceiptV1";
  receipt_id: string;
  package_id: string;
  notebook_id: string;
  package_format: "directory" | "tar_gzip";
  package_dir: string;
  archive_path?: string | null;
  manifest_path: string;
  file_count: number;
  manifest_digest: string;
  recorded_utc: string;
}

export interface NotebookImportReceipt {
  schema: "NotebookImportReceiptV1";
  receipt_id: string;
  package_id: string;
  source_notebook_id: string;
  imported_notebook_id: string;
  imported_notebook_dir: string;
  file_count: number;
  manifest_digest: string;
  recorded_utc: string;
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
  processing_state?: SourceProcessingState | null;
}

export interface SourceProcessingState {
  source_id: string;
  lifecycle_status: string;
  summary_status: string;
  fts_index_status: string;
  dense_index_status: string;
  semantic_projection_status: string;
  last_summary_receipt_id?: string | null;
  last_dense_index_receipt_id?: string | null;
  last_projection_receipt_id?: string | null;
  last_error?: string | null;
  updated_at: string;
}

export interface FailedImportQuarantineReceipt {
  schema: "FailedImportQuarantineReceiptV1";
  receipt_id: string;
  notebook_id: string;
  action: "quarantine" | "delete";
  failed_sources_before: number;
  affected_sources: number;
  quarantined_sources: number;
  deleted_sources: number;
  cancelled_queue_jobs: number;
  recorded_utc: string;
}

export interface YouTubeTranscriptSpan {
  start_ms: number;
  end_ms: number;
}

export interface YouTubeTranscriptReceipt {
  schema: "YouTubeTranscriptReceiptV1";
  receipt_id: string;
  original_url_digest: string;
  watch_url_digest: string;
  video_id_digest: string;
  language: string;
  transcript_source: string;
  transcript_url_host: string;
  segment_count: number;
  timestamp_spans: YouTubeTranscriptSpan[];
  bytes_read: number;
  elapsed_ms: number;
  network_consent: boolean;
  max_bytes: number;
  max_segments: number;
}

export type ImportSupport =
  | "supported"
  | "supported_degraded"
  | "deferred"
  | "unsupported";

export interface ImportCapability {
  key: string;
  label: string;
  extensions: string[];
  source_type?: string | null;
  language?: string | null;
  support: ImportSupport;
  receipt_schema: string;
  reason: string;
}

export interface EmbeddingDiagnosticsReceipt {
  native_fastembed: {
    init_ok: boolean;
    embed_one_ok: boolean;
    dims?: number | null;
    cache_dir: string;
    error?: string | null;
  };
  semantic_memory_provider: {
    provider: string;
    dims: number;
    model: string;
  };
  optional_ollama: {
    configured: boolean;
    url?: string | null;
    embed_ok?: boolean | null;
  };
}

export interface ToolInvocationReceiptV1 {
  schema: "ToolInvocationReceiptV1";
  receipt_id: string;
  tool: string;
  action: string;
  args_redacted: string[];
  timeout_ms: number;
  elapsed_ms: number;
  exit_code?: number | null;
  success: boolean;
  timed_out: boolean;
  stderr_sha256?: string | null;
  stderr_len: number;
  stderr_preview?: string | null;
  stdout_sha256?: string | null;
  stdout_len: number;
}

export interface ExternalToolAvailabilityReceipt {
  available: boolean;
  receipt: ToolInvocationReceiptV1;
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
  citations?: ChatEvidencePayload;
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

export interface CitationFilterReasonV1 {
  ref_number: number;
  reason_code: string;
  detail: string;
}

export interface CitationAnchorV1 {
  ref_number: number;
  source_id: string;
  chunk_id: string;
  quote_digest: string;
  evidence_class: string;
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
  citation_anchors: CitationAnchorV1[];
  citation_filter_reasons: CitationFilterReasonV1[];
  omitted_candidate_count: number;
  source_scope_preserved: boolean;
  index_status: string;
  link_status: string;
  receipt_id: string;
  context_digest: string;
  source_context_digest: string;
  prompt_digest?: string | null;
  semantic_memory_receipt_id?: string | null;
  candidate_backend?: string | null;
  turbo_quant_generation_id?: string | null;
  vector_artifact_manifest_digest?: string | null;
  exact_rerank?: boolean | null;
  exact_rerank_count?: number | null;
  approximate_candidate_count?: number | null;
  semantic_memory_fallback_reason?: string | null;
  retrieval_outcome?: RetrievalOutcome | null;
  retrieval_capability_decision: RetrievalCapabilityDecisionV1;
  semantic_memory_runtime_truth: SemanticMemoryRuntimeTruthV1;
  decoding_settings_receipt?: DecodingSettingsReceiptV1 | null;
  prompt_receipt?: PromptReceiptV1 | null;
  generation_receipt?: GenerationReceiptV1 | null;
  prompt_budget_receipt?: PromptBudgetReceiptV1 | null;
}

export interface RetrievalCapabilityDecisionV1 {
  requested_backend: string;
  effective_backend: string;
  decision_reason?: string | null;
  build_feature_available: boolean;
  runtime_enabled: boolean;
  projection_ready: boolean;
  dense_ready: boolean;
  fallback_allowed: boolean;
  degraded: boolean;
}

export interface SemanticMemoryRuntimeTruthV1 {
  schema: "SemanticMemoryRuntimeTruthV1";
  receipt_id: string;
  build: Record<string, unknown>;
  settings: Record<string, unknown>;
  projection: Record<string, unknown>;
  turbo_quant?: Record<string, unknown> | null;
  decision: RetrievalCapabilityDecisionV1;
}

export interface PromptBudgetReceiptV1 {
  schema: "PromptBudgetReceiptV1";
  receipt_id: string;
  model_context_window: number;
  system_prompt_chars: number;
  message_count: number;
  source_passage_count: number;
  prompt_digest: string;
  context_budgeted: boolean;
  estimated_prompt_tokens: number;
  recorded_at: string;
}

export interface ProviderModelTestResult {
  provider_healthy: boolean;
  model_found: boolean;
  model_available: boolean;
  model_list_error?: string | null;
  model_list_count: number;
}

export interface DecodingSettingsReceiptV1 {
  schema: "DecodingSettingsReceiptV1";
  receipt_id: string;
  provider: string;
  model: string;
  requested: Record<string, unknown>;
  effective: {
    temperature: number;
    top_p?: number | null;
    top_k?: number | null;
    min_p?: number | null;
    repeat_penalty?: number | null;
    max_tokens: number;
  };
  unsupported_fields: string[];
  provider_capability: Record<string, boolean>;
  recorded_at: string;
}

export interface PromptReceiptV1 {
  schema: "PromptReceiptV1";
  receipt_id: string;
  notebook_id: string;
  conversation_id: string;
  message_id: string;
  prompt_digest: string;
  context_payload_digest: string;
  capture_state: string;
  redaction_state: string;
  system_prompt_digest: string;
  user_turn_digest: string;
  source_passage_count: number;
  recorded_at: string;
}

export interface GenerationReceiptV1 {
  schema: "GenerationReceiptV1";
  receipt_id: string;
  notebook_id: string;
  conversation_id: string;
  message_id: string;
  provider: string;
  model: string;
  provider_request_digest: string;
  response_digest?: string | null;
  status: string;
  error?: string | null;
  terminal_cause?: string | null;
  done_frame_seen: boolean;
  eof_seen: boolean;
  partial_persisted: boolean;
  chunks_seen: number;
  prompt_receipt_id: string;
  decoding_settings_receipt_id: string;
  recorded_at: string;
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
  citations?: Citation[];
  pinned: boolean;
  source_id?: string;
  created_at: string;
  updated_at: string;
}

export interface StudioOutputConfig {
  schema?: "StudioOutputConfigV1";
  deterministic?: boolean;
  source_bound?: boolean;
  schema_validated?: boolean;
  all_items_source_cited?: boolean;
  max_items?: number;
  receipt_id?: string;
  mode?: string;
  model?: string;
  provider?: string;
  source_ids?: string[];
  temperature?: number;
  max_tokens?: number;
}

export interface StudioOutput {
  id: string;
  output_type: string;
  title?: string;
  prompt_used: string;
  raw_content?: string;
  config?: StudioOutputConfig;
  source_ids: string[];
  file_path?: string;
  status: string;
  error_message?: string;
  created_at: string;
}

export interface StudioExportReceipt {
  schema: "StudioExportReceiptV1";
  receipt_id: string;
  output_id: string;
  output_type: string;
  notebook_id: string;
  format: "json";
  file_path: string;
  file_path_redacted: string;
  bytes_written: number;
  sha256: string;
  recorded_utc: string;
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
  vector_artifact_receipt?: VectorArtifactReceipt | null;
}

export interface ChatAttemptTraceEvent {
  phase: string;
  recorded_at: string;
  elapsed_ms?: number | null;
  detail?: string | null;
  error?: string | null;
}

export interface NetworkScopeReceiptV1 {
  schema: "NetworkScopeReceiptV1";
  provider: string;
  base_url: string;
  host: string;
  egress_class: string;
  policy: string;
  cloud_opt_in_required: boolean;
  lan_opt_in_applied: boolean;
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

export interface ChatCancelledPayload {
  notebook_id: string;
  conversation_id: string;
  message_id: string;
  reason: string;
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
  import_batch_id?: string;
  notebook_epoch?: number;
  count: number;
  found?: number;
  created?: number;
}

export interface ImportBatchPerformanceReceipt {
  schema: "ImportBatchPerformanceReceiptV1";
  elapsed_ms: number;
  scan_ms: number;
  source_create_ms: number;
  ingestion_ms: number;
  index_save_ms: number;
  found_per_second: number;
  created_per_second: number;
  ingested_ready_per_second: number;
}

export interface BatchIngestionCompletePayload {
  schema?: "ImportBatchReceiptV1";
  notebook_id: string;
  import_batch_id?: string;
  notebook_epoch?: number;
  status?: 'completed' | 'completed_empty' | 'empty' | 'failed' | 'cancelled_superseded' | string;
  count: number;
  found?: number;
  created?: number;
  ingested_ready?: number;
  failed?: number;
  skipped_duplicate?: number;
  skipped_unsupported?: number;
  cancelled_superseded?: number;
  message?: string | null;
  performance?: ImportBatchPerformanceReceipt | null;
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

export interface MemoryBackendProfileReceipt {
  profile: string;
  requested_backend: string;
  backend_used: string;
  strict_mode: boolean;
  semantic_memory_auto_project: boolean;
  turbo_quant_requested: boolean;
  turbo_quant_active: boolean;
  blocked: boolean;
  next_action?: string | null;
  blocking_reasons: string[];
  receipt_id: string;
  status: MemoryBackendStatus;
}

export interface SemanticMemoryProjectionSummary {
  notebook_id: string;
  total_sources: number;
  chunk_bearing_sources: number;
  zero_chunk_sources: number;
  projected_sources: number;
  failed_sources: number;
  skipped_no_chunks: number;
  stale_sources: number;
  partial_sources: number;
  projecting_sources: number;
  healthy_links: number;
  degraded_links: number;
  missing_links: number;
  total_chunks: number;
  projected_chunks: number;
  projection_required: boolean;
}

export interface VectorArtifactStatus {
  compiled_turbo_quant: boolean;
  runtime_turbo_quant_enabled: boolean;
  candidate_backend?: string | null;
  artifact_generation_id?: string | null;
  vector_artifact_manifest_digest?: string | null;
  vector_artifact_missing_count: number;
  vector_artifact_stale_count: number;
  exact_rerank: boolean;
  exact_rerank_count: number;
  last_receipt_id?: string | null;
  last_error?: string | null;
}

export interface VectorArtifactReceipt {
  schema?: string;
  receipt_id?: string;
  notebook_id?: string;
  backend_id?: string;
  candidate_backend?: string;
  artifact_generation_id?: string;
  vector_artifact_manifest_digest?: string | null;
  indexed_chunks?: number;
  projected_chunks?: number;
  total_chunks?: number;
  elapsed_ms?: number;
  recorded_at?: string;
  status?: string;
  error?: string | null;
}

export interface RetrievalProbeReceipt {
  receipt_id: string;
  notebook_id: string;
  query_digest: string;
  source_scope_kind: string;
  scoped_sources: number;
  scoped_chunks: number;
  backend_requested: string;
  backend_used: string;
  bm25_candidates: number;
  vector_candidates: number;
  tq_candidates: number;
  candidate_backend?: string | null;
  artifact_generation_id?: string | null;
  vector_artifact_manifest_digest?: string | null;
  exact_rerank: boolean;
  exact_rerank_count: number;
  fallback_used: boolean;
  fallback_reason?: string | null;
  degradation_markers: string[];
}

export interface SemanticMemoryProfileStatus {
  compiled_semantic_memory: boolean;
  compiled_turbo_quant: boolean;
  experimental_enabled: boolean;
  semantic_memory_flag_enabled: boolean;
  turbo_quant_flag_enabled: boolean;
  selected_backend: string;
  effective_backend: string;
  fallback_allowed: boolean;
  strict_testing: boolean;
  projection_summary?: SemanticMemoryProjectionSummary | null;
  turbo_quant_status?: VectorArtifactStatus | null;
  next_actions: string[];
  blocking_reasons: string[];
}

export interface SemanticMemoryBackfillReceipt {
  notebook_id: string;
  receipt_id: string;
  total_sources: number;
  chunk_bearing_sources: number;
  projected_sources: number;
  skipped_no_chunks: number;
  failed_sources: number;
  stale_sources: number;
  total_chunks: number;
  projected_chunks: number;
  errors: Array<{ source_id: string; title: string; error: string }>;
  vector_artifact_receipt?: VectorArtifactReceipt | null;
  projection_summary: SemanticMemoryProjectionSummary;
}

export interface RetrievalDiagnostics {
  query: string;
  scope_kind: string;
  scoped_sources: number;
  scoped_chunks: number;
  fts_indexed_chunks: number;
  bm25_hit_count: number;
  semantic_links_total: number;
  semantic_links_healthy: number;
  semantic_links_missing: number;
  semantic_links_degraded: number;
  semantic_search_attempted: boolean;
  semantic_candidate_count: number;
  candidate_backend?: string | null;
  fallback_allowed: boolean;
  fallback_used: boolean;
  fallback_reason?: string | null;
  retrieval_mode: string;
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
  vector_artifact_receipt?: VectorArtifactReceipt | null;
}
