import { invoke } from "@tauri-apps/api/core";
import type {
  Notebook,
  Source,

  Conversation,
  Message,
  Note,
  ModelInfo,
  ModelRecord,
  Provider,
  FeatureFlagStatus,
  SourceContent,
  NotebookStats,
  QueueStatus,
  SourceScope,
  QueueSummariesResult,
  MemoryBackendStatus,
  MemoryBackendProfileReceipt,
  SemanticMemoryLinkStatus,
  IndexSourceReceipt,
  SemanticMemoryBackfillReceipt,
  SemanticMemoryProfileStatus,
  RetrievalProbeReceipt,
  ChatAttemptTraceV1,
  ChatStreamEventV1,
  EmbeddingDiagnosticsReceipt,
  ExternalToolAvailabilityReceipt,
  FailedImportQuarantineReceipt,
  DbDoctorReceipt,
  NotebookExportReceipt,
  NotebookImportReceipt,
  NotebookPortableManifest,
  StudioOutput,
  StudioExportReceipt,
  StopChatResponseV1,
} from "./types";

// === Notebooks ===

export async function listNotebooks(): Promise<Notebook[]> {
  return invoke("list_notebooks");
}

export async function runDatabaseDoctor(repair: boolean): Promise<DbDoctorReceipt> {
  return invoke("run_database_doctor", { repair });
}

export async function exportNotebookArchive(
  notebookId: string,
  archivePath: string
): Promise<NotebookExportReceipt> {
  return invoke("export_notebook_archive", { notebookId, archivePath });
}

export async function validateNotebookImportArchive(
  archivePath: string
): Promise<NotebookPortableManifest> {
  return invoke("validate_notebook_import_archive", { archivePath });
}

export async function importNotebookArchive(
  archivePath: string,
  nameOverride?: string
): Promise<NotebookImportReceipt> {
  return invoke("import_notebook_archive", { archivePath, nameOverride });
}

export async function createNotebook(name: string): Promise<string> {
  return invoke("create_notebook", { name });
}

export async function renameNotebook(id: string, name: string): Promise<void> {
  return invoke("rename_notebook", { id, name });
}

export async function deleteNotebook(id: string): Promise<void> {
  return invoke("delete_notebook", { id });
}

export async function setActiveNotebook(notebookId: string | null): Promise<void> {
  return invoke("set_active_notebook", { notebookId });
}

// === Sources ===

export async function listSources(notebookId: string): Promise<Source[]> {
  return invoke("list_sources", { notebookId });
}

export async function setSelectedSources(
  notebookId: string,
  selectedSourceIds: string[]
): Promise<void> {
  return invoke("set_selected_sources", { notebookId, selectedSourceIds });
}

export async function addSourceFile(
  notebookId: string,
  path: string
): Promise<string> {
  return invoke("add_source_file", { notebookId, path });
}

export async function addSourceFiles(
  notebookId: string,
  paths: string[]
): Promise<string[]> {
  return invoke("add_source_files", { notebookId, paths });
}

export async function addSourceFolder(
  notebookId: string,
  path: string
): Promise<void> {
  return invoke("add_source_folder", { notebookId, path });
}

export async function addSourcePaste(
  notebookId: string,
  title: string,
  text: string
): Promise<string> {
  return invoke("add_source_paste", { notebookId, title, text });
}

export async function addSourceUrl(
  notebookId: string,
  url: string,
  networkConsent: boolean
): Promise<string> {
  return invoke("add_source_url", { notebookId, url, networkConsent });
}

export async function addSourceYouTubeTranscript(
  notebookId: string,
  url: string,
  language: string | null,
  networkConsent: boolean
): Promise<string> {
  return invoke("add_source_youtube_transcript", {
    notebookId,
    url,
    language,
    networkConsent,
  });
}

export async function deleteSource(
  notebookId: string,
  sourceId: string
): Promise<void> {
  return invoke("delete_source", { notebookId, sourceId });
}

export async function deleteSources(
  notebookId: string,
  sourceIds: string[]
): Promise<void> {
  return invoke("delete_sources", { notebookId, sourceIds });
}

