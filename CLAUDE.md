# CLAUDE.md — Gloss Project Instructions

> **Read SPEC-gloss.md before writing any code.** It is the single source of truth for architecture, schemas, APIs, and phases. If this file and the spec conflict, the spec wins.

## What Is Gloss

Gloss is a local-first NotebookLM alternative. Tauri 2.x desktop app. Rust backend + React 19 frontend. Users add documents to notebooks, Gloss builds a queryable knowledge base (embeddings + FTS5 + hybrid search), and users chat with grounded Q&A (RAG with citations) and generate structured outputs (reports, flashcards, quizzes, mind maps, audio overviews).

## Project Structure

```
gloss/
├── CLAUDE.md                    # You are here
├── AGENTS.md                    # Sub-agent task delegation rules
├── SPEC-gloss.md                # Full specification (READ THIS FIRST)
├── src-tauri/                   # Rust backend (Tauri 2.x)
│   ├── Cargo.toml
│   ├── tauri.conf.json
│   ├── capabilities/
│   ├── src/
│   │   ├── main.rs              # Tauri app entry point
│   │   ├── lib.rs               # Tauri command registration + setup
│   │   ├── commands/            # Tauri IPC command handlers (thin wrappers)
│   │   │   ├── mod.rs
│   │   │   ├── notebooks.rs
│   │   │   ├── sources.rs
│   │   │   ├── chat.rs
│   │   │   ├── notes.rs
│   │   │   ├── studio.rs
│   │   │   └── settings.rs
│   │   ├── db/                  # Database layer
│   │   │   ├── mod.rs
│   │   │   ├── app_db.rs        # gloss.db (notebook registry, providers, settings)
│   │   │   ├── notebook_db.rs   # Per-notebook DB (sources, chunks, conversations, etc.)
│   │   │   └── migrations.rs    # Schema creation + migration logic
│   │   ├── ingestion/           # Source processing pipeline
│   │   │   ├── mod.rs
│   │   │   ├── extract.rs       # Text extraction dispatch by source_type
│   │   │   ├── chunk.rs         # Recursive character splitting with format awareness
│   │   │   ├── embed.rs         # fastembed-rs embedding generation
│   │   │   └── summarize.rs     # LLM-based source summarization
│   │   ├── retrieval/           # RAG search pipeline
│   │   │   ├── mod.rs
│   │   │   ├── hybrid_search.rs # HNSW + FTS5 + RRF fusion
│   │   │   ├── reranker.rs      # fastembed BGERerankerBase
│   │   │   ├── context.rs       # Context budget assembly
│   │   │   └── citations.rs     # Citation extraction + mapping
│   │   ├── providers/           # LLM provider abstraction
│   │   │   ├── mod.rs           # Provider trait + registry
│   │   │   ├── ollama.rs        # Ollama HTTP API
│   │   │   ├── openai.rs        # OpenAI chat completions API
│   │   │   └── anthropic.rs     # Anthropic messages API
│   │   ├── studio/              # Studio output generation
│   │   │   ├── mod.rs           # Universal pipeline: template → prompt → LLM → parse → store
│   │   │   ├── templates.rs     # TOML template loader
│   │   │   └── audio.rs         # Audio overview pipeline (script → TTS → assembly)
│   │   ├── jobs/                # Job definitions for tauri-queue
│   │   │   ├── mod.rs
│   │   │   └── handlers.rs      # GlossJob enum + JobHandler implementation
│   │   ├── state.rs             # AppState: DB handles, provider registry, queue, fastembed
│   │   └── error.rs             # GlossError enum with thiserror
│   └── prompts/
│       └── studio_templates.toml  # Studio output prompt templates (data, not code)
├── src/                         # React frontend
│   ├── main.tsx                 # React entry point
│   ├── App.tsx                  # Root layout: sidebar + notebook view
│   ├── stores/                  # Zustand stores
│   │   ├── notebookStore.ts
│   │   ├── sourceStore.ts
│   │   ├── chatStore.ts
│   │   ├── studioStore.ts
│   │   ├── noteStore.ts
│   │   └── settingsStore.ts
│   ├── components/              # React components
│   │   ├── layout/              # App shell, panels, status bar
│   │   ├── notebooks/           # Sidebar, notebook cards
│   │   ├── sources/             # Upload zone, source list, source viewer
│   │   ├── chat/                # Message list, input, citation badges
│   │   ├── studio/              # Output grid, renderers
│   │   │   ├── StudioPanel.tsx
│   │   │   ├── StudioGrid.tsx
│   │   │   ├── renderers/       # One component per renderer type
│   │   │   │   ├── MarkdownViewer.tsx
│   │   │   │   ├── FlashcardWidget.tsx
│   │   │   │   ├── QuizWidget.tsx
│   │   │   │   ├── MindMapGraph.tsx
│   │   │   │   ├── TimelineView.tsx
│   │   │   │   ├── DataTableView.tsx
│   │   │   │   ├── SlideViewer.tsx
│   │   │   │   ├── InfographicView.tsx
│   │   │   │   └── AudioPlayer.tsx
│   │   │   └── GenerateDialog.tsx
│   │   ├── notes/               # Note list, editor
│   │   └── settings/            # Provider config, model picker, tools status
│   ├── hooks/                   # Custom React hooks (useTauriEvent, useStreamingChat, etc.)
│   ├── lib/                     # Tauri IPC wrappers, types, utils
│   │   ├── tauri.ts             # invoke() wrappers matching Rust commands
│   │   ├── types.ts             # TypeScript types mirroring Rust structs
│   │   └── events.ts            # Tauri event listeners
│   └── styles/
│       └── globals.css          # Tailwind imports + custom properties
├── package.json
├── tsconfig.json
├── vite.config.ts
├── tailwind.config.ts
└── index.html
```

