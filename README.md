# Gloss

A local-first, privacy-preserving alternative to Google's NotebookLM. Add documents to notebooks, build a queryable knowledge base, and have grounded conversations with RAG-powered chat — all using local LLM inference via Ollama.

*gloss (n.) — a brief explanatory note or translation of a difficult word or passage.*

## Features

- **Notebook Management** — Create, rename, and delete isolated notebooks, each with its own SQLite database and vector index
- **Source Ingestion** — Import text files, Markdown, code files, folders, or paste text directly. Code-aware chunking detects Rust, TypeScript, and Python boundaries
- **Hybrid RAG Search** — Three-tier retrieval: semantic search (HNSW) + keyword search (FTS5 BM25) with reciprocal rank fusion, chunk DB fallback, and raw content fallback
- **Streaming Chat** — Real-time token streaming from Ollama with source-grounded responses and a source manifest in the system prompt
- **Automatic Summaries** — Background summary generation per source via a job queue with pause/resume controls
- **Scheduling Arbitration** — Single-flight GPU gate ensures chat and summaries never compete for inference. 15-second grace window prioritizes interactive chat
- **Notes** — Create manual notes or save assistant responses for later reference
- **Settings** — Configure Ollama endpoint, select default and summary models, and monitor connection status

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
| LLM Provider | Ollama |
| Job Queue | tauri-queue (custom scheduling loop) |
| LLM Calls | llm-pipeline (streaming, structured output) |

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
│  RAG Chat (hybrid search → context → LLM)  │
│  Summary Queue (tauri-queue, GPU-gated)     │
│  rusqlite │ usearch │ fastembed │ Ollama    │
└─────────────────────────────────────────────┘
```

## Prerequisites

- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) (v18+)
- [Ollama](https://ollama.ai/) running and accessible (default: `http://localhost:11434`)
- A chat model pulled in Ollama (e.g., `ollama pull qwen3:8b`)
- Tauri 2 system dependencies ([see Tauri docs](https://v2.tauri.app/start/prerequisites/))

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
│   ├── stores/             # Zustand state (notebook, source, chat, note, settings, studio)
│   └── lib/                # Types, Tauri bindings, events
├── src-tauri/              # Rust backend
│   └── src/
│       ├── commands/       # Tauri IPC commands (chat, notebooks, sources)
│       ├── db/             # SQLite schema, migrations, notebook DB
│       ├── ingestion/      # Chunking, embedding, summarization
│       ├── jobs/           # Job queue handler (GlossJob)
│       ├── providers/      # LLM provider abstraction (Ollama)
│       ├── retrieval/      # Hybrid search, context assembly
│       ├── studio/         # Studio output types (placeholder)
│       ├── state.rs        # AppState (Mutex-wrapped shared state)
│       └── lib.rs          # Tauri setup, summary job loop
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

Gloss is built in 4 phases. Phase 1 is complete.

- [x] **Phase 1** — Foundation: notebooks, text/code ingestion, hybrid RAG chat, streaming, notes, summaries, scheduling
- [ ] **Phase 2** — Rich ingestion (PDF, DOCX, URL, YouTube) + studio reports (briefing docs, FAQs, timelines) + OpenAI/Anthropic providers
- [ ] **Phase 3** — Interactive outputs (flashcards, quizzes, mind maps) + audio/video ingestion
- [ ] **Phase 4** — Audio overviews (TTS podcasts) + slide decks + infographics

See [SPEC-gloss.md](SPEC-gloss.md) for the full specification.

## License

MIT
