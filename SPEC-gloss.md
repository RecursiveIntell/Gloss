# Gloss — Full Specification

**Version:** 1.0.0
**Date:** 2025-02-14
**Status:** Implementation-ready
**Supersedes:** DESIGN-local-notebooklm.md, RESEARCH-gloss-addendum.md, PARITY-gloss-vs-notebooklm.md

---

## 1. Product Definition

Gloss is a local-first, privacy-preserving alternative to Google's NotebookLM. Users add documents to notebooks, Gloss builds a queryable knowledge base, and users can have grounded conversations, generate structured outputs (reports, flashcards, quizzes, mind maps, timelines), and produce audio overviews — all using local LLM inference via Ollama.

**Name etymology:** *gloss (n.)* — a brief explanatory note or translation of a difficult word or passage. The verb "to gloss" describes exactly what the app does.

### 1.1 Hardware Target

| Resource | Spec | Design Implication |
|----------|------|-------------------|
| GPU | GTX 1070, 8GB VRAM | One 7-8B model loaded at a time. No concurrent GPU inference. |
| Inference server | Ollama on dedicated machine, Tailscale-connected | Network latency is negligible on LAN. Ollama handles model loading/unloading. |
| Embedding | fastembed-rs on CPU | Decoupled from GPU. Runs concurrently with LLM inference. |
| TTS | Piper on CPU | No VRAM pressure. ~1-3x realtime generation speed. |
| STT | whisper-rs on CPU (or GPU when Ollama unloaded) | Video/audio transcription is a background job; latency acceptable. |

**Critical constraint:** Never assume two GPU models can be loaded simultaneously. All pipelines are sequential on GPU. CPU-bound work (embedding, TTS, search) runs in parallel.

### 1.2 Tech Stack

| Layer | Choice | Rationale |
|-------|--------|-----------|
| App shell | Tauri 2.x | Native perf, small binary, Rust backend, web frontend |
| Backend | Rust | Type safety, performance, your existing library ecosystem |
| Frontend | React 19 + TypeScript + Vite | Ecosystem depth for widget-heavy desktop app (PDF viewer, graph viz, slide renderer, audio player) |
| Styling | TailwindCSS 4 | Utility-first, fast iteration, dark mode built-in |
| Components | shadcn/ui | Accessible primitives, fully customizable, no vendor lock |
| State | Zustand | Minimal boilerplate, Tauri IPC–friendly |
| Database | rusqlite (bundled, FTS5) | Per-notebook portability. No external DB process. |
| Vector index | usearch 2.x | Production-grade HNSW. Serializable for notebook portability. |
| Embeddings | fastembed 5.x (NomicEmbedTextV15) | 768-dim, CPU, no VRAM competition, built-in reranker |
| LLM orchestration | llm-pipeline + agent-graph + tauri-queue | Your existing libraries. Chain jobs, stream tokens, background processing. |
| LLM providers | Ollama (primary), OpenAI (optional), Anthropic (optional) | Local-first with cloud escape hatch |

### 1.3 Scope Boundaries

**In scope for v1:**
- Everything classified ✅ DIRECT or 🔄 ADAPTED in the parity analysis
- Phased delivery across 4 phases (~8 weeks)

**Explicitly deferred (add later, not now):**
- Audio Interactive Mode (real-time voice join) — VRAM orchestration too complex for v1
- AI-generated images in video overviews / infographics — no local image model at 8GB VRAM
- Deep Research (agentic multi-step web browsing) — 8B models unreliable for multi-step planning
- Video Overview — combines slide rendering + TTS + ffmpeg; too many moving parts for v1
- Fast Research / Source Discovery — requires web search backend integration
- Code repository ingestion — tree walking + language-aware chunking is a separate problem
- EPUB / PPTX ingestion — lower priority formats, add when kreuzberg is integrated

**Out of scope permanently (wrong product):**
- Google Drive integration, public/shared notebooks, mobile app, usage tiers

---

## 2. Architecture

### 2.1 System Diagram

```
┌──────────────────────────────────────────────────────────────────────┐
│                        Tauri 2.x Shell                               │
│                                                                      │
│  ┌───────────┐  ┌──────────────────────────────────────┐             │
│  │  Notebook  │  │          Active Notebook View         │             │
│  │  Sidebar   │  │  ┌──────────┐ ┌──────────┐ ┌──────┐ │             │
│  │            │  │  │ Sources  │ │   Chat   │ │Studio│ │             │
│  │  • list    │  │  │  Panel   │ │  Panel   │ │Panel │ │             │
│  │  • create  │  │  └──────────┘ └──────────┘ └──────┘ │             │
│  │  • search  │  │                                      │             │
│  └───────────┘  │  ┌────────────────────────────────┐   │             │
│                  │  │      Source Detail Viewer       │   │             │
│                  │  └────────────────────────────────┘   │             │
│                  └──────────────────────────────────────┘             │
│                                                                      │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │  Status Bar: jobs · model · notebook stats · provider status   │  │
│  └────────────────────────────────────────────────────────────────┘  │
└─────────────────────────────┬────────────────────────────────────────┘
                              │ Tauri IPC (invoke / events)
┌─────────────────────────────┴────────────────────────────────────────┐
│                         Rust Backend                                  │
│                                                                      │
│  ┌──────────────┐  ┌──────────────┐  ┌────────────────────────────┐ │
│  │  Notebook     │  │  Ingestion   │  │  Conversation Engine       │ │
│  │  Manager      │  │  Pipeline    │  │  (RAG)                     │ │
│  │               │  │              │  │                            │ │
│  │  CRUD,        │  │  Extract →   │  │  Rewrite query →           │ │
│  │  export/      │  │  Chunk →     │  │  Hybrid search →           │ │
│  │  import       │  │  Embed →     │  │  Rerank (fastembed) →      │ │
│  │               │  │  Summarize   │  │  Assemble context →        │ │
│  │               │  │              │  │  Generate (stream) →       │ │
│  │               │  │              │  │  Extract citations          │ │
│  └───────┬──────┘  └──────┬───────┘  └─────────────┬──────────────┘ │
│          │                │                         │                │
│  ┌───────┴────────────────┴─────────────────────────┴──────────────┐ │
│  │                    Shared Services                               │ │
│  │                                                                  │ │
│  │  ┌─────────────┐ ┌─────────────┐ ┌─────────────┐ ┌───────────┐ │ │
│  │  │ tauri-queue  │ │llm-pipeline │ │ agent-graph  │ │ Provider  │ │ │
│  │  │ (jobs,       │ │(LLM calls,  │ │(multi-step   │ │ Registry  │ │ │
│  │  │  progress,   │ │ structured  │ │ pipelines,   │ │(Ollama,   │ │ │
│  │  │  priority)   │ │ output,     │ │ checkpoints) │ │ OpenAI,   │ │ │
│  │  │              │ │ streaming)  │ │              │ │ Anthropic)│ │ │
│  │  └──────────────┘ └─────────────┘ └──────────────┘ └───────────┘ │ │
│  │                                                                  │ │
│  │  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐             │ │
│  │  │ rusqlite     │ │ usearch      │ │ fastembed    │             │ │
│  │  │ (FTS5)       │ │ (HNSW)       │ │ (embed +    │             │ │
│  │  │              │ │              │ │  rerank)     │             │ │
│  │  └──────────────┘ └──────────────┘ └──────────────┘             │ │
│  └──────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
          │                │               │
          ▼                ▼               ▼
    ┌──────────┐    ┌──────────┐    ┌──────────────┐
    │ SQLite   │    │ HNSW     │    │ Ollama       │
    │ (disk)   │    │ (disk)   │    │ (inference   │
    │          │    │          │    │  server, LAN)│
    └──────────┘    └──────────┘    └──────────────┘
```

### 2.2 Library Composition

```
gloss (this app)
├── tauri-queue           — Job scheduling with Tauri event bridge
├── llm-pipeline          — LLM HTTP calls, structured output, streaming
├── agent-graph           — Multi-step analysis/generation pipelines
├── fastembed 5.x         — Embeddings (NomicEmbedTextV15) + reranking (BGERerankerBase)
├── usearch 2.x           — HNSW vector similarity index
├── rusqlite              — Metadata + chunks + FTS5 full-text search
├── lopdf                 — PDF text extraction
├── calamine              — Excel/ODS/CSV reading
├── scraper + reqwest     — Web page fetch + HTML parsing
├── readability           — Article extraction from messy HTML
├── yt-transcript-rs      — YouTube transcript extraction (pure Rust)
├── whisper-rs            — Local audio/video transcription (STT)
├── piper-rs              — Text-to-speech (CPU, ONNX runtime)
├── hound                 — WAV file I/O for audio assembly
└── tauri 2.x             — Application shell
```