export async function quarantineFailedImports(
  notebookId: string
): Promise<FailedImportQuarantineReceipt> {
  return invoke("quarantine_failed_imports", { notebookId });
}

export async function deleteFailedImports(
  notebookId: string
): Promise<FailedImportQuarantineReceipt> {
  return invoke("delete_failed_imports", { notebookId });
}

export async function getSourceContent(
  notebookId: string,
  sourceId: string
): Promise<SourceContent> {
  return invoke("get_source_content", { notebookId, sourceId });
}

export async function retrySourceIngestion(
  notebookId: string,
  sourceId: string
): Promise<void> {
  return invoke("retry_source_ingestion", { notebookId, sourceId });
}

export async function getNotebookStats(
  notebookId: string
): Promise<NotebookStats> {
  return invoke("get_notebook_stats", { notebookId });
}

export async function runRetrievalProbe(
  notebookId: string,
  query: string,
  sourceScope: SourceScope,
  limit?: number
): Promise<RetrievalProbeReceipt> {
  return invoke("run_retrieval_probe", {
    notebookId,
    query,
    sourceScope,
    limit,
  });
}

export async function listStudioOutputs(
  notebookId: string
): Promise<StudioOutput[]> {
  return invoke("list_studio_outputs", { notebookId });
}

export async function generateStudioOutput(
  notebookId: string,
  outputType: string,
  sourceIds?: string[],
  title?: string,
  maxItems?: number,
  attemptId?: string
): Promise<StudioOutput> {
  return invoke("generate_studio_output", {
    notebookId,
    outputType,
    sourceIds,
    title,
    maxItems,
    attemptId,
  });
}

export async function cancelStudioGeneration(
  notebookId: string,
  attemptId?: string
): Promise<boolean> {
  return invoke("cancel_studio_generation", { notebookId, attemptId });
}

export async function exportStudioOutput(
  notebookId: string,
  outputId: string
): Promise<StudioExportReceipt> {
  return invoke("export_studio_output", { notebookId, outputId });
}

// === Chat ===

export async function listConversations(
  notebookId: string
): Promise<Conversation[]> {
  return invoke("list_conversations", { notebookId });
}

export async function createConversation(
  notebookId: string
): Promise<string> {
  return invoke("create_conversation", { notebookId });
}

export async function deleteConversation(
  notebookId: string,
  conversationId: string
): Promise<void> {
  return invoke("delete_conversation", { notebookId, conversationId });
}

export async function loadMessages(
  notebookId: string,
  conversationId: string
): Promise<Message[]> {
  return invoke("load_messages", { notebookId, conversationId });
}

export async function getChatEventsSince(
  notebookId: string,
  conversationId: string,
  afterSeq?: number | null
): Promise<ChatStreamEventV1[]> {
  return invoke("get_chat_events_since", {
    notebookId,
    conversationId,
    afterSeq,
  });
}

export async function sendMessage(
  notebookId: string,
  conversationId: string,
  query: string,
  sourceScope: SourceScope,
  model: string,
  messageId?: string,
  style?: string,
  customGoal?: string,
  responseLength?: string,
): Promise<string> {
  return invoke("send_message", {
    notebookId,
    conversationId,
    query,
    sourceScope,
    model,
    messageId,
    style,
    customGoal,
    responseLength,
  });
}

export async function stopChat(notebookId: string): Promise<StopChatResponseV1> {
  return invoke("stop_chat", { notebookId });
}

export async function getSuggestedQuestions(
  notebookId: string
): Promise<string[]> {
  return invoke("get_suggested_questions", { notebookId });
}

export async function debugChatProviderSmoke(
  providerId: string,
  model: string,
  prompt?: string
): Promise<ChatAttemptTraceV1> {
  return invoke("debug_chat_provider_smoke", { providerId, model, prompt });
}

export async function getLastChatAttemptTrace(): Promise<ChatAttemptTraceV1 | null> {
  return invoke("get_last_chat_attempt_trace");
}

// === Notes ===

export async function listNotes(notebookId: string): Promise<Note[]> {
  return invoke("list_notes", { notebookId });
}

export async function createNote(
  notebookId: string,
  title: string,
  content: string
): Promise<string> {
  return invoke("create_note", { notebookId, title, content });
}

