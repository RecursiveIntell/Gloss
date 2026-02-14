# AGENTS.md — Sub-Agent Delegation Rules

This file defines how to decompose Gloss implementation work into sub-agent tasks. Each agent operates on a bounded scope, produces verifiable output, and must not cross into another agent's domain.

## Agent Roles

### `backend-db` — Database Layer Agent
**Scope:** Everything in `src-tauri/src/db/`
**Inputs:** Schema definitions from SPEC §3.2, §3.3
**Responsibilities:**
- `app_db.rs`: Create/open `gloss.db`, all CRUD for `notebooks`, `providers`, `models`, `settings` tables
- `notebook_db.rs`: Create/open per-notebook `notebook.db`, all CRUD for `sources`, `chunks`, `conversations`, `messages`, `notes`, `studio_outputs`, `notebook_config` tables
- `migrations.rs`: Schema creation with version tracking, WAL mode, foreign keys, FTS5 triggers
- Connection pooling: Each notebook gets one connection. App DB has one connection. Both in `AppState`.

**Verification:**
```rust
#[cfg(test)]
mod tests {
    // For every table: insert, read, update, delete, list
    // FTS5: insert chunk, search, verify results
    // Migrations: create fresh DB, verify all tables exist
    // Edge cases: duplicate file_hash rejection, cascade deletes
}
```

**Must NOT:** Make LLM calls. Touch the frontend. Import provider or ingestion modules.

---

### `backend-providers` — LLM Provider Agent
**Scope:** Everything in `src-tauri/src/providers/`
**Inputs:** Provider trait from SPEC §4.1, API specs from SPEC §4.2
**Responsibilities:**
- `mod.rs`: `LlmProvider` trait, `ProviderType` enum, `ModelRegistry`, `ChatRequest`, `ChatToken` types
- `ollama.rs`: Ollama implementation — list models (`GET /api/tags`), streaming chat (`POST /api/chat`), health check, model unload (`keep_alive: "0"`)
- `openai.rs`: OpenAI implementation — list models, streaming chat (SSE parsing), API key auth
- `anthropic.rs`: Anthropic implementation — list models, streaming chat (SSE with `content_block_delta`), `system` as top-level field, `x-api-key` + `anthropic-version` headers

**Verification:**
```rust
#[cfg(test)]
mod tests {
    // Unit tests with mock HTTP responses (use a local mock server or recorded responses)
    // Parse real Ollama /api/tags response JSON
    // Parse SSE stream fragments for OpenAI and Anthropic
    // Error handling: connection refused, 401, 404 model, rate limit, malformed JSON
}
```

**Must NOT:** Access the database directly. Handle UI events. Know about notebooks or sources.

---

### `backend-ingestion` — Ingestion Pipeline Agent
**Scope:** Everything in `src-tauri/src/ingestion/`
**Inputs:** Pipeline from SPEC §5, chunking config from SPEC §5.2
**Responsibilities:**
- `extract.rs`: Dispatch by `source_type` → return `content_text`. PDF (lopdf), DOCX (zip + XML), XLSX/CSV (calamine), URL (reqwest + readability), YouTube (yt-transcript-rs), plain text (fs::read).
- `chunk.rs`: `ChunkConfig` struct, `ChunkBoundary` enum, recursive character split. Respect paragraph > line > sentence > word boundaries. Attach metadata (page, section, chunk_index). Overlap between adjacent chunks.
- `embed.rs`: Wrap fastembed `TextEmbedding`. Batch embed chunks. Add to usearch index. Record `embedding_id` per chunk.
- `summarize.rs`: Single-pass summary for small sources, map-reduce for large sources. Generate suggested questions.

**Verification:**
```rust
#[cfg(test)]
mod tests {
    // Chunking: verify chunk sizes within bounds, overlap correct, boundaries respected
    // Chunking: markdown with headings splits on headings
    // Chunking: very short text produces 1 chunk (not zero)
    // Extract: text file round-trip
    // Extract: handle empty file gracefully
    // Embed: verify vector dimensions (768 for NomicEmbedTextV15)
}
```

**Must NOT:** Make chat LLM calls (only summary/question generation calls). Access conversation or studio tables. Handle streaming.

---