---

## 3. Data Model

### 3.1 Filesystem Layout

```
~/.local/share/gloss/
├── gloss.db                        # App-level DB (notebook registry, settings, provider config)
├── notebooks/
│   └── {uuid}/
│       ├── notebook.db             # Per-notebook DB (sources, chunks, conversations, notes, studio outputs)
│       ├── sources/                # Original source files (copied on import)
│       │   ├── {hash}.pdf
│       │   ├── {hash}.txt
│       │   └── {hash}.html
│       ├── embeddings/
│       │   └── chunks.usearch      # Serialized HNSW index
│       ├── audio/                  # Generated audio overviews
│       │   └── {output_id}.wav
│       └── exports/                # Generated slide decks, infographics
│           ├── {output_id}.pdf
│           └── {output_id}.png
└── cache/
    ├── fastembed/                   # Downloaded ONNX embedding models
    ├── whisper/                     # Downloaded whisper models
    └── piper/                       # Downloaded Piper voice models
```

### 3.2 App-Level Schema (gloss.db)

```sql
-- Notebook registry
CREATE TABLE notebooks (
    id          TEXT PRIMARY KEY,
    name        TEXT NOT NULL,
    description TEXT,
    directory   TEXT NOT NULL,          -- Absolute path to notebook dir
    source_count INTEGER DEFAULT 0,
    last_accessed TEXT,
    created_at  TEXT DEFAULT (datetime('now')),
    updated_at  TEXT DEFAULT (datetime('now'))
);

-- Provider configuration
CREATE TABLE providers (
    id          TEXT PRIMARY KEY,       -- 'ollama', 'openai', 'anthropic'
    enabled     INTEGER DEFAULT 0,
    base_url    TEXT,                   -- 'http://192.168.1.x:11434' for Ollama
    api_key     TEXT,                   -- Encrypted via tauri-plugin-store; NULL for Ollama
    last_refreshed TEXT,
    created_at  TEXT DEFAULT (datetime('now'))
);

-- Cached model list (refreshed from providers)
CREATE TABLE models (
    id              TEXT NOT NULL,      -- 'qwen3:8b', 'gpt-4o', 'claude-sonnet-4-5-20250929'
    provider_id     TEXT NOT NULL REFERENCES providers(id),
    display_name    TEXT NOT NULL,
    parameter_size  TEXT,               -- '8.2B' (Ollama provides this)
    context_window  INTEGER,
    capabilities    TEXT,               -- JSON: { chat: true, vision: false, embedding: false }
    PRIMARY KEY (id, provider_id)
);

-- App settings (key-value)
CREATE TABLE settings (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Default settings:
-- 'default_provider'        -> 'ollama'
-- 'default_model'           -> 'qwen3:8b'
-- 'default_embedding_model' -> 'NomicEmbedTextV15'
-- 'ollama_url'              -> 'http://localhost:11434'
-- 'theme'                   -> 'system'
-- 'whisper_model'           -> 'base'
-- 'piper_voice_a'           -> 'en_US-amy-medium'
-- 'piper_voice_b'           -> 'en_US-ryan-medium'
```

### 3.3 Per-Notebook Schema (notebook.db)

```sql
-- Sources: original documents added to the notebook
CREATE TABLE sources (
    id                TEXT PRIMARY KEY,
    source_type       TEXT NOT NULL,    -- 'text', 'markdown', 'pdf', 'docx', 'xlsx', 'csv',
                                       -- 'url', 'youtube', 'audio', 'video', 'image', 'paste'
    title             TEXT NOT NULL,
    original_filename TEXT,
    file_hash         TEXT,             -- SHA-256 of original file (dedup)
    url               TEXT,             -- For web/youtube sources
    file_path         TEXT,             -- Relative path in sources/ dir
    content_text      TEXT,             -- Extracted full text
    word_count        INTEGER,
    metadata          TEXT,             -- JSON: { pages, author, duration_seconds, ... }
    summary           TEXT,             -- AI-generated summary
    summary_model     TEXT,
    status            TEXT DEFAULT 'pending',  -- pending | extracting | chunking | embedding | summarizing | ready | error
    error_message     TEXT,
    selected          INTEGER DEFAULT 1,       -- Include in queries by default
    created_at        TEXT DEFAULT (datetime('now')),
    updated_at        TEXT DEFAULT (datetime('now'))
);

-- Chunks: semantic segments of source documents
CREATE TABLE chunks (
    id              TEXT PRIMARY KEY,
    source_id       TEXT NOT NULL REFERENCES sources(id) ON DELETE CASCADE,
    chunk_index     INTEGER NOT NULL,   -- Order within source
    content         TEXT NOT NULL,
    token_count     INTEGER,
    start_offset    INTEGER,            -- Character offset in source content_text
    end_offset      INTEGER,
    metadata        TEXT,               -- JSON: { page, section, heading }
    embedding_id    INTEGER,            -- Row ID in HNSW index (for deletion/rebuild)
    embedding_model TEXT,
    created_at      TEXT DEFAULT (datetime('now'))
);

-- Full-text search index
CREATE VIRTUAL TABLE chunks_fts USING fts5(
    content,
    content='chunks',
    content_rowid='rowid'
);

-- Conversations
CREATE TABLE conversations (
    id          TEXT PRIMARY KEY,
    title       TEXT,                   -- Auto-generated from first message, user-editable
    style       TEXT DEFAULT 'default', -- 'default', 'learning_guide', 'custom'
    custom_goal TEXT,                   -- Free-form system prompt for 'custom' style
    created_at  TEXT DEFAULT (datetime('now')),
    updated_at  TEXT DEFAULT (datetime('now'))
);

-- Messages within conversations
CREATE TABLE messages (
    id                  TEXT PRIMARY KEY,
    conversation_id     TEXT NOT NULL REFERENCES conversations(id) ON DELETE CASCADE,
    role                TEXT NOT NULL,   -- 'user', 'assistant'
    content             TEXT NOT NULL,   -- Markdown with citation markers [1], [2], ...
    citations           TEXT,            -- JSON: [{ chunk_id, source_id, source_title, quote, page, section }]
    model_used          TEXT,
    tokens_prompt       INTEGER,
    tokens_response     INTEGER,
    created_at          TEXT DEFAULT (datetime('now'))
);

-- Notes (manual + saved AI responses)
CREATE TABLE notes (
    id          TEXT PRIMARY KEY,
    title       TEXT,
    content     TEXT NOT NULL,          -- Markdown
    note_type   TEXT NOT NULL,          -- 'manual', 'saved_response'
    citations   TEXT,                   -- JSON array (preserved from saved responses)
    pinned      INTEGER DEFAULT 0,
    source_id   TEXT,                   -- Optional: linked to a specific source
    created_at  TEXT DEFAULT (datetime('now')),
    updated_at  TEXT DEFAULT (datetime('now'))
);

-- Studio outputs (universal table for all generated content)
CREATE TABLE studio_outputs (
    id          TEXT PRIMARY KEY,
    output_type TEXT NOT NULL,          -- See §6.1 for complete list
    title       TEXT,                   -- User-editable
    prompt_used TEXT NOT NULL,          -- Exact prompt for reproducibility / "View prompt"
    raw_content TEXT,                   -- LLM's structured output (JSON or Markdown)
    config      TEXT,                   -- JSON: { difficulty, count, language, style, focus, ... }
    source_ids  TEXT,                   -- JSON array of source IDs used in generation
    file_path   TEXT,                   -- For audio: relative path to .wav
    status      TEXT DEFAULT 'pending', -- pending | generating | ready | error
    error_message TEXT,
    created_at  TEXT DEFAULT (datetime('now'))
);

-- Notebook-level config
CREATE TABLE notebook_config (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- Default config:
-- 'default_style'         -> 'default'
-- 'custom_goal'           -> ''
-- 'response_length'       -> 'default'
-- 'output_language'       -> 'en'

-- Indexes
CREATE INDEX idx_chunks_source ON chunks(source_id);
CREATE INDEX idx_messages_conversation ON messages(conversation_id, created_at);
CREATE INDEX idx_sources_status ON sources(status);
CREATE INDEX idx_sources_hash ON sources(file_hash);
CREATE INDEX idx_notes_pinned ON notes(pinned DESC, updated_at DESC);
CREATE INDEX idx_studio_type ON studio_outputs(output_type, created_at DESC);
```