export async function saveResponseAsNote(
  notebookId: string,
  messageId: string
): Promise<string> {
  return invoke("save_response_as_note", { notebookId, messageId });
}

export async function updateNote(
  notebookId: string,
  noteId: string,
  title?: string,
  content?: string
): Promise<void> {
  return invoke("update_note", { notebookId, noteId, title, content });
}

export async function togglePin(
  notebookId: string,
  noteId: string
): Promise<void> {
  return invoke("toggle_pin", { notebookId, noteId });
}

export async function deleteNote(
  notebookId: string,
  noteId: string
): Promise<void> {
  return invoke("delete_note", { notebookId, noteId });
}

// === Settings ===

export async function getProviders(): Promise<Provider[]> {
  return invoke("get_providers");
}

export async function updateProvider(
  id: string,
  enabled: boolean,
  baseUrl?: string,
  apiKey?: string
): Promise<void> {
  return invoke("update_provider", {
    id,
    enabled,
    baseUrl,
    apiKey,
  });
}

export async function testProvider(providerId: string): Promise<boolean> {
  return invoke("test_provider", { providerId });
}

export async function refreshModels(
  providerId?: string
): Promise<ModelInfo[]> {
  return invoke("refresh_models", { providerId });
}

export async function getAllModels(): Promise<ModelRecord[]> {
  return invoke("get_all_models");
}

export async function getSettings(): Promise<Record<string, string>> {
  return invoke("get_settings");
}

export async function runEmbeddingDiagnostics(): Promise<EmbeddingDiagnosticsReceipt> {
  return invoke("run_embedding_diagnostics");
}

export async function updateSetting(
  key: string,
  value: string
): Promise<void> {
  return invoke("update_setting", { key, value });
}

export async function getFeatureFlags(): Promise<FeatureFlagStatus[]> {
  return invoke("get_feature_flags");
}

export async function updateFeatureFlag(
  id: string,
  enabled: boolean
): Promise<FeatureFlagStatus[]> {
  return invoke("update_feature_flag", { id, enabled });
}

export async function setMemoryBackendProfile(
  profile: string,
  notebookId?: string | null
): Promise<MemoryBackendProfileReceipt> {
  return invoke("set_memory_backend_profile", { profile, notebookId });
}

export async function checkExternalTools(): Promise<Record<string, ExternalToolAvailabilityReceipt>> {
  return invoke("check_external_tools");
}

// === Jobs ===

export async function regenerateMissingSummaries(
  notebookId: string
): Promise<QueueSummariesResult> {
  return invoke("regenerate_missing_summaries", { notebookId });
}

export async function pauseSummaries(): Promise<void> {
  return invoke("pause_summaries");
}

export async function resumeSummaries(): Promise<void> {
  return invoke("resume_summaries");
}

export async function getQueueStatus(): Promise<QueueStatus> {
  return invoke("get_queue_status");
}

export async function memoryBackendStatus(
  notebookId?: string | null
): Promise<MemoryBackendStatus> {
  return invoke("memory_backend_status", { notebookId });
}

export async function semanticMemoryLinkStatus(
  notebookId: string
): Promise<SemanticMemoryLinkStatus> {
  return invoke("semantic_memory_link_status", { notebookId });
}

export async function semanticMemoryReindexSource(
  notebookId: string,
  sourceId: string
): Promise<IndexSourceReceipt> {
  return invoke("semantic_memory_reindex_source", {
    notebookId,
    sourceId,
    traceId: crypto.randomUUID(),
  });
}

export async function semanticMemoryBackfillNotebook(
  notebookId: string
): Promise<SemanticMemoryBackfillReceipt> {
  return invoke("semantic_memory_backfill_notebook", { notebookId });
}

export async function semanticMemoryRebuildVectorArtifacts(
  notebookId: string
): Promise<Record<string, unknown> | null> {
  return invoke("semantic_memory_rebuild_vector_artifacts", { notebookId });
}

export async function getSemanticMemoryProfileStatus(
  notebookId?: string | null,
  sourceScope?: SourceScope | null
): Promise<SemanticMemoryProfileStatus> {
  return invoke("get_semantic_memory_profile_status", { notebookId, sourceScope });
}