### `backend-retrieval` — Search & RAG Agent
**Scope:** Everything in `src-tauri/src/retrieval/`
**Inputs:** Retrieval pipeline from SPEC §7, context assembly from SPEC §7.2
**Responsibilities:**
- `hybrid_search.rs`: HNSW vector search + FTS5 BM25 search + Reciprocal Rank Fusion. Source filtering (only selected sources). Deduplication by chunk_id.
- `reranker.rs`: fastembed `TextRerank` (BGERerankerBase). Rerank top-30 → top-6.
- `context.rs`: `ContextBudget` calculator. Dynamic chunk/history allocation based on model context window. Assemble system prompt + chunks + history + query.
- `citations.rs`: Parse `[1]`, `[2]` references from LLM output. Map to chunk_id/source_id. Build `Citation` structs.

**Verification:**
```rust
#[cfg(test)]
mod tests {
    // RRF: known inputs produce expected merged ranking
    // Context budget: small window → fewer chunks. Large window → more chunks.
    // Citation parsing: "[1] Title, p.5" → Citation { source_title: "Title", page: 5 }
    // Citation parsing: handle missing/malformed citations gracefully (warn, don't crash)
    // Source filtering: deselected sources excluded from results
}
```

**Must NOT:** Know about ingestion. Access studio tables. Manage provider connections.

---

### `backend-studio` — Studio Output Agent
**Scope:** Everything in `src-tauri/src/studio/`
**Inputs:** Studio system from SPEC §6, templates from SPEC §6.2
**Responsibilities:**
- `mod.rs`: Universal pipeline — load template → build prompt (fill source summaries + options) → LLM generate → parse output (JSON or markdown) → validate → store in `studio_outputs` table.
- `templates.rs`: Load and parse `prompts/studio_templates.toml`. Template struct with `system_prompt`, `user_prompt_template`, `output_format`, `json_schema`.
- `audio.rs`: Audio overview pipeline — key point extraction → script generation → Piper TTS per segment → WAV concatenation via hound. (Phase 4 only)

**Verification:**
```rust
#[cfg(test)]
mod tests {
    // Template loading: parse all templates from TOML, verify required fields
    // Prompt building: template variables substituted correctly
    // JSON parsing: valid JSON passes, malformed triggers retry
    // JSON parsing: extract JSON from markdown fences
    // Output storage: verify studio_outputs row created with correct fields
}
```

**Must NOT:** Implement retrieval logic. Handle chat conversations. Manage providers directly (use them through a passed-in reference).

---

### `frontend-core` — UI Shell Agent
**Scope:** `src/App.tsx`, `src/stores/`, `src/lib/`, `src/hooks/`, `src/components/layout/`
**Responsibilities:**
- App shell layout (sidebar + 3-panel workspace)
- All Zustand stores (matching interfaces in SPEC §8.2)
- Tauri IPC wrappers in `src/lib/tauri.ts` (one typed function per Rust command)
- Event listeners in `src/lib/events.ts` (chat:token, job:*, source:status, studio:*)
- Custom hooks: `useTauriEvent`, `useStreamingChat`, `useJobProgress`
- Panel resizing (CSS grid or a lightweight splitter)

**Must NOT:** Implement individual renderers (those are separate agents). Make direct `invoke()` calls — always go through `lib/tauri.ts`.

---

### `frontend-panels` — Panel Component Agent
**Scope:** `src/components/notebooks/`, `src/components/sources/`, `src/components/chat/`, `src/components/notes/`, `src/components/settings/`
**Responsibilities:**
- NotebookSidebar, NotebookCard
- SourcesPanel (upload zone, source list, source cards, source detail viewer)
- ChatPanel (message list with streaming, citation badges, input, conversation selector, settings)
- NotesPanel (note list, editor, pin/unpin)
- SettingsDialog (providers, models, tools, appearance)

**Must NOT:** Implement studio renderers. Contain business logic — delegates to stores. Make direct LLM calls.

---

### `frontend-studio` — Studio Renderer Agent
**Scope:** `src/components/studio/`
**Responsibilities:**
- StudioPanel, StudioGrid (tile grid of output types), GenerateDialog
- Individual renderers: MarkdownViewer, FlashcardWidget, QuizWidget, MindMapGraph, TimelineView, DataTableView, SlideViewer, InfographicView, AudioPlayer
- Each renderer is a self-contained component that receives structured data (JSON or markdown string) as props and renders it.
- Export capabilities where applicable (CSV for DataTable, PNG for MindMap/Infographic, PDF for Slides)