---

## 4. LLM Provider Abstraction

### 4.1 Provider Trait

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    /// List available models from this provider
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;

    /// Send a chat completion request, returning a token stream
    async fn chat(&self, request: ChatRequest) -> Result<Pin<Box<dyn Stream<Item = Result<ChatToken>>>>>;

    /// Test connectivity
    async fn health_check(&self) -> Result<bool>;

    /// Provider identifier
    fn provider_type(&self) -> ProviderType;
}

pub struct ChatRequest {
    pub model: String,
    pub system_prompt: Option<String>,
    pub messages: Vec<ChatMessage>,
    pub max_tokens: u32,
    pub temperature: f32,
    pub stream: bool,
}

pub struct ModelInfo {
    pub id: String,                  // "qwen3:8b"
    pub display_name: String,        // "Qwen3 8B"
    pub provider: ProviderType,      // Ollama
    pub parameter_size: Option<String>,
    pub context_window: Option<u32>,
}
```

### 4.2 Provider Implementations

**Ollama:**
- List models: `GET {base_url}/api/tags`
- Chat: `POST {base_url}/api/chat` (native API, streaming)
- Health: `GET {base_url}/`
- Model unload: `POST {base_url}/api/generate` with `keep_alive: "0"`

**OpenAI:**
- List models: `GET https://api.openai.com/v1/models` with `Authorization: Bearer {key}`
- Chat: `POST https://api.openai.com/v1/chat/completions`

**Anthropic:**
- List models: `GET https://api.anthropic.com/v1/models` with `x-api-key: {key}` and `anthropic-version: 2023-06-01`
- Chat: `POST https://api.anthropic.com/v1/messages` (different schema: `system` is a top-level field, not a message role)

### 4.3 Model Registry

```rust
pub struct ModelRegistry {
    providers: HashMap<ProviderType, Box<dyn LlmProvider>>,
    cache: RwLock<HashMap<ProviderType, Vec<ModelInfo>>>,
    last_refreshed: RwLock<HashMap<ProviderType, Instant>>,
}

impl ModelRegistry {
    /// Refresh model lists from all enabled providers
    pub async fn refresh_all(&self) -> Result<()>;

    /// Refresh one provider
    pub async fn refresh_provider(&self, provider: ProviderType) -> Result<()>;

    /// Get cached model list
    pub fn get_models(&self, provider: ProviderType) -> Vec<ModelInfo>;

    /// Get all models across all providers
    pub fn get_all_models(&self) -> Vec<ModelInfo>;
}
```

**Refresh triggers:**
1. App startup
2. User clicks "Refresh" button in model selector
3. Provider config changes (API key added, URL changed)
4. Chat request fails with "model not found"

### 4.4 API Key Storage

API keys for OpenAI/Anthropic are stored via `tauri-plugin-store`, which uses the OS keyring (libsecret on Linux, Keychain on macOS). Keys are never written to plaintext config files. Ollama needs no key.

---

## 5. Ingestion Pipeline

### 5.1 Supported Source Types

| Type | Format(s) | Extraction Method | Phase |
|------|-----------|------------------|-------|
| Plain text | .txt | Direct read | 1 |
| Markdown | .md, .rst | Direct read | 1 |
| Pasted text | clipboard | Tauri clipboard API | 1 |
| PDF | .pdf | lopdf text extraction; vision model fallback for scans | 2 |
| Word | .docx | docx-parser (XML unzip + text) | 2 |
| Spreadsheet | .xlsx, .xls, .ods, .csv, .tsv | calamine (structured), csv crate | 2 |
| Web URL | http/https | reqwest + readability (article extraction) | 2 |
| YouTube | URL | yt-transcript-rs (InnerTube API, pure Rust) | 2 |
| Audio | .mp3, .wav, .ogg, .flac, .m4a | whisper-rs transcription (CPU or GPU) | 3 |
| Video | .mp4, .mkv, .webm, .avi, .mov | ffmpeg audio extraction → whisper-rs | 3 |
| Image | .png, .jpg, .webp | Vision model (qwen3-vl:8b) or OCR via kreuzberg | 3 |

### 5.2 Pipeline Flow

```
Source added (file drop / URL / paste)
    │
    ▼
┌─────────────────────────────────────────────────────────────┐
│  1. EXTRACT                                                  │
│  Dispatch by source_type:                                    │
│    text/md/paste  → direct read                              │
│    pdf            → lopdf → if garbled, queue vision model   │
│    docx           → docx-parser XML extraction               │
│    xlsx/csv       → calamine → rows to structured text       │
│    url            → reqwest + readability → article text      │
│    youtube        → yt-transcript-rs → timestamped transcript │
│    audio          → whisper-rs → timestamped transcript       │
│    video          → ffmpeg extract audio → whisper-rs         │
│    image          → vision model description / OCR            │
│                                                              │
│  Result: content_text stored in sources table                │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  2. CHUNK                                                    │
│  Recursive character split respecting format boundaries:     │
│    Target: 800 tokens, Max: 1500, Overlap: 100, Min: 50     │
│    Priority: SectionHeading > ParagraphBreak > LineBreak     │
│              > SentenceEnd > WordBoundary                    │
│  Attach metadata: page number, section heading, chunk_index  │
│                                                              │
│  Result: chunks stored in chunks table + FTS5 index          │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  3. EMBED                                                    │
│  fastembed-rs (NomicEmbedTextV15, 768-dim, CPU)              │
│  Batch: all chunks for this source                           │
│  Store vectors in HNSW index (usearch)                       │
│  Record embedding_id on each chunk row                       │
└────────────────────────┬────────────────────────────────────┘
                         │
                         ▼
┌─────────────────────────────────────────────────────────────┐
│  4. SUMMARIZE                                                │
│  Small source (< 3000 tokens): single-pass LLM summary      │
│  Large source: map-reduce via agent-graph                    │
│    → Summarize each chunk group → Synthesize into one summary│
│  Store in sources.summary                                    │
│  Generate 3 suggested questions, cache in notebook_config    │
└─────────────────────────────────────────────────────────────┘
```

### 5.3 Job Types

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum GlossJob {
    // === Ingestion ===
    IngestSource { notebook_id: String, source_id: String },   // Chains all steps below
    ExtractSource { notebook_id: String, source_id: String },
    ChunkSource { notebook_id: String, source_id: String },
    EmbedChunks { notebook_id: String, source_id: String },
    SummarizeSource { notebook_id: String, source_id: String },

    // === Transcription (media) ===
    TranscribeAudio { notebook_id: String, source_id: String },
    TranscribeVideo { notebook_id: String, source_id: String },  // Extract audio first, then transcribe
    FetchYouTubeTranscript { notebook_id: String, source_id: String, url: String },

    // === Studio outputs ===
    GenerateStudioOutput { notebook_id: String, output_id: String },
    GenerateAudioOverview { notebook_id: String, output_id: String },

    // === Maintenance ===
    ReindexNotebook { notebook_id: String },  // Re-embed all chunks (model change)
    RefreshModels { provider: Option<ProviderType> },
}
```

### 5.4 Video/Audio Transcription Pipeline

**External dependency:** `ffmpeg` must be installed for video ingestion. Audio-only files (mp3, wav, etc.) can be handled by whisper-rs directly via rodio format conversion.

**VRAM coordination for GPU-accelerated whisper:**
1. Signal Ollama to unload: `POST /api/generate { "model": "...", "keep_alive": "0" }`
2. Load whisper model, run transcription
3. Unload whisper (drop the model handle)
4. Ollama reloads on next request automatically

**CPU fallback:** If VRAM coordination is too fragile, default to whisper on CPU. Slower (~2-3x realtime for whisper-base) but zero VRAM conflict. The user chooses in settings.

**YouTube handling (two-tier):**
1. `yt-transcript-rs` fetches transcript (captions) — fast, no download, pure Rust
2. If no transcript available → notify user, offer to download audio via `yt-dlp` (optional external dep) → whisper transcription

---

## 6. Studio System

### 6.1 Output Types

Every studio output follows one universal pipeline: **build prompt → LLM generates structured output → renderer displays it → store in `studio_outputs` table.**

| Output Type | ID | LLM Output Format | Renderer | Phase |
|-------------|----|--------------------|----------|-------|
| Briefing Doc | `briefing_doc` | Markdown | MarkdownViewer | 2 |
| Study Guide | `study_guide` | Markdown | MarkdownViewer | 2 |
| FAQ | `faq` | JSON array | MarkdownViewer (rendered from JSON) | 2 |
| Timeline | `timeline` | JSON array | TimelineView | 2 |
| Custom Report | `custom_report` | Markdown | MarkdownViewer | 2 |
| Data Table | `data_table` | JSON array of rows | DataTableView (sortable, CSV export) | 2 |
| Flashcards | `flashcards` | JSON array | FlashcardWidget | 3 |
| Quiz | `quiz` | JSON array | QuizWidget | 3 |
| Mind Map | `mind_map` | JSON (nodes + edges) | MindMapGraph (d3-force) | 3 |
| Audio — Deep Dive | `audio_deep_dive` | Script JSON → TTS → WAV | AudioPlayer | 4 |
| Audio — Brief | `audio_brief` | Script JSON → TTS → WAV | AudioPlayer | 4 |
| Audio — Critique | `audio_critique` | Script JSON → TTS → WAV | AudioPlayer | 4 |
| Audio — Debate | `audio_debate` | Script JSON → TTS → WAV | AudioPlayer | 4 |
| Slide Deck | `slide_deck` | JSON (slides array) | SlideViewer + PDF export | 4 |
| Infographic | `infographic` | JSON (sections + stats) | InfographicView + PNG export | 4 |

### 6.2 Prompt Template System

Templates are defined in TOML (or JSON) config, not hardcoded in Rust. Users can customize or create new output types by editing the config.

```toml
# prompts/studio_templates.toml