## Critical Rules

### 1. NEVER Invent APIs for Local Libraries

Gloss depends on three local crates at `~/Coding/Libraries/`:

- **llm-pipeline** — LLM HTTP calls, chains, structured output parsing, streaming. Provides `LlmClient`, `Chain`, `LlmCall`, `parse_as::<T>()`, `EventHandler` for streaming callbacks.
- **agent-graph** — Multi-step pipelines with conditional routing, checkpoints, fan-out/fan-in, interrupts. Provides `AgentGraph`, nodes, edges, `CheckpointStore`.
- **tauri-queue** — Job scheduling with Tauri event bridge, priority, cooldown, persistence (SQLite), cancellation. Provides `QueueManager`, `JobHandler` trait, progress events.

**Before using any of these libraries:**
1. Read the README.md and lib.rs of the crate at `~/Coding/Libraries/{crate-name}/`
2. Check the actual public API with `cargo doc --open` or by reading `src/lib.rs`
3. If an API you need doesn't exist, implement the feature in Gloss — do NOT hallucinate methods on these crates

If you cannot read the library source (e.g., it's not accessible), **write a clean trait-based abstraction in Gloss** that can be wired up to the real library later. Mark it with `// TODO: Wire to llm-pipeline` comments.

### 2. Build Phase by Phase

The spec defines 4 phases. **Do not skip ahead.** Each phase has:
- Explicit backend and frontend task lists
- A verification test at the end
- Dependencies on prior phases

Build Phase 1 completely before starting Phase 2. Every phase must compile and run before proceeding.

### 3. Error Handling — No unwrap(), No panic!()

Every fallible operation uses `Result<T, GlossError>`. The error type hierarchy:

```rust
#[derive(Debug, thiserror::Error)]
pub enum GlossError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Provider error: {source}")]
    Provider { provider: String, #[source] source: anyhow::Error },
    #[error("Ingestion error: {message}")]
    Ingestion { source_id: String, message: String },
    #[error("Embedding error: {0}")]
    Embedding(String),
    #[error("Search error: {0}")]
    Search(String),
    #[error("Studio error: {message}")]
    Studio { output_type: String, message: String },
    #[error("JSON parse error: {0}")]
    JsonParse(#[from] serde_json::Error),
    #[error("Configuration error: {0}")]
    Config(String),
    #[error("External tool not found: {tool}")]
    ExternalToolMissing { tool: String },
    #[error("{0}")]
    Other(String),
}

// For Tauri IPC serialization
impl serde::Serialize for GlossError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where S: serde::Serializer {
        serializer.serialize_str(&self.to_string())
    }
}
```

**Every Tauri command returns `Result<T, GlossError>`.** Tauri serializes the error to the frontend. The frontend displays it in the status bar or a toast — never silently swallowed.

### 4. Database Discipline

- **Two SQLite databases:** `gloss.db` (app-level) and `notebook.db` (per-notebook). See SPEC §3.2 and §3.3 for exact schemas.
- **Use parameterized queries.** No string interpolation for SQL. Ever.
- **WAL mode** on both databases for concurrent reads during writes:
  ```rust
  conn.execute_batch("PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;")?;
  ```
- **Migrations:** Create all tables in a `migrations.rs` that runs on first open. Include a `schema_version` in a `_meta` table. Check version before operations.
- **FTS5 sync triggers:** When inserting/deleting chunks, the FTS5 index must be kept in sync:
  ```sql
  CREATE TRIGGER chunks_ai AFTER INSERT ON chunks BEGIN
      INSERT INTO chunks_fts(rowid, content) VALUES (new.rowid, new.content);
  END;
  CREATE TRIGGER chunks_ad AFTER DELETE ON chunks BEGIN
      INSERT INTO chunks_fts(chunks_fts, rowid, content) VALUES ('delete', old.rowid, old.content);
  END;
  ```

### 5. Frontend Rules

- **TypeScript strict mode.** No `any` types except in Tauri IPC boundaries (then immediately parse into typed interfaces).
- **Zustand stores** are the single source of truth. Components read from stores, never from local state for shared data.
- **Tauri IPC calls** go through typed wrapper functions in `src/lib/tauri.ts`, not raw `invoke()` calls scattered in components.
- **Tailwind only.** No CSS modules, no styled-components, no inline styles except for truly dynamic values (e.g., panel widths from drag).
- **shadcn/ui primitives** for buttons, dialogs, inputs, dropdowns, tooltips, etc. Do not build custom versions of these.
- **Lazy load** heavy renderers (MindMapGraph with d3, SlideViewer, InfographicView). Use `React.lazy()` + `Suspense`.

### 6. Structured Output from LLMs

Every studio output that expects JSON from the LLM **will sometimes receive garbage.** Handle this:

1. Try `serde_json::from_str(raw)`
2. If that fails, try extracting JSON from markdown code fences (` ```json ... ``` `)
3. If that fails, retry the LLM call once with a stricter prompt ("Return ONLY valid JSON")
4. If still fails, store the raw output with `status = 'error'`, surface error to user with "Try again" button

**Never trust LLM output shape.** Validate every field after parsing. Use `serde(default)` on optional fields. Log malformed responses at `warn` level for debugging.

### 7. Streaming Chat Implementation

Chat responses must stream token-by-token:

**Rust side:**
```rust
// In the chat command handler
let message_id = uuid::Uuid::new_v4().to_string();
app_handle.emit("chat:token", json!({
    "conversation_id": conversation_id,
    "message_id": message_id,
    "token": token_text,
    "done": false,
}))?;
```

**React side:**
```typescript
// In useTauriEvent hook
listen('chat:token', (event) => {
    const { conversation_id, message_id, token, done } = event.payload;
    chatStore.appendToken(conversation_id, message_id, token);
    if (done) chatStore.finalizeMessage(message_id);
});
```

The `send_message` Tauri command returns immediately with the `message_id`. Tokens arrive via events. The final event has `done: true` and the complete message (with citations) is persisted to the database at that point.

### 8. Embedding & Search Pipeline

All on CPU. Never competes with GPU.

```
fastembed-rs (NomicEmbedTextV15, 768-dim) → usearch HNSW index
fastembed-rs (BGERerankerBase) → cross-encoder reranking
rusqlite FTS5 → BM25 keyword search
```

**Initialize once, reuse:** Create `TextEmbedding` and `TextRerank` instances in `AppState` at startup. They're thread-safe. Don't recreate per request.

**HNSW index lifecycle:**
- Created when first source is added to a notebook
- Loaded from `embeddings/chunks.usearch` on notebook open
- New vectors added during ingestion
- Serialized to disk after each ingestion batch completes
- Rebuilt entirely on `ReindexNotebook` (embedding model change)

### 9. Provider Abstraction

All three providers (Ollama, OpenAI, Anthropic) implement the same `LlmProvider` trait (SPEC §4.1). The chat command handler never knows which provider it's talking to.

**Key differences handled inside providers:**
- Ollama: `POST /api/chat`, streaming via newline-delimited JSON
- OpenAI: `POST /v1/chat/completions`, streaming via SSE (`data: {...}`)
- Anthropic: `POST /v1/messages`, `system` is a top-level field (not a message role), streaming via SSE with `content_block_delta` events

**Model selection:** The frontend sends the model ID (e.g., `"qwen3:8b"` or `"gpt-4o"`). The backend looks up which provider owns that model in the registry and routes accordingly.

### 10. What NOT to Build

Do NOT implement these (they are explicitly deferred in the spec):
- Video Overview generation (slide rendering + TTS + ffmpeg → MP4)
- Audio Interactive Mode (real-time voice join)
- AI-generated images in infographics/slides
- Deep Research (agentic multi-step web browsing)
- Fast Research / Source Discovery (web search integration)
- Code repository ingestion
- EPUB / PPTX ingestion (beyond what kreuzberg handles)
- Google Drive integration, public notebooks, mobile app, usage tiers

If a user asks about these, they should see a "Coming soon" disabled button, not an error.

## Build Commands

```bash
# Backend
cd src-tauri && cargo build          # Debug build
cd src-tauri && cargo clippy         # Lint — must pass with zero warnings
cd src-tauri && cargo test           # Run tests

# Frontend
npm install                          # Install deps
npm run dev                          # Vite dev server (Tauri picks this up)
npm run build                        # Production build
npm run lint                         # ESLint

# Full app
cargo tauri dev                      # Run app in dev mode (hot reload frontend)
cargo tauri build                    # Production build
```

## Testing Strategy

- **Rust unit tests:** Every module in `db/`, `ingestion/`, `retrieval/`, `providers/`, `studio/` gets unit tests. Use `tempfile` crate for temp directories.
- **Integration tests:** `tests/` directory with tests that create a real notebook, add sources, run retrieval, generate outputs.
- **Frontend:** Components don't need unit tests in v1. Manual testing via `cargo tauri dev` is acceptable. Zustand stores can be tested if complex logic exists.

## Logging

Use `tracing` throughout the backend:

```rust
use tracing::{info, warn, error, debug, instrument};

#[instrument(skip(db, provider))]
pub async fn send_message(...) -> Result<String, GlossError> {
    info!(notebook_id, conversation_id, "Processing chat message");
    // ...
    debug!(chunk_count = chunks.len(), "Retrieved relevant chunks");
}
```

Set up the subscriber in `main.rs`:
```rust
tracing_subscriber::fmt()
    .with_env_filter("gloss=debug,tauri_queue=info")
    .init();
```

## Style Guide

### Rust
- `rustfmt` default config. No custom formatting rules.
- All public functions documented with `///` doc comments.
- Group imports: std → external crates → local modules.
- Prefer `thiserror` for library errors, `anyhow` only in top-level command handlers.

### TypeScript/React
- Functional components only. No class components.
- Named exports (not default exports) for all components.
- Props interfaces defined inline or in the same file (not a separate types file, unless shared).
- Event handlers: `handleVerb` naming (e.g., `handleSend`, `handleDelete`).

### Commit Messages
- `feat(scope): description` for new features
- `fix(scope): description` for bug fixes
- `refactor(scope): description` for structural changes
- Scopes: `db`, `ingest`, `search`, `chat`, `studio`, `providers`, `ui`, `config`
