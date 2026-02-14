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
  citations?: Citation[];
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
}

export interface Provider {
  id: string;
  enabled: boolean;
  base_url?: string;
  has_api_key: boolean;
  last_refreshed?: string;
}

export interface SourceContent {
  content_text?: string;
  word_count?: number;
}

export interface ChatTokenPayload {
  conversation_id: string;
  message_id: string;
  token: string;
  done: boolean;
}

export interface SourceStatusPayload {
  notebook_id: string;
  source_id: string;
  status: string;
  error_message?: string;
}