[briefing_doc]
output_type = "briefing_doc"
display_name = "Briefing Doc"
description = "Executive summary of key findings"
output_format = "markdown"       # "markdown" | "json" | "json_array"
renderer = "markdown_viewer"
system_prompt = """You are a professional analyst creating an executive briefing document.
Include: Executive Summary, Key Findings (with supporting evidence), Main Themes,
Open Questions, and Recommendations.
Cite sources using [Source: title, section] format for every factual claim."""

user_prompt_template = """Create a briefing document from the following source material.

{source_summaries}

{options}"""

[faq]
output_type = "faq"
display_name = "FAQ"
description = "Frequently asked questions from sources"
output_format = "json_array"
renderer = "markdown_viewer"
json_schema = """[{
  "question": "string",
  "answer": "string",
  "citations": [{ "source_title": "string", "section": "string", "quote": "string" }]
}]"""
system_prompt = """You are creating a comprehensive FAQ document.
Generate questions that a reader would naturally ask about this material.
Each answer must cite the specific source and section.
Return ONLY a JSON array, no other text."""

user_prompt_template = """Generate {count} FAQ entries from this material.

{source_summaries}

Return as a JSON array matching this schema:
{json_schema}"""

[flashcards]
output_type = "flashcards"
display_name = "Flashcards"
description = "Study cards for key concepts"
output_format = "json_array"
renderer = "flashcard_widget"
json_schema = """[{
  "front": "string (term, question, or concept)",
  "back": "string (definition, answer, or explanation)",
  "citation": { "source_title": "string", "section": "string" },
  "difficulty": "easy | medium | hard"
}]"""
system_prompt = """You are creating study flashcards from source material.
Each card should test understanding of a key concept, term, or fact.
Front: a clear question or term.
Back: a concise, accurate answer with citation.
Return ONLY a JSON array, no other text."""

user_prompt_template = """Generate {count} flashcards at {difficulty} difficulty level.

{focus}

Source material:
{source_summaries}

Return as a JSON array matching this schema:
{json_schema}"""

[quiz]
output_type = "quiz"
display_name = "Quiz"
description = "Multiple-choice knowledge check"
output_format = "json_array"
renderer = "quiz_widget"
json_schema = """[{
  "question": "string",
  "options": ["string", "string", "string", "string"],
  "correct_index": 0,
  "explanation": "string (why the correct answer is right)",
  "citation": { "source_title": "string", "section": "string" }
}]"""

[mind_map]
output_type = "mind_map"
display_name = "Mind Map"
description = "Visual concept map of relationships"
output_format = "json"
renderer = "mind_map_graph"
json_schema = """{
  "nodes": [{ "id": "string", "label": "string", "summary": "string", "source_ids": ["string"] }],
  "edges": [{ "from": "string", "to": "string", "label": "string" }]
}"""

[timeline]
output_type = "timeline"
display_name = "Timeline"
description = "Chronological events from sources"
output_format = "json_array"
renderer = "timeline_view"
json_schema = """[{
  "date": "string",
  "event": "string",
  "detail": "string",
  "citation": { "source_title": "string", "section": "string" }
}]"""

[data_table]
output_type = "data_table"
display_name = "Data Table"
description = "Structured comparison table"
output_format = "json"
renderer = "data_table_view"
json_schema = """{
  "columns": ["string"],
  "rows": [{ "values": ["string"] }]
}"""

[slide_deck]
output_type = "slide_deck"
display_name = "Slide Deck"
description = "Presentation slides from sources"
output_format = "json"
renderer = "slide_viewer"
json_schema = """{
  "slides": [{
    "type": "title | content | quote | comparison",
    "title": "string",
    "subtitle": "string (optional)",
    "bullets": ["string (optional)"],
    "quote": "string (optional)",
    "attribution": "string (optional)",
    "speaker_notes": "string (optional)"
  }]
}"""

[infographic]
output_type = "infographic"
display_name = "Infographic"
description = "Visual summary with key stats"
output_format = "json"
renderer = "infographic_view"
json_schema = """{
  "title": "string",
  "subtitle": "string",
  "sections": [{
    "heading": "string",
    "type": "stat | list | process | comparison",
    "stat_value": "string (optional, e.g. '85%')",
    "items": ["string (optional)"],
    "detail": "string"
  }],
  "key_takeaway": "string"
}"""
```

**Audio overview templates** follow the same pattern but additionally specify the TTS pipeline (voices, format, target duration). Their `renderer` is `audio_pipeline` which chains script generation → per-segment TTS → audio concatenation.

### 6.3 Generation Options (User-Configurable Per Output)

| Option | Applies To | Values | Default |
|--------|-----------|--------|---------|
| `count` | Flashcards, Quiz, FAQ | 5, 10, 15, 20 | 10 |
| `difficulty` | Flashcards, Quiz | easy, medium, hard | medium |
| `focus` | All | Free text: "Focus on security implications" | (empty) |
| `language` | All | Language code: en, es, fr, de, zh, ja, ... | en |
| `style` | Slide Deck, Infographic | clean, dark, academic, bold | clean |
| `mode` | Slide Deck | standalone, presenter | standalone |
| `format` | Audio | deep_dive, brief, critique, debate | deep_dive |
| `length` | Audio | shorter, default, longer | default |
| `tone` | Audio, Custom Report | Free text: "Casual and humorous" | (empty) |
| `structure` | Custom Report | Free text: "Blog post format with intro, 3 sections, conclusion" | (empty) |

### 6.4 Audio Overview Pipeline (Detail)

```
Selected sources
    │
    ▼
┌──────────────────────────────────────────┐
│  1. EXTRACT KEY POINTS                    │
│  agent-graph: map source summaries →      │
│  identify themes, surprising findings,    │
│  connections, controversies                │
│  Output: JSON { themes, findings, ... }   │
└───────────────────┬──────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  2. GENERATE SCRIPT                       │
│  LLM generates conversation as JSON:      │
│  [                                        │
│    { speaker: "A", text: "...",           │
│      emotion: "curious" },               │
│    { speaker: "B", text: "...",           │
│      emotion: "emphasis" },              │
│    ...                                    │
│  ]                                        │
│                                           │
│  Format-specific prompts:                 │
│    deep_dive: 2 hosts, in-depth, 10-15min │
│    brief: 1 host, key points, <2min       │
│    critique: 2 hosts, evaluative          │
│    debate: 2 hosts, opposing views        │
└───────────────────┬──────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  3. TTS RENDERING (Piper, CPU)            │
│  For each script segment:                 │
│    Speaker A → piper voice 1              │
│    Speaker B → piper voice 2              │
│  Insert silence between turns (200-500ms) │
│  Output: Vec<AudioSegment> (WAV 22050Hz)  │
└───────────────────┬──────────────────────┘
                    │
                    ▼
