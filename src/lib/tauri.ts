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
  SourceContent,
  NotebookStats,
  QueueStatus,
} from "./types";

// === Notebooks ===

export async function listNotebooks(): Promise<Notebook[]> {
  return invoke("list_notebooks");
}

export async function createNotebook(name: string): Promise<string> {
  return invoke("create_notebook", { name });
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

export async function addSourceFile(
  notebookId: string,
  path: string
): Promise<string> {
  return invoke("add_source_file", { notebookId, path });
}

export async function addSourceFolder(
  notebookId: string,
  path: string
): Promise<string[]> {
  return invoke("add_source_folder", { notebookId, path });
}

export async function addSourcePaste(
  notebookId: string,
  title: string,
  text: string
): Promise<string> {
  return invoke("add_source_paste", { notebookId, title, text });
}

export async function deleteSource(
  notebookId: string,
  sourceId: string
): Promise<void> {
  return invoke("delete_source", { notebookId, sourceId });
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

export async function sendMessage(
  notebookId: string,
  conversationId: string,
  query: string,
  selectedSourceIds: string[],
  model: string
): Promise<string> {
  return invoke("send_message", {
    notebookId,
    conversationId,
    query,
    selectedSourceIds,
    model,
  });
}

export async function getSuggestedQuestions(
  notebookId: string
): Promise<string[]> {
  return invoke("get_suggested_questions", { notebookId });
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

export async function updateSetting(
  key: string,
  value: string
): Promise<void> {
  return invoke("update_setting", { key, value });
}

// === Jobs ===

export async function regenerateMissingSummaries(
  notebookId: string
): Promise<number> {
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