**Must NOT:** Make LLM calls. Access the database. Know about the generation pipeline.

---

## Delegation Strategy

### Phase 1 (Foundation)
Execute in this order:
1. `backend-db` — Full schema, all tables, migrations, tests ← **everything else depends on this**
2. `backend-providers` — Ollama provider only (OpenAI/Anthropic in Phase 2)
3. `backend-ingestion` — Text/markdown/paste extraction + chunking + embedding
4. `backend-retrieval` — Hybrid search + context assembly + citation parsing
5. `frontend-core` — App shell, stores, IPC wrappers, event listeners
6. `frontend-panels` — All panels (sources, chat, notes, settings basics)

### Phase 2 (Rich Ingestion + Studio)
1. `backend-ingestion` — Add PDF, DOCX, XLSX, CSV, URL, YouTube extractors
2. `backend-providers` — Add OpenAI + Anthropic implementations
3. `backend-retrieval` — Add reranker, multi-angle query rewriting
4. `backend-studio` — Template system + 6 report-type generators
5. `frontend-studio` — StudioPanel + MarkdownViewer + TimelineView + DataTableView
6. `frontend-panels` — Update settings for new providers, update source upload for new types

### Phase 3 (Interactive Outputs + Media)
1. `backend-studio` — Flashcard, quiz, mind map templates + JSON validation
2. `backend-ingestion` — Audio (whisper-rs) + video (ffmpeg) + image extractors
3. `frontend-studio` — FlashcardWidget + QuizWidget + MindMapGraph
4. `frontend-panels` — Audio/video upload UX, transcription progress, export/import

### Phase 4 (Audio Overviews + Visual Outputs)
1. `backend-studio` — Audio pipeline (script → Piper TTS → hound assembly)
2. `backend-studio` — Slide deck + infographic JSON generation
3. `frontend-studio` — AudioPlayer + SlideViewer + InfographicView + export buttons

## Integration Contracts

Agents communicate through well-defined interfaces. When building in parallel, agree on these types first:

```typescript
// Shared types that BOTH backend and frontend must agree on
// Define in src/lib/types.ts AND mirror in src-tauri/src/commands/*.rs

interface Notebook { id: string; name: string; description?: string; source_count: number; last_accessed?: string; created_at: string; }
interface Source { id: string; source_type: string; title: string; file_hash?: string; url?: string; word_count?: number; summary?: string; status: string; error_message?: string; selected: boolean; created_at: string; }
interface Conversation { id: string; title?: string; style: string; custom_goal?: string; created_at: string; updated_at: string; }
interface Message { id: string; conversation_id: string; role: 'user' | 'assistant'; content: string; citations?: Citation[]; model_used?: string; created_at: string; }
interface Citation { chunk_id: string; source_id: string; source_title: string; quote?: string; page?: number; section?: string; }
interface Note { id: string; title?: string; content: string; note_type: 'manual' | 'saved_response'; citations?: Citation[]; pinned: boolean; created_at: string; updated_at: string; }
interface StudioOutput { id: string; output_type: string; title?: string; prompt_used: string; raw_content?: string; config?: Record<string, any>; source_ids: string[]; file_path?: string; status: string; error_message?: string; created_at: string; }
interface ModelInfo { id: string; provider: string; display_name: string; parameter_size?: string; context_window?: number; }
interface Provider { id: string; enabled: boolean; base_url?: string; has_api_key: boolean; }
```

## Conflict Resolution

If two agents need the same resource:
1. **Database access:** Only `backend-db` writes migration/schema code. Other agents call functions from `db/` module.
2. **LLM calls:** Only through the provider abstraction in `providers/`. Never raw HTTP.
3. **fastembed instances:** Created once in `AppState`, shared by reference. `ingestion/embed.rs` and `retrieval/reranker.rs` both use the same instances.
4. **Tauri events:** Only the Rust command handlers emit events. Frontend only listens. Store updates happen in store actions triggered by event listeners.