┌──────────────────────────────────────────┐
│  4. ASSEMBLY (hound)                      │
│  Concatenate segments with crossfade      │
│  Write final WAV to audio/{output_id}.wav │
│  Calculate duration, store metadata       │
└──────────────────────────────────────────┘
```

### 6.5 React Renderer Components

| Component | Used By | Key Features |
|-----------|---------|-------------|
| `MarkdownViewer` | Briefing Doc, Study Guide, FAQ, Custom Report | react-markdown, citation badges [Source: ...] rendered as clickable links |
| `FlashcardWidget` | Flashcards | Card stack, flip animation (CSS transform), Known/Review buttons, progress bar |
| `QuizWidget` | Quiz | One question at a time, option buttons, submit → show correct/incorrect + explanation, "Explain" button (extra LLM call), final score |
| `MindMapGraph` | Mind Map | d3-force layout, click node → citation sidebar, zoom/pan, export PNG |
| `TimelineView` | Timeline | Vertical CSS timeline, date markers, expandable event detail, citation badges |
| `DataTableView` | Data Table | Sortable columns, export CSV button, row highlight |
| `SlideViewer` | Slide Deck | 16:9 aspect, arrow key navigation, presenter notes toggle, export PDF |
| `InfographicView` | Infographic | Template-based HTML/SVG (stat, list, process, comparison templates), export PNG |
| `AudioPlayer` | Audio (all formats) | Play/pause, seek bar, speed control, transcript display synced to playback position, download button |

---

## 7. Retrieval & Conversation Engine

### 7.1 Retrieval Pipeline

```
User query
    │
    ├─── [Multi-angle rewrite] ──────────────────────────┐
    │    LLM generates 2 rephrased queries                │
    │    (costs 1 extra LLM call; significant quality     │
    │     boost for complex questions)                    │
    │                                                     │
    │    Original: "What are the risks?"                  │
    │    Rewrite 1: "potential dangers and threats"        │
    │    Rewrite 2: "risk factors and mitigation"          │
    │                                                     │
    ▼                                                     ▼
┌──────────────┐  ┌──────────────┐          (same for each rewrite)
│  Semantic    │  │  Keyword     │
│  Search      │  │  Search      │
│              │  │              │
│  fastembed   │  │  FTS5 BM25   │
│  query →     │  │  match →     │
│  HNSW top-20 │  │  top-20      │
└──────┬───────┘  └──────┬───────┘
       │                 │
       └────────┬────────┘
                │
                ▼
        ┌──────────────┐
        │  Reciprocal   │  Score = Σ 1/(60 + rank_i) across all queries
        │  Rank Fusion  │  Merge results, deduplicate by chunk_id
        │  (RRF)        │  Keep top-30
        └───────┬──────┘
                │
                ▼
        ┌──────────────┐
        │  Reranker     │  fastembed TextRerank (BGERerankerBase)
        │  (CPU)        │  Rerank top-30 → return top-6
        └───────┬──────┘
                │
                ▼
        ┌──────────────┐
        │  Source       │  Filter out chunks from deselected sources
        │  Filter       │  (WHERE source_id IN selected_sources)
        └───────┬──────┘
                │
                ▼
        Top-6 chunks with source attribution, ordered by relevance
```

**All retrieval runs on CPU.** The GPU is reserved for the final generation step.

### 7.2 Context Assembly

```rust
pub struct ContextBudget {
    pub total_tokens: u32,           // Model's context window (from ModelInfo)
    pub system_prompt_tokens: u32,   // ~500
    pub generation_headroom: u32,    // ~2500
    // Remaining = total - system - headroom, split between:
    pub max_chunk_tokens: u32,       // ~60% of remaining
    pub max_history_tokens: u32,     // ~40% of remaining
}
```

**Assembly order:**
1. System prompt (notebook goal + style instructions + citation format instructions)
2. Retrieved chunks (formatted with source attribution, sorted by relevance)
3. Conversation history (recent turns first; older turns summarized if they exceed budget)
4. User query
5. Generation headroom

**Dynamic adjustment:** If the conversation is long, reduce chunk count. If the query is complex (long, multi-part), allocate more to chunks. The budget manager adjusts per request.

### 7.3 Citation Format

The system prompt instructs the LLM to cite using numbered references:

```
When answering, cite sources using [1], [2], etc. At the end of your response,
list the citations:
[1] {source_title}, {section/page}
[2] {source_title}, {section/page}
```

The backend parses these from the response, maps them to chunk_ids and source_ids, and stores the mapping in `messages.citations`. The frontend renders `[1]` as a clickable badge that:
1. Highlights the citation in the source panel
2. Scrolls the source viewer to the matching passage (using `chunks.start_offset`)
3. Shows a hover tooltip with the quoted passage

### 7.4 Chat Configuration

| Setting | Scope | Options | Implementation |
|---------|-------|---------|----------------|
| Conversational style | Per-conversation | Default, Learning Guide, Custom | Modifies system prompt prefix |
| Custom goal | Per-notebook | Free-text | Prepended to system prompt for all conversations in notebook |
| Response length | Per-notebook | Shorter (512), Default (1536), Longer (4096) | Sets `max_tokens` in ChatRequest |
| Output language | Per-notebook | Language code | Adds "Respond in {language}" to system prompt |
| Source selection | Per-query | Checkboxes on sources | Filters chunks by source_id in retrieval |

**Learning Guide style** adds to the system prompt:
```
You are a patient tutor. Before answering, ask clarifying questions to understand
the user's learning goal and current knowledge level. Break complex topics into
steps. Use analogies. After explaining, check understanding with a quick question.
```

---

## 8. Frontend Architecture

### 8.1 Component Tree

```
<App>
├── <NotebookSidebar>
│   ├── <NotebookList>             # List of all notebooks, search, create
│   └── <NotebookCard>             # Name, source count, last accessed
│
├── <NotebookView>                 # Active notebook — the main workspace
│   ├── <PanelLayout>              # Resizable 3-panel split
│   │   ├── <SourcesPanel>
│   │   │   ├── <SourceUploadZone>  # Drag-drop + file picker + URL input + paste
│   │   │   ├── <SourceList>
│   │   │   │   └── <SourceCard>    # Title, type icon, status badge, checkbox, word count
│   │   │   └── <SourceDetail>      # Full-text viewer with chunk highlighting
│   │   │
│   │   ├── <ChatPanel>
│   │   │   ├── <ChatHeader>        # Conversation selector, style picker, settings
│   │   │   ├── <MessageList>
│   │   │   │   ├── <UserMessage>
│   │   │   │   └── <AssistantMessage>  # Markdown + citation badges + "Save to note"
│   │   │   ├── <SuggestedQuestions>    # 3-5 clickable question chips
│   │   │   └── <ChatInput>            # Text input + send + model selector
│   │   │
│   │   └── <StudioPanel>
│   │       ├── <StudioGrid>           # Tile grid of output types
│   │       │   └── <StudioTile>       # Icon, name, "Generate" button, count badge
│   │       ├── <StudioOutputList>     # Generated outputs for selected type
│   │       │   └── <StudioOutputCard> # Title, date, "View prompt", delete
│   │       └── <StudioOutputViewer>   # Renders the selected output using its renderer
│   │           ├── <MarkdownViewer>
│   │           ├── <FlashcardWidget>
│   │           ├── <QuizWidget>
│   │           ├── <MindMapGraph>
│   │           ├── <TimelineView>
│   │           ├── <DataTableView>
│   │           ├── <SlideViewer>
│   │           ├── <InfographicView>
│   │           └── <AudioPlayer>
│   │
│   └── <NotesSidebar>              # Toggleable side panel
│       ├── <NoteList>               # Pinned first, then by date
│       └── <NoteEditor>             # Markdown editor for manual notes
│
├── <SettingsDialog>
│   ├── <ProvidersTab>              # Ollama URL, OpenAI/Anthropic API keys, test/refresh
│   ├── <ModelsTab>                 # Default model selection, model list per provider
│   ├── <AudioTab>                  # Whisper model, Piper voices, TTS settings
│   ├── <ExternalToolsTab>          # ffmpeg/yt-dlp detection status
│   └── <AppearanceTab>             # Theme (system/light/dark)
│
└── <StatusBar>
    ├── <JobQueueIndicator>          # Active jobs, progress
    ├── <ModelIndicator>             # Currently loaded model + provider
    └── <NotebookStats>              # Source count, chunk count, conversation count
