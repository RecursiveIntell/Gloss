# Gloss

**Current run:** `GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526`

**Privacy-first local RAG, right on your desktop.**

Gloss is a local-first desktop application for intelligent document research, retrieval-augmented generation (RAG) chat, and AI-assisted knowledge work. Import your sources, chat with them using local or cloud LLMs, and generate structured study materials — all without sending your documents to third parties.

Built with **Tauri 2** (Rust backend) and **React 19 + Zustand** (TypeScript frontend), Gloss keeps your data on your machine by default and connects to cloud providers only when you explicitly configure them.

---

## Table of Contents

- [Features](#features)
  - [Notebook Management](#notebook-management)
  - [Source Ingestion](#source-ingestion)
  - [RAG Chat](#rag-chat)
  - [Studio Outputs](#studio-outputs)
  - [Retrieval Engine](#retrieval-engine)
  - [Semantic Memory](#semantic-memory)
  - [Notes System](#notes-system)
  - [Settings and Providers](#settings-and-providers)
  - [Receipt and Observability System](#receipt-and-observability-system)
- [Architecture](#architecture)
- [Tech Stack](#tech-stack)
- [Getting Started](#getting-started)
- [Feature Flags](#feature-flags)
- [Graceful Degradation](#graceful-degradation)
- [Security and Egress](#security-and-egress)
- [Notebook Export / Import](#notebook-export--import-gloss-format)
- [Related Projects](#related-projects)
- [License](#license)

---

## Features

### Notebook Management

- Create, rename, delete, and list notebooks from the sidebar
- Each notebook is an isolated workspace with its own database, sources, and conversations
- **Export/import** notebooks as portable `.gloss` archives — full fidelity round-trip including SQLite databases, source files, and vector indices
- **Database Doctor** for integrity verification: orphan row detection, source count mismatches, stale job cleanup
- Epoch-based soft-cancellation ensures correct behavior when switching notebooks mid-operation

### Source Ingestion

Gloss ingests a wide range of document formats with format-aware text extraction:

| Category | Formats | Details |
|----------|---------|---------|
| Text & Code | `.txt`, `.md`, `.rs`, `.py`, `.js`, `.ts`, etc. | Direct text extraction |
| Documents | `.pdf`, `.docx`, `.epub`, `.pptx` | lopdf (PDF), zip+XML (DOCX/PPTX), epub extraction |
| Spreadsheets | `.xlsx`, `.csv`, `.tsv` | Tabular data via calamine |
| Web | URLs | `reqwest` + readability article extraction |
| YouTube | URLs | Transcript extraction via yt-transcript-rs API |
| Audio | `.mp3`, `.wav`, `.ogg`, `.flac`, `.m4a` | Whisper transcription (whisper-rs) |
| Video | `.mp4`, `.mkv`, `.webm`, `.mov`, `.avi` | ffmpeg audio extraction → Whisper |
| Images | `.png`, `.jpg`, `.webp`, `.gif` | Vision model description (Ollama multimodal) |
| Clipboard | Paste text | Direct text input |

Each source goes through a multi-stage pipeline:

1. **Extract** — format-aware text extraction
2. **Chunk** — recursive character splitting with format-aware boundaries
3. **Embed** — FastEmbed (NomicEmbedTextV15, 768-dimensional) dense vectors
4. **Index** — FTS5 full-text index + HNSW vector index (usearch)
5. **Summarize** — LLM-generated per-source summary (background job)
6. **Optional** — vision description for images, Whisper transcription for audio, ffprobe metadata for media

Sources track their full processing lifecycle with per-stage status: `pending → processing → ready` (or `error`), with failed-import quarantine and retry.

### RAG Chat

- **Multi-provider LLM support**: Ollama (local), OpenAI, Anthropic, LlamaCpp
- **Streaming responses** via Tauri events — see tokens as they arrive
- **Three source scoping modes**:
  - `none` — no source filtering, free-form chat
  - `all` — use every source in the notebook
  - `explicit` — select specific sources for targeted queries
- **Chat styles**: Default, Learning Guide, Custom goal — each steers the system prompt differently
- **Response length**: shorter / default / longer
- **Suggested questions** — auto-generated from source summaries to guide exploration
- **Citations with evidence disclosure** — every response can show which chunks contributed, with source references and chunk highlighting
- **GPU/LLM concurrency gate** — single-flight for GPU operations prevents resource contention
- **Chat grace window** — 15-second cooldown that blocks background summaries during active chat

### Studio Outputs

Generate structured outputs from your notebook sources:

| Output Type | Description |
|-------------|-------------|
| Briefing Doc | Executive summary of the source material |
| Study Guide | Structured study guide with key concepts |
| FAQ | Frequently asked questions with answers sourced from your data |
| Timeline | Chronological overview of events mentioned in sources |
| Custom Report | User-specified report format and focus |
| Data Table | Structured data extracted from unstructured text |
| Flashcards | Interactive flashcard widget for review |
| Quiz | Interactive quiz widget for self-testing |
| Mind Map | Force-directed graph visualization of concepts |

Studio outputs are bound to source selections and include progress events for generation stages. All outputs are exportable.

### Retrieval Engine

Gloss implements a multi-tier retrieval system with graceful fallback:

```
SemanticMemory → HybridRRF → DenseOnly → BM25Only → SourceOrderFallback → RawContentFallback
```

- **BM25 (FTS5)** — full-text search over source chunks
- **Dense vector search** — HNSW index via usearch with FastEmbed/NomicEmbedTextV15 embeddings (768-dim)
- **Reciprocal Rank Fusion (RRF)** — merges BM25 and dense results for hybrid retrieval
- **BGEReranker** — cross-encoder reranking when available
- **Source scoping** — restrict retrieval to explicit source selections
- **Coverage analysis and diagnostics** — inspect retrieval outcomes, engine status, and reason codes

### Semantic Memory System

Gloss includes an integrated semantic memory system powered by its own vector index:

- **Local embedding**: FastEmbed (NomicEmbedTextV15) — no external embedding API required
- **HNSW vector index**: usearch for approximate nearest neighbor search
- **Full-text search**: SQLite FTS5 for BM25 retrieval
- **TurboQuant validation profiles**: optional compressed sidecar codec for vector search experiments via the [`turbo-quant`](https://github.com/RecursiveIntell/turbo-quant) crate
- **Memory backend profile switching** per notebook (Gloss-local or Semantic Memory Preview)
- **Vector artifact management**: build, rebuild, and check status of vector indices
- **Retrieval capability decision engine** with automatic fallback chains

### Notes System

- Create, edit, delete notes within any notebook
- Pin/unpin important notes for quick access
- Save chat responses directly as notes
- Note types: manual and saved_response
- Notes persist per-notebook and export with `.gloss` archives

### Settings and Providers

- **Provider management**: configure Ollama, OpenAI, Anthropic, and LlamaCpp endpoints
- **Encrypted API key storage** via Tauri's SecretStore — keys never stored in plaintext
- **LAN security**: RFC1918 address detection with configurable allow-list for local network providers
- **Model registry**: per-provider model lists with health testing / smoke tests
- **Feature flags**: 16+ feature flags for experimental capabilities
- **External tools**: optional ffmpeg, ffprobe, yt-dlp integration with graceful disable when missing

### Receipt and Observability System

Every significant operation in Gloss produces a versioned receipt for audit and debugging:

| Receipt Type | What it Records |
|---|---|
| `ToolInvocationReceiptV1` | External tool name, action, args (redacted), timeout, exit code, stdout/stderr digests |
| `ChatEvidenceDisclosure` | Retrieval mode, fallback chain, source scope decisions, citation anchors, filter reasons |
| `DecodingSettingsReceiptV1` | Provider, model, requested vs effective settings (temperature, top_p, etc.) |
| `PromptReceiptV1` | Prompt digest, context digest, source passage count |
| `GenerationReceiptV1` | Provider, model, chunks seen, done frame status |
| `PromptBudgetReceiptV1` | Model context window, estimated tokens, context budgeted |
| `EmbeddingDiagnosticsReceipt` | FastEmbed init/embed status, dimensions, optional Ollama embed check |
| `DbDoctorReceipt` | Findings, notebook reports, orphan cleanup, stale job cleanup |
| `NotebookExportReceipt` | Package format, file count, manifest digest |
| `FailedImportQuarantineReceipt` | Quarantined/deleted source counts |
| `YouTubeTranscriptReceipt` | Video ID digest, language, segment count, timing |
| `StudioExportReceipt` | Output type, format, bytes written, SHA-256 |

---

## Architecture

```
┌──────────────────────────────────────────────────────────────┐
│                        Gloss Desktop                         │
│                                                              │
│  ┌─────────────────────────────────────────────────────────┐ │
│  │               Frontend (React 19 + Zustand)            │ │
│  │  ┌──────────┐  ┌──────────┐  ┌────────────────────┐  │ │
│  │  │ Sources   │  │   Chat   │  │      Studio        │  │ │
│  │  │  Panel    │  │  Panel   │  │      Panel         │  │ │
│  │  └──────────┘  └──────────┘  └────────────────────┘  │ │
│  │  ┌──────────┐  ┌──────────┐  ┌────────────────────┐  │ │
│  │  │ Notebook │  │  Notes   │  │   Inspector Dock    │  │ │
│  │  │ Sidebar  │  │  Panel   │  │ (Evidence/Diag/    │  │ │
│  │  └──────────┘  └──────────┘  │  Receipt/Prompt)    │  │ │
│  │                               └────────────────────┘  │ │
│  └────────────────────┬────────────────────────────────────┘ │
│                       │ Tauri IPC (~70 commands)              │
│  ┌────────────────────┴────────────────────────────────────┐ │
│  │              Backend (Rust / Tauri 2)                    │ │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────────────────┐ │ │
│  │  │ Commands │  │  Jobs    │  │    Providers          │ │ │
│  │  │ (12 files)│  │ Queue   │  │ Ollama/OpenAI/       │ │ │
│  │  └──────────┘  └──────────┘  │ Anthropic/LlamaCpp   │ │ │
│  │  ┌──────────┐  ┌──────────┐  └──────────────────────┘ │ │
│  │  │ SQLite   │  │ Embed +  │  ┌──────────────────────┐ │ │
│  │  │ (WAL)    │  │ Vector   │  │   Receipt Engine      │ │ │
│  │  └──────────┘  └──────────┘  └──────────────────────┘ │ │
│  └────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────┘
```

**Key architectural decisions:**

- **Single-flight GPU/LLM gate**: At most one inference request at a time via tokio `Semaphore(1)`, preventing resource contention between chat and background jobs
- **Chat preemption**: Chat requests take priority over background summarization; a 15-second grace window blocks summaries during active chat
- **Epoch-based notebook switching**: Soft-cancellation of stale jobs when switching notebooks, preventing cross-notebook contamination
- **Local-first data**: All data stored in per-notebook SQLite databases (WAL mode); no server-side storage
- **Streaming via Tauri events**: `chat:token`, `source:status`, `studio:progress`, `job:*` events flow from Rust to frontend in real-time

---

## Tech Stack

### Frontend

| Technology | Purpose |
|-----------|---------|
| React 19 | UI framework |
| Zustand 5 | State management (7 stores, 50K+ lines) |
| TypeScript 5.5 | Type safety |
| Vite 7 | Build tooling |
| TailwindCSS 4 | Design system and styling |
| d3-force | Mind map force layout |
| lucide-react | Icons |
| react-markdown | Markdown rendering with rehype-raw |
| react-syntax-highlighter | Code block syntax highlighting |

### Backend (Rust)

| Crate | Purpose |
|-------|---------|
| tauri 2 | Desktop application framework |
| tokio | Async runtime |
| rusqlite (FTS5, bundled) | SQLite databases with full-text search |
| fastembed 4 | Local embeddings (NomicEmbedTextV15, BGEReranker) |
| usearch 2 | HNSW vector index |
| semantic-memory | Semantic memory backend with TurboQuant codec |
| llm-pipeline | LLM provider abstraction |
| tauri-queue | Background job queue |
| reqwest (rustls) | HTTP client (streaming responses) |
| lopdf / calamine / quick-xml | Document extraction |
| whisper-rs | Audio transcription |
| piper-rs | Text-to-speech |
| sha2 / uuid / chrono | Utilities |
| tauri-plugin-* | Dialog, FS, shell, clipboard, store (encrypted API keys) |

---

## Getting Started

### Prerequisites

- **Rust** 1.75+ (for backend compilation)
- **Node.js** 18+ (for frontend tooling)
- **Ollama** running locally with at least one chat model (for core functionality)

Optional for enhanced ingestion:
- **ffmpeg** — audio extraction from video files
- **ffprobe** — media metadata (ships with ffmpeg)
- **yt-dlp** — YouTube download fallback
- **Whisper model** — audio transcription (configured in Ollama)

### Build

```bash
# Clone the repository
git clone https://github.com/RecursiveIntell/Gloss.git
cd Gloss

# Install frontend dependencies
npm install

# Development with semantic memory + TurboQuant
npm run tauri:dev:sm-tq

# Production build
npm run tauri:build:sm-tq
```

### Feature Flags

Build with different retrieval backends:

| Command | Features |
|---------|----------|
| `npm run tauri:dev:sm` | Semantic memory (no TurboQuant codec) |
| `npm run tauri:dev:sm-tq` | Semantic memory + TurboQuant codec (recommended) |

---

## Feature Flags

Gloss has a rich feature flag system controlled from Settings. Key flags:

| Flag | Purpose |
|------|---------|
| `experimental_features_enabled` | Master switch for experimental features |
| `feature_semantic_memory_preview_enabled` | Semantic Memory Preview backend |
| `feature_semantic_memory_turbo_quant_enabled` | TurboQuant vector compression codec |
| `feature_chat_diagnostics_enabled` | Detailed chat diagnostics and receipts |
| `feature_provider_smoke_tools_enabled` | Provider health testing tools |
| `feature_advanced_retrieval_controls_enabled` | Advanced retrieval configuration |
| `feature_index_replay_tools_enabled` | Index replay debugging tools |
| `feature_vision_jobs_enabled` | Vision description for images |
| `feature_video_import_enabled` | Video file import with audio extraction |
| `feature_flashcard_widget_enabled` | Interactive flashcard studio output |
| `feature_quiz_widget_enabled` | Interactive quiz studio output |
| `feature_mind_map_widget_enabled` | Force-directed mind map visualization |
| `feature_background_summaries_enabled` | Background source summarization |
| `feature_external_tools_enabled` | External tool integration (ffmpeg, etc.) |
| `feature_local_rag_enabled` | Local RAG chat |
| `feature_source_scope_enabled` | Source scope selection for chat |

---

## Graceful Degradation

Gloss degrades gracefully when optional components are unavailable. Missing capabilities surface as disabled UI elements with explanatory tooltips — never as errors.

| Tier | Requirements | Available Features |
|------|-------------|-------------------|
| **Minimal** | Ollama running + one chat model | Chat, text/MD/paste ingestion, basic search, notes |
| **Standard** | + fastembed + PDF/web libs | Rich ingestion, hybrid search, reranker, all studio outputs |
| **Full** | + ffmpeg + Whisper model + Piper voices | Audio/video ingestion, audio overviews, vision description |

---

## Security and Egress

Gloss enforces strict egress controls on network requests:

- **Local providers** (localhost, 127.0.0.1, ::1) — always allowed
- **Cloud providers** — only `api.openai.com` and `api.anthropic.com` permitted
- **LAN addresses** (RFC1918: 10.x, 172.16-31.x, 192.168.x, fc00::/7) — blocked unless `allow_lan_local_providers` is explicitly enabled
- **API keys** — encrypted at rest via Tauri SecretStore, never stored in plaintext
- **Content Security Policy** — strict CSP headers enforced in the Tauri webview

---

## Notebook Export / Import (`.gloss` format)

A `.gloss` file is a zip archive containing:

```
notebook.gloss
├── manifest.json          # schema_version, gloss_version, name, source_count, embedding_model
├── notebook.db            # SQLite database (conversations, sources, chunks, notes)
├── sources/               # Original source files (hashed filenames)
├── embeddings/             # HNSW index files (usearch binary)
└── audio/                  # Generated audio files (optional)
```

Import logic: unzip → verify manifest schema → create notebook directory → copy files → register in global `gloss.db` → optionally re-embed if the embedding model differs from the original.

---

## Related Projects

### turbo-quant — Vector Compression Sidecars

Gloss uses the [`turbo-quant`](https://github.com/RecursiveIntell/turbo-quant) crate for vector compression experiments in its semantic memory system. `turbo-quant` is an experimental Rust crate for derived vector-compression sidecars inspired by TurboQuant, PolarQuant, and Quantized Johnson-Lindenstrauss (QJL) sketches.

**Key capabilities:**
- **PolarQuant** — Cartesian-to-polar compression with bitpacked angle indices and lossless radius storage
- **QJL sketches** — Random Gaussian hyperplane projections stored as binary signs for residual estimation
- **TurboQuant** — Two-stage codec combining PolarQuant + QJL residual, estimating inner product as `IP_polar + IP_qjl`
- **Sidecar candidate search** — `TurboSidecarIndex` returns approximate candidates with explicit `SearchReceiptV1` (always marks `approximate_only: true, exact_rerank_required: true`)
- **KV-cache shadow mode** — Experiment surface for measuring compressed key/value behavior with exact fallback comparison
- **Deterministic and data-oblivious** — No trained codebook, no k-means, same parameters always produce identical quantizers

**Recommended parameters:**

| Use case | bits | projections |
|----------|------|-------------|
| Semantic search (recall@10) | 8 | dim/4 |
| KV cache compression | 4-6 | dim/8 |
| Maximum compression | 3 | dim/16 |

**Learn more:**
- **GitHub**: [RecursiveIntell/turbo-quant](https://github.com/RecursiveIntell/turbo-quant)
- **crates.io**: [turbo-quant](https://crates.io/crates/turbo-quant)
- **Medium** — [Vector Compression Sidecars with turbo-quant](https://medium.com/@sikmindz/turbo-quant-vector-compression-sidecars) <!-- TODO: Replace with actual Medium URL -->
- **dev.to** — [TurboQuant: Polar Quantization and QJL Sketches for Rust](https://dev.to/sikmindz/turbo-quant-polar-quantization-qjl-sketchs-rust) <!-- TODO: Replace with actual dev.to URL -->
- **PyTorch version** — [turbo-quant-pytorch](https://github.com/RecursiveIntell/turbo-quant-pytorch) <!-- TODO: Replace with actual PyTorch version URL -->

### Other Libraries

| Crate | Purpose |
|-------|---------|
| [semantic-memory](https://github.com/RecursiveIntell/semantic-memory) | Vector index and retrieval backend for Gloss |
| [llm-pipeline](https://github.com/RecursiveIntell/llm-pipeline) | LLM provider abstraction (Ollama, OpenAI, Anthropic, LlamaCpp) |
| [tauri-queue](https://github.com/RecursiveIntell/tauri-queue) | Background job queue for Tauri applications |

---

## License

Gloss is licensed under **AGPL-3.0-only**. See [LICENSE](./LICENSE) for details.