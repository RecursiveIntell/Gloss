# Gloss

A local-first, privacy-preserving alternative to Google's NotebookLM. Add documents to notebooks, build a queryable knowledge base, and have grounded conversations with RAG-powered chat — all running locally with your choice of LLM provider.

*gloss (n.) — a brief explanatory note or translation of a difficult word or passage.*

## Features

- **Notebook Management** — Create, rename, and delete isolated notebooks, each with its own SQLite database and vector index
- **Source Ingestion** — Import text files, Markdown, code files, images, videos, folders, or paste text directly. Code-aware chunking detects Rust, TypeScript, and Python boundaries. Hash-based deduplication prevents re-importing the same file
- **Vision & Video Pipeline** — Describe images and extract video frames via ffmpeg, then embed descriptions for RAG retrieval
- **Multi-Provider LLM Support** — Ollama, OpenAI, Anthropic, and llama.cpp backends with a unified provider trait and model registry
- **Hybrid RAG Search** — Three-tier retrieval: semantic search (HNSW) + keyword search (FTS5 BM25) with reciprocal rank fusion and cross-encoder reranking (BGE), chunk DB fallback, and raw content fallback
- **Citations** — Inline citation badges linking chat responses back to source documents
- **Streaming Chat** — Real-time token streaming with source-grounded responses and a source manifest in the system prompt
- **Automatic Summaries** — Background summary generation per source via a job queue with pause/resume controls
- **Scheduling Arbitration** — Single-flight GPU gate ensures chat and summaries never compete for inference. 15-second grace window prioritizes interactive chat
- **Notes** — Create manual notes or save assistant responses for later reference
- **Toast Notifications** — Non-blocking toast system for ingestion progress, errors, and status updates
- **Settings** — Configure multiple provider endpoints and API keys, select default/summary/vision models, check external tool availability (ffmpeg)

## Tech Stack

| Layer | Technology |
|-------|-----------|
| App Shell | Tauri 2 |
| Backend | Rust |
| Frontend | React 19, TypeScript, Vite 7 |
| Styling | Tailwind CSS 4 |
| State | Zustand 5 |
| Database | rusqlite (per-notebook SQLite + FTS5) |
| Vector Index | usearch (HNSW) |
| Embeddings | fastembed (NomicEmbedTextV15, 768-dim, CPU) |
| Reranking | fastembed TextRerank (BGE cross-encoder) |
| LLM Providers | Ollama, OpenAI, Anthropic, llama.cpp |
| Job Queue | tauri-queue (custom scheduling loop) |
| LLM Calls | llm-pipeline (streaming, structured output) |
| Video | ffmpeg/ffprobe (frame extraction) |

## Architecture

```
┌─────────────────────────────────────────────┐
│  Tauri 2 Shell                              │
│  ┌──────────┐ ┌──────────┐ ┌──────────────┐│
│  │ Sources  │ │  Chat    │ │ Notes/Studio ││
│  │ Panel    │ │  Panel   │ │ Panel        ││
│  └──────────┘ └──────────┘ └──────────────┘│
│  ┌─────────────────────────────────────────┐│
│  │ Status Bar: summaries, model, stats     ││
│  └─────────────────────────────────────────┘│
└──────────────────┬──────────────────────────┘
                   │ Tauri IPC
┌──────────────────┴──────────────────────────┐
│  Rust Backend                               │
│  Ingestion → Chunk → Embed → HNSW + FTS5   │
│  RAG Chat (hybrid search → rerank → LLM)   │
│  Summary Queue (tauri-queue, GPU-gated)     │
│  Vision/Video pipeline (ffmpeg frames)      │
│  rusqlite │ usearch │ fastembed │ providers │
└─────────────────────────────────────────────┘
```

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) (v18+)
- At least one LLM provider:
  - [Ollama](https://ollama.ai/) running locally (default: `http://localhost:11434`), or
  - An OpenAI / Anthropic API key, or
  - [llama.cpp](https://github.com/ggerganov/llama.cpp) server running locally
- A chat model available from your provider (e.g., `ollama pull qwen3:8b`)
- Tauri 2 system dependencies ([see Tauri docs](https://v2.tauri.app/start/prerequisites/))
- *(Optional)* [ffmpeg/ffprobe](https://ffmpeg.org/) for video source ingestion

## Getting Started

```bash
# Clone the repo
git clone https://github.com/RecursiveIntell/Gloss.git
cd Gloss

# Install frontend dependencies
npm install

# Run in development mode
npm run tauri dev
```

### Build for Production

```bash
npm run tauri build
```

The built application will be in `src-tauri/target/release/bundle/`.

## Project Structure

```
Gloss/
├── src/                    # React frontend
│   ├── components/
│   │   ├── chat/           # Chat panel, messages, input
│   │   ├── layout/         # Panel layout, status bar
│   │   ├── notebooks/      # Notebook sidebar, CRUD
│   │   ├── notes/          # Notes panel
│   │   ├── settings/       # Settings dialog
│   │   ├── sources/        # Source list, upload, detail
│   │   └── studio/         # Studio panel (placeholder)
│   ├── stores/             # Zustand state (notebook, source, chat, note, settings, toast, studio)
│   └── lib/                # Types, Tauri bindings, events
├── src-tauri/              # Rust backend
│   ├── src/
│   │   ├── commands/       # Tauri IPC commands (chat, notebooks, sources, settings)
│   │   ├── db/             # SQLite schema, migrations, notebook DB
│   │   ├── ingestion/      # Chunking, embedding, summarization, vision
│   │   ├── jobs/           # Job queue handler (GlossJob, image/video jobs)
│   │   ├── providers/      # LLM provider abstraction (Ollama, OpenAI, Anthropic, llama.cpp)
│   │   ├── retrieval/      # Hybrid search, reranking, context assembly, citations
│   │   ├── studio/         # Studio output types (placeholder)
│   │   ├── state.rs        # AppState (Mutex-wrapped shared state)
│   │   └── lib.rs          # Tauri setup, summary job loop
│   └── vendor/             # Vendored Rust dependencies
│       ├── llm-pipeline/   # LLM streaming & structured output
│       ├── tauri-queue/    # Tauri job queue integration
│       ├── job-queue/      # Background job processing
│       ├── llm-output-parser/ # LLM response parser
│       └── stack-ids/      # Shared identity primitives
├── SPEC-gloss.md           # Full product specification
└── CLAUDE.md               # Development contracts and rules
```

## Development

```bash
# Backend checks
cd src-tauri && cargo clippy -- -D warnings && cargo test

# Frontend type check + build
npm run build
```

21 Rust unit tests cover chunking, migrations, and code-aware boundary detection. `cargo clippy` runs with zero warnings and `npm run build` produces zero errors.

## Roadmap

Gloss is built in 4 phases. Phases 1–2 are complete.

- [x] **Phase 1** — Foundation: notebooks, text/code ingestion, hybrid RAG chat, streaming, notes, summaries, scheduling
- [x] **Phase 2** — Multi-provider support (Ollama, OpenAI, Anthropic, llama.cpp), vision/video pipeline, cross-encoder reranking, citations, toast notifications, vendored dependencies
- [ ] **Phase 3** — Rich ingestion (PDF, DOCX, URL, YouTube) + studio reports (briefing docs, FAQs, timelines) + interactive outputs (flashcards, quizzes, mind maps)
- [ ] **Phase 4** — Audio overviews (TTS podcasts) + slide decks + infographics

See [SPEC-gloss.md](SPEC-gloss.md) for the full specification.

## License

MIT