```

### 8.2 State Management (Zustand)

```typescript
// Core stores — Zustand slices

interface NotebookStore {
  notebooks: Notebook[];
  activeNotebookId: string | null;
  loadNotebooks: () => Promise<void>;
  createNotebook: (name: string) => Promise<string>;
  deleteNotebook: (id: string) => Promise<void>;
  setActive: (id: string) => void;
}

interface SourceStore {
  sources: Source[];
  selectedSourceIds: Set<string>;
  loadSources: (notebookId: string) => Promise<void>;
  addSource: (notebookId: string, file: File | string) => Promise<void>;
  toggleSource: (sourceId: string) => void;
  selectAll: () => void;
  selectNone: () => void;
}

interface ChatStore {
  conversations: Conversation[];
  activeConversationId: string | null;
  messages: Message[];
  isStreaming: boolean;
  streamingContent: string;
  sendMessage: (query: string) => Promise<void>;
  createConversation: () => Promise<string>;
  loadConversation: (id: string) => Promise<void>;
}

interface StudioStore {
  outputs: StudioOutput[];
  activeOutputType: string | null;
  activeOutputId: string | null;
  generate: (type: string, config: StudioConfig) => Promise<void>;
  loadOutputs: (notebookId: string) => Promise<void>;
}

interface NoteStore {
  notes: Note[];
  loadNotes: (notebookId: string) => Promise<void>;
  createNote: (content: string) => Promise<void>;
  saveResponse: (messageId: string) => Promise<void>;
  togglePin: (noteId: string) => Promise<void>;
}

interface SettingsStore {
  providers: Provider[];
  models: ModelInfo[];
  activeModel: string;
  settings: Record<string, string>;
  refreshModels: (provider?: string) => Promise<void>;
  updateSetting: (key: string, value: string) => Promise<void>;
}
```

### 8.3 Tauri IPC Commands

```rust
// Tauri command signatures — the bridge between React and Rust

// === Notebooks ===
#[tauri::command] async fn list_notebooks() -> Result<Vec<Notebook>>;
#[tauri::command] async fn create_notebook(name: String) -> Result<String>;
#[tauri::command] async fn delete_notebook(id: String) -> Result<()>;
#[tauri::command] async fn export_notebook(id: String, path: String, include_audio: bool) -> Result<()>;
#[tauri::command] async fn import_notebook(path: String) -> Result<String>;

// === Sources ===
#[tauri::command] async fn list_sources(notebook_id: String) -> Result<Vec<Source>>;
#[tauri::command] async fn add_source_file(notebook_id: String, path: String) -> Result<String>;
#[tauri::command] async fn add_source_url(notebook_id: String, url: String) -> Result<String>;
#[tauri::command] async fn add_source_paste(notebook_id: String, title: String, text: String) -> Result<String>;
#[tauri::command] async fn delete_source(notebook_id: String, source_id: String) -> Result<()>;
#[tauri::command] async fn get_source_content(notebook_id: String, source_id: String) -> Result<SourceContent>;

// === Chat ===
#[tauri::command] async fn list_conversations(notebook_id: String) -> Result<Vec<Conversation>>;
#[tauri::command] async fn create_conversation(notebook_id: String) -> Result<String>;
#[tauri::command] async fn delete_conversation(notebook_id: String, conversation_id: String) -> Result<()>;
#[tauri::command] async fn load_messages(notebook_id: String, conversation_id: String) -> Result<Vec<Message>>;
#[tauri::command] async fn send_message(
    notebook_id: String,
    conversation_id: String,
    query: String,
    selected_source_ids: Vec<String>,
    model: String,
) -> Result<String>; // Returns message_id; tokens streamed via events
#[tauri::command] async fn get_suggested_questions(notebook_id: String) -> Result<Vec<String>>;

// === Citations ===
#[tauri::command] async fn get_chunk_context(
    notebook_id: String,
    chunk_id: String,
) -> Result<ChunkContext>; // Returns chunk + surrounding text + source metadata + scroll offset

// === Notes ===
#[tauri::command] async fn list_notes(notebook_id: String) -> Result<Vec<Note>>;
#[tauri::command] async fn create_note(notebook_id: String, title: String, content: String) -> Result<String>;
#[tauri::command] async fn save_response_as_note(notebook_id: String, message_id: String) -> Result<String>;
#[tauri::command] async fn update_note(notebook_id: String, note_id: String, title: Option<String>, content: Option<String>) -> Result<()>;
#[tauri::command] async fn toggle_pin(notebook_id: String, note_id: String) -> Result<()>;
#[tauri::command] async fn delete_note(notebook_id: String, note_id: String) -> Result<()>;

// === Studio ===
#[tauri::command] async fn list_studio_outputs(notebook_id: String, output_type: Option<String>) -> Result<Vec<StudioOutput>>;
#[tauri::command] async fn generate_studio_output(
    notebook_id: String,
    output_type: String,
    config: StudioConfig,
    source_ids: Vec<String>,
) -> Result<String>; // Returns output_id; progress via events
#[tauri::command] async fn get_studio_output(notebook_id: String, output_id: String) -> Result<StudioOutput>;
#[tauri::command] async fn delete_studio_output(notebook_id: String, output_id: String) -> Result<()>;

// === Settings / Providers ===
#[tauri::command] async fn get_providers() -> Result<Vec<Provider>>;
#[tauri::command] async fn update_provider(provider: Provider) -> Result<()>;
#[tauri::command] async fn test_provider(provider_id: String) -> Result<bool>;
#[tauri::command] async fn refresh_models(provider_id: Option<String>) -> Result<Vec<ModelInfo>>;
#[tauri::command] async fn get_all_models() -> Result<Vec<ModelInfo>>;
#[tauri::command] async fn get_settings() -> Result<HashMap<String, String>>;
#[tauri::command] async fn update_setting(key: String, value: String) -> Result<()>;
#[tauri::command] async fn get_external_tools_status() -> Result<ExternalToolsStatus>; // ffmpeg, yt-dlp
```

### 8.4 Tauri Events (Backend → Frontend)

```rust
// Streamed chat tokens
"chat:token"     → { conversation_id, message_id, token: String, done: bool }

// Job queue progress
"job:started"    → { job_id, job_type, notebook_id }
"job:progress"   → { job_id, progress: f32, message: String }  // 0.0 - 1.0
"job:completed"  → { job_id, job_type, notebook_id }
"job:failed"     → { job_id, error: String }

// Source status changes
"source:status"  → { notebook_id, source_id, status, error_message? }

// Studio output progress
"studio:progress" → { output_id, stage: String, progress: f32 }
"studio:ready"    → { output_id, output_type }

// Model refresh
"models:updated"  → { provider_id, count: u32 }
```

---

## 9. Notebook Export/Import

### 9.1 `.gloss` Format

A `.gloss` file is a zip archive:

```
notebook-name.gloss (zip)
├── manifest.json
│   {
│     "schema_version": 1,
│     "gloss_version": "1.0.0",
│     "name": "My Research",
│     "created_at": "2025-02-14T...",
│     "source_count": 12,
│     "embedding_model": "NomicEmbedTextV15"
│   }
├── notebook.db            # Full SQLite database
├── sources/               # Original source files
│   ├── {hash}.pdf
│   └── {hash}.txt
├── embeddings/
│   └── chunks.usearch     # HNSW index
└── audio/                 # Generated audio (optional, can be large)
    └── {output_id}.wav
```

### 9.2 Import Logic

1. Unzip to temp directory
2. Read manifest, verify `schema_version` is supported
3. Create new notebook directory, copy files
4. Register in `gloss.db`
5. If `manifest.embedding_model` differs from current setting → queue `ReindexNotebook` job
6. Notify user: "Imported. {n} sources, {reindex_needed ? 'reindexing...' : 'ready'}"

---

## 10. Crate Dependencies

```toml
[package]
name = "gloss"
version = "1.0.0"
edition = "2024"

[dependencies]
# === Your libraries ===
llm-pipeline = { path = "../../Libraries/llm-pipeline" }
agent-graph = { path = "../../Libraries/agent-graph" }
tauri-queue = { path = "../../Libraries/tauri-queue" }

# === Tauri ===
tauri = { version = "2", features = ["dialog", "fs", "shell", "clipboard"] }
tauri-plugin-dialog = "2"
tauri-plugin-fs = "2"
tauri-plugin-shell = "2"
tauri-plugin-clipboard-manager = "2"
tauri-plugin-store = "2"               # Encrypted API key storage

# === Async ===
tokio = { version = "1", features = ["full"] }

# === Serialization ===
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# === LLM Providers ===
reqwest = { version = "0.12", features = ["json", "stream", "rustls-tls"] }

# === Embeddings + Reranking ===
fastembed = "5"

# === Vector search ===
usearch = "2"

# === Database ===
rusqlite = { version = "0.32", features = ["bundled", "fts5"] }

# === Document processing ===
lopdf = "0.34"                         # PDF text extraction
calamine = "0.26"                      # Excel/ODS reading
csv = "1"                              # CSV/TSV parsing
scraper = "0.21"                       # HTML parsing
readability = "0.3"                    # Article extraction from HTML
zip = "2"                              # Notebook export/import + DOCX extraction

# === YouTube ===
yt-transcript-rs = "0.1"              # Pure Rust YouTube transcript API

# === Audio/Video ===
whisper-rs = { version = "0.15", features = [] }  # Add "cuda" feature when ready
hound = "3"                            # WAV file I/O
rodio = "0.19"                         # Audio format conversion

# === TTS ===
piper-rs = "0.3"                       # Piper TTS via ONNX runtime

# === Text processing ===
tiktoken-rs = "0.6"                    # Token counting for context budget
unicode-segmentation = "1"             # Text boundary detection

# === Utilities ===
uuid = { version = "1", features = ["v4", "serde"] }
sha2 = "0.10"
chrono = { version = "0.4", features = ["serde"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
thiserror = "2"
anyhow = "1"
walkdir = "2"
directories = "5"

[dev-dependencies]
tempfile = "3"
```

### 10.1 Frontend Dependencies

```json
{
  "dependencies": {
    "react": "^19",
    "react-dom": "^19",
    "@tauri-apps/api": "^2",
    "@tauri-apps/plugin-dialog": "^2",
    "@tauri-apps/plugin-fs": "^2",
    "@tauri-apps/plugin-clipboard-manager": "^2",
    "@tauri-apps/plugin-store": "^2",
    "zustand": "^5",
    "react-markdown": "^9",
    "react-syntax-highlighter": "^15",
    "rehype-raw": "^7",
    "d3": "^7",
    "lucide-react": "^0.450"
  },
  "devDependencies": {
    "typescript": "^5.5",
    "tailwindcss": "^4",
    "@vitejs/plugin-react": "^4",
    "vite": "^6"
  }
}
```

### 10.2 External Binary Dependencies

| Binary | Purpose | Required For | Detection |
|--------|---------|-------------|-----------|
| `ffmpeg` | Audio extraction from video files | Video source ingestion | Check `which ffmpeg` at startup |
| `ffprobe` | Media file probing | Video source ingestion | Ships with ffmpeg |
| `yt-dlp` | YouTube audio download (fallback) | YouTube videos without transcripts | Optional, check `which yt-dlp` |

Missing binaries don't crash the app — the relevant ingestion paths are disabled with a tooltip: "Install ffmpeg to enable video ingestion."

---

## 11. Implementation Phases

### Phase 1 — Foundation (Week 1-2)

**Goal:** Working notebook with text ingestion and grounded chat.

**Backend:**
- [ ] Tauri 2 + React project scaffold
- [ ] SQLite schema (gloss.db + per-notebook DB) with all tables
- [ ] Notebook manager: create, open, delete, list
- [ ] Provider registry: Ollama only, list models, health check
- [ ] Source ingestion: text, markdown, paste (direct read)
- [ ] Chunking engine (recursive character split, format-aware boundaries)
- [ ] Embedding pipeline (fastembed-rs, NomicEmbedTextV15)
- [ ] HNSW index (usearch): add, search, serialize/deserialize
- [ ] FTS5 full-text index: insert on chunk, BM25 search
- [ ] Basic RAG: embed query → HNSW top-K → FTS5 top-K → RRF merge → stuff → generate
- [ ] Streaming chat via Tauri events (chat:token)
- [ ] Per-source summary on ingest (single LLM call)
- [ ] Citation extraction from LLM responses
- [ ] Suggested questions generation (3 questions from summaries)
- [ ] Notes: create, list, save response as note, pin/unpin, delete

**Frontend:**
- [ ] 3-panel layout (Sources | Chat | Studio) with resizable splitters
- [ ] Notebook sidebar with list, create, delete
- [ ] Source upload zone (drag-drop + file picker + paste text dialog)
- [ ] Source list with status badges, type icons, checkboxes
- [ ] Chat: message list, streaming display, citation badges (clickable → source scroll)
- [ ] Chat: conversation list, create new, model selector dropdown
- [ ] Suggested question chips above input
- [ ] Source detail viewer (text display, chunk highlighting on citation click)
- [ ] Notes panel (list, create, pin, edit, delete)
- [ ] Status bar (job progress, model indicator)
- [ ] Basic settings dialog (Ollama URL, test connection)

**Verify:** Add 3 .txt files → ask a cross-source question → get a cited answer → click citation → source viewer scrolls to passage → save response as note → note appears pinned.

### Phase 2 — Rich Ingestion + Studio Reports (Week 3-4)

**Goal:** All major document formats work. First studio outputs.

**Backend:**
- [ ] PDF extraction (lopdf; detect garbled output for vision model fallback later)
- [ ] DOCX extraction (XML unzip via zip crate + text extraction)
- [ ] XLSX/CSV extraction (calamine rows → structured text)
- [ ] URL ingestion (reqwest + readability)
- [ ] YouTube transcript (yt-transcript-rs)
- [ ] Job queue integration (tauri-queue) for all ingestion pipelines
- [ ] Hybrid search with fastembed reranker (BGERerankerBase, CPU)
- [ ] Multi-angle query rewriting (generate 2 rephrased queries, merge results)
- [ ] Source selection filtering in retrieval
- [ ] Studio output pipeline: load template → fill prompt → LLM generate → parse → store
- [ ] Studio templates: briefing_doc, study_guide, faq, timeline, custom_report, data_table
- [ ] Chat: conversational styles (Default, Learning Guide, Custom)
- [ ] Chat: custom goals (per-notebook system prompt)
- [ ] Chat: response length control (shorter/default/longer)
- [ ] OpenAI + Anthropic provider implementations
- [ ] Model refresh across all configured providers

**Frontend:**
- [ ] Source upload: accept PDF, DOCX, XLSX, CSV; URL input field; YouTube URL detection
- [ ] Job queue progress indicators on source cards
- [ ] Chat settings panel (style, goal, response length)
- [ ] Model selector: grouped by provider, refresh button
- [ ] Studio panel: tile grid for output types (6 report types)
- [ ] Studio: generate dialog (options: focus, language, count for applicable types)
- [ ] Studio: output list per type, "View prompt" button
- [ ] Studio: MarkdownViewer renderer (for all report types)
- [ ] Studio: TimelineView renderer (vertical timeline)
- [ ] Studio: DataTableView renderer (sortable table, CSV export)
- [ ] Settings: provider management (Ollama, OpenAI, Anthropic), API key input, test, refresh
- [ ] Settings: external tools status (ffmpeg, yt-dlp)

**Verify:** Drop PDF + YouTube URL + web link → all ingest successfully → ask synthesis question → get reranked, multi-source cited answer → generate briefing doc + FAQ + timeline → all render correctly with citations → switch to GPT-4o → response streams from OpenAI.

### Phase 3 — Interactive Outputs + Media Ingestion (Week 5-6)

**Goal:** Flashcards, quizzes, mind maps. Audio/video ingestion.

**Backend:**
- [ ] Studio templates: flashcards, quiz, mind_map
- [ ] Flashcard: JSON generation + validation + retry on parse failure
- [ ] Quiz: JSON generation + "Explain" endpoint (extra LLM call per question)
- [ ] Mind map: concept/relationship extraction as JSON graph
- [ ] Audio file ingestion (whisper-rs, CPU mode, via rodio for format conversion)
- [ ] Video file ingestion (ffmpeg CLI audio extraction → whisper-rs)
- [ ] Image ingestion (vision model via Ollama, or skip if no vision model available)
- [ ] Notebook export (.gloss zip creation)
- [ ] Notebook import (.gloss zip extraction + optional re-embed)

**Frontend:**
- [ ] FlashcardWidget (card flip, Known/Review, progress tracking)
- [ ] QuizWidget (question display, answer selection, scoring, Explain button)
- [ ] MindMapGraph (d3-force, clickable nodes → citation sidebar, zoom/pan)
- [ ] Source upload: accept audio files (.mp3, .wav, .ogg, .flac, .m4a)
- [ ] Source upload: accept video files (.mp4, .mkv, .webm) — shown only if ffmpeg detected
- [ ] Transcription progress indicators (whisper is slow; show estimated time)
- [ ] Notebook export/import buttons in notebook context menu

**Verify:** Drop an MP3 → see transcription progress → ask questions about audio content → generate flashcards → flip through them → take quiz → score displayed → mind map renders with clickable nodes → export notebook → import on fresh install → all data intact.

### Phase 4 — Audio Overviews + Visual Outputs (Week 7-8)

**Goal:** Audio podcast generation. Slide decks and infographics.

**Backend:**
- [ ] Audio overview: key point extraction (agent-graph map-reduce)
- [ ] Audio overview: script generation (4 formats: deep_dive, brief, critique, debate)
- [ ] Audio overview: Piper TTS integration (piper-rs, 2 voices)
- [ ] Audio overview: segment concatenation with silence gaps (hound)
- [ ] Audio overview: customization (tone, length, focus topic)
- [ ] Slide deck: structured slide JSON generation
- [ ] Infographic: structured section/stat JSON generation
- [ ] Multiple outputs per type (no limits, stored as rows)
- [ ] "View custom prompt" on all studio outputs

**Frontend:**
- [ ] AudioPlayer (play/pause, seek, speed control, transcript display, download button)
- [ ] Audio generation dialog (format selector, customization options)
- [ ] SlideViewer (16:9 aspect, arrow navigation, presenter notes toggle)
- [ ] Slide deck PDF export (via window.print or html2canvas)
- [ ] InfographicView (template selector: stat/list/process/comparison)
- [ ] Infographic PNG export (html2canvas)
- [ ] Settings: Piper voice selection, whisper model selection
- [ ] Studio: multiple outputs per type displayed as a list with dates

**Verify:** Select 3 sources → generate Deep Dive audio → listen to coherent 10-min podcast → download WAV → generate slide deck → navigate slides → export as PDF → generate infographic → export as PNG.

---

## 12. Quality & Reliability

### 12.1 Structured Output Validation

Every studio output that expects JSON from the LLM must handle malformed responses:

```rust
fn parse_studio_output(raw: &str, expected_format: &str) -> Result<serde_json::Value> {
    // 1. Try direct parse
    if let Ok(v) = serde_json::from_str(raw) { return Ok(v); }

    // 2. Try extracting JSON from markdown code fences
    if let Some(json_block) = extract_json_from_markdown(raw) {
        if let Ok(v) = serde_json::from_str(&json_block) { return Ok(v); }
    }

    // 3. Retry with stricter prompt (1 retry max)
    Err(Error::MalformedOutput { raw: raw.to_string(), expected: expected_format.to_string() })
}
```

On parse failure: retry once with an even more explicit prompt ("Return ONLY valid JSON, no markdown, no explanations"). If still fails: store the raw output with status `error` and surface the error to the user with "Try again" button.

### 12.2 Error Handling Strategy

| Error Type | User Experience | Backend Behavior |
|------------|----------------|------------------|
| Ollama unreachable | Red status bar indicator: "Cannot connect to Ollama at {url}" | Retry 3x with backoff. Surface error. Don't block UI. |
| Model not found | "Model {name} not available. Refresh models?" | Trigger model refresh. Suggest selecting a different model. |
| Ingestion failure | Source card shows error badge with message | Store error in `sources.error_message`. Allow retry. |
| LLM returns malformed JSON | "Generation failed. Retrying..." | Auto-retry once with stricter prompt. Then surface error. |
| whisper/ffmpeg not found | Feature disabled with tooltip | Check on startup. Don't crash. Gray out unavailable features. |
| Disk full during audio generation | "Not enough disk space for audio generation" | Check available space before starting TTS pipeline. |
| Context window exceeded | Silent: fewer chunks included | Dynamic budget management adjusts automatically. Log warning. |

### 12.3 Graceful Degradation

The app runs at three capability tiers depending on available infrastructure:

| Tier | Requirements | Available Features |
|------|-------------|-------------------|
| **Minimal** | Ollama running + one chat model | Chat, text/MD/paste ingestion, basic search, notes |
| **Standard** | + fastembed working, + PDF/web libs | + rich ingestion, hybrid search, reranker, all studio outputs |
| **Full** | + ffmpeg, + whisper model, + Piper voices | + audio/video ingestion, audio overviews |

Each tier works correctly without the features of higher tiers. Missing capabilities are surfaced as disabled UI elements with explanatory tooltips, never as errors.

---

## 13. Risk Register

| Risk | Probability | Impact | Mitigation |
|------|-------------|--------|------------|
| 8B model produces low-quality studio outputs | Medium | High | Invest in prompt engineering. Structured JSON output with schema. Auto-retry on malformed output. Cloud providers (GPT-4o) available as fallback. |
| PDF extraction garbage for complex layouts | High | Medium | Detect low-quality extraction by character entropy. Queue vision model fallback. Show warning badge on source. |
| fastembed ONNX + whisper-rs ONNX version conflict | Low | High | Both use `ort` crate. Pin to compatible versions. Test coexistence in Phase 1. |
| Context window too small for complex queries | Medium | High | Aggressive budget management. Summarize old turns. Multi-angle retrieval compensates with better chunk selection. |
| Piper voice quality disappoints | Medium | Low | Audio overviews are Phase 4. Core value is RAG chat, not TTS. Users can skip audio entirely. |
| Structured JSON parsing failures from LLM | High | Medium | Auto-retry with stricter prompt. Accept that 8B models need more hand-holding. Template prompts include explicit schema. |
| Large video files exhaust disk during transcription | Medium | Medium | Temp file cleanup after audio extraction. Show estimated disk usage before ingestion. |
| React bundle size too large | Low | Low | Desktop app — no network transfer. Tree-shake. Lazy-load heavy renderers (d3, slide viewer). |

---

## 14. Key Design Decisions Summary

| Decision | Choice | Why |
|----------|--------|-----|
| Name | Gloss | Short, unique, semantically perfect |
| Frontend | React 19 + TypeScript + Vite | Ecosystem depth for widget-heavy app |
| State management | Zustand | Minimal boilerplate, Tauri IPC–friendly |
| CSS | TailwindCSS 4 | Utility-first, dark mode, fast iteration |
| Components | shadcn/ui | Accessible, unstyled, customizable |
| LLM orchestration | llm-pipeline + agent-graph + tauri-queue | Your existing libraries, proven patterns |
| Embedding | fastembed 5.x (CPU) | Decoupled from GPU, built-in reranker |
| Reranking | fastembed BGERerankerBase (CPU) | No GPU swap needed; entire retrieval on CPU |
| Vector store | usearch (embedded HNSW) | Portable notebooks, no server process |
| Database | Per-notebook SQLite + app-level SQLite | Notebook = unit of portability |
| TTS | Piper via piper-rs (CPU) | Shares ONNX runtime with fastembed, no VRAM |
| STT | whisper-rs (CPU default, CUDA optional) | Best-in-class local transcription |
| YouTube | yt-transcript-rs (pure Rust) | Zero external deps for the common case |
| API key storage | tauri-plugin-store (OS keyring) | Never plaintext on disk |
| Studio architecture | Template-driven: TOML config → prompt → LLM → renderer | Adding output type = 1 template, 0-1 renderers. Linear complexity. |
| Notebook export | .gloss (zip archive) | Self-contained, Syncthing-friendly, schema-versioned |
| Deferred: AI images | Template-based SVG/HTML instead | No local image model at 8GB VRAM |
| Deferred: Audio Interactive | Sequential pipeline (5-15s latency) | Can't run STT + LLM + TTS simultaneously on one GPU |
| Deferred: Deep Research | Requires cloud provider | 8B models unreliable for multi-step web agents |
