# Gloss

Gloss is a local-first Tauri desktop app for building notebook-scoped knowledge bases and chatting over them. The code in this repo currently supports notebooks, source ingestion, grounded chat, notes, provider configuration, and background summary / vision jobs.

This README is intentionally about the code that exists in `src/` and `src-tauri/` today. `SPEC-gloss.md` and the audit / finish-line docs in the repo root describe a broader target surface that is not fully shipped yet.

## Current State

- Shipped now:
  - Notebook create / rename / delete
  - Source ingestion from files, folders, and pasted text
  - Notebook-scoped source selection
  - Streaming chat with citations and source viewer
  - Manual notes plus "save response to notes"
  - Background source summaries with pause / resume and queue status
  - Image and video description jobs
  - Chat providers: Ollama, OpenAI, Anthropic, llama.cpp
- Not shipped yet:
  - PDF, DOCX, spreadsheet, URL, YouTube, and audio ingestion
  - Studio outputs and report-generation UI
  - Audio overviews / podcast generation
  - Shared notebooks / sync / mobile
- Important caveats:
  - Interactive chat can use Ollama, OpenAI, Anthropic, or llama.cpp.
  - Background summaries and image / video jobs still run through the Ollama-compatible queue worker. Keep Ollama reachable if you want those background features.
  - Native semantic indexing is currently disabled in `src-tauri/src/state.rs`, so chat currently relies on chunk / FTS / raw-content fallback retrieval rather than live HNSW semantic search.
  - Gloss enforces single-flight inference. Chat and background jobs do not run concurrently.

If you are comparing the app to the broader roadmap, start with `AUDIT_BASIS_AND_FINDINGS.md`, `FINISHLINE_SPEC.md`, and `IMPLEMENTATION_PLAN.md`.

## What Gloss Does Today

- Creates isolated notebooks. Each notebook gets its own SQLite database plus its own `sources/`, `embeddings/`, `audio/`, and `exports/` directories.
- Imports single files, whole folders, or pasted text. Folder imports run in the background, preserve relative paths as titles, and dedupe files by SHA-256 within a notebook.
- Lets users scope chat to all sources or only a selected subset.
- Streams assistant responses into the UI and turns `[1]`, `[2]`, etc. citations into clickable source badges.
- Opens cited source text in a modal viewer so users can inspect the stored content behind a citation.
- Generates per-source summaries in the background and exposes queue status, pause / resume, and "generate missing summaries" controls in the status bar.
- Saves assistant responses as pinned notes or creates / edits manual notes.
- Configures provider URLs, API keys, chat model, summary model, vision model, and external tool availability from Settings.

## Supported Sources

### Text, Markdown, and Code

Gloss currently imports:

- Plain text and markdown: `txt`, `md`, `markdown`, `rst`
- Code and config: `py`, `js`, `jsx`, `ts`, `tsx`, `rs`, `go`, `java`, `c`, `cpp`, `cc`, `cxx`, `h`, `hpp`, `cs`, `rb`, `php`, `swift`, `kt`, `kts`, `scala`, `lua`, `r`, `sql`, `sh`, `bash`, `zsh`, `css`, `scss`, `sass`, `html`, `htm`, `xml`, `json`, `yaml`, `yml`, `toml`, `ini`, `cfg`, `conf`, `vue`, `svelte`, `dart`, `ex`, `exs`, `zig`, `nim`, `pl`, `pm`, `proto`, `graphql`, `gql`, `tf`, `hcl`
- Extensionless files are allowed, which covers cases like `Dockerfile`, `Makefile`, and `LICENSE`
- `svg` is currently treated as text / XML, not as an image source

### Images and Video

- Images: `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `tiff`, `tif`
- Video: `mp4`, `webm`, `mov`, `avi`, `mkv`

Image and video files are not OCRed or transcribed in the classic sense. Instead:

- Images are sent to the configured vision model and stored as generated descriptions.
- Videos are sampled into frames with `ffmpeg`, then each frame is described by the vision model and combined into notebook content.
- Both pipelines store generated text in the notebook database so chat can retrieve it later.

### Current Import Limits and Folder Rules

- Max text / code / markdown / image file size: 10 MB
- Max video file size: 100 MB
- Folder imports recurse to a maximum depth of 20
- Folder imports stop after 5,000 supported files
- Hidden files, symlinks, and common junk / build directories are skipped
- The walker explicitly skips directories such as `node_modules`, `target`, `.git`, `dist`, `build`, `vendor`, `.venv`, `venv`, `.next`, `.nuxt`, and `.cache`
- Binary archives, office docs, compiled artifacts, audio files, and lockfiles are rejected today

## Architecture

### Stack

- Desktop shell: Tauri 2
- Frontend: React 19, TypeScript, Vite 7, Zustand, Tailwind CSS 4
- Backend: Rust
- App storage: SQLite via `rusqlite`
- Queueing: vendored `tauri-queue`
- LLM calls / streaming helpers: vendored `llm-pipeline`
- Retrieval primitives: SQLite FTS5 plus in-tree HNSW / fastembed code paths
- Providers: Ollama, OpenAI, Anthropic, llama.cpp

### Runtime Behavior That Matters

These are not just design notes; the current code actively depends on them:

- `llm_gate` and `gpu_gate` enforce single-flight inference across chat and background jobs.
- No summaries run until a notebook has been explicitly selected.
- Sending a chat message opens a 15 second grace window that blocks new summary work.
- Notebook switches advance an epoch so stale background jobs and stale chat streams get filtered or cancelled.
- Provider failures are emitted as separate UI error state (`chat:error`) rather than being inserted into assistant text.
- Notebook-scoped frontend state is reset on notebook change before new data loads.

### Retrieval Reality

The repo contains code for:

- chunking
- FTS5 keyword search
- usearch HNSW indices
- fastembed embeddings
- cross-encoder reranking

But the current runtime flag `NATIVE_SEMANTIC_INDEXING_ENABLED` is `false`, so the live app does not currently run the semantic HNSW path. Chat currently falls back to:

1. stored chunks from SQLite
2. raw `content_text` when chunks are unavailable

That distinction matters when you are evaluating search behavior or planning follow-up work.

## Development

### Prerequisites

- Node.js 22 is what CI uses today
- `npm`
- Rust stable
- Tauri 2 system dependencies
- At least one working provider, with Ollama strongly recommended for the current feature set
- `ffmpeg` and `ffprobe` if you want video ingestion

On Ubuntu, the current CI workflow installs:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  curl \
  file \
  libayatana-appindicator3-dev \
  librsvg2-dev \
  libssl-dev \
  libwebkit2gtk-4.1-dev \
  libxdo-dev \
  pkg-config \
  wget
```

### Run Locally

```bash
npm ci
npm run tauri -- dev
```

Recommended first-run flow:

1. Create a notebook.
2. Open Settings.
3. Configure your provider URL and any API keys you need.
4. Click `Refresh` in the Models section.
5. Pick a chat model, and optionally separate summary and vision models.

### Useful Commands

```bash
npm run build
cd src-tauri && cargo test
npm run tauri -- build
```

Notes:

- The current Linux bundle target in `src-tauri/tauri.conf.json` is `rpm`.
- The checked-in CI workflow runs `npm run build`, a Rust format check, and `cargo test`.

## Storage and Data Layout

Gloss resolves its app-data directory with `directories::ProjectDirs("com", "sikmindz", "Gloss")`.

At a high level the runtime data looks like this:

```text
<app-data-dir>/
├── gloss.db
├── queue.db
├── models/
├── secrets/
│   ├── secret-store.enc
│   └── secret-store.key
└── notebooks/
    └── <notebook-id>/
        ├── notebook.db
        ├── sources/
        ├── embeddings/
        ├── audio/
        └── exports/
```

- `gloss.db` stores notebook registry data, provider URLs, cached model lists, and app settings.
- Each notebook has its own `notebook.db` with sources, chunks, conversations, messages, notes, and notebook config.
- Imported files are copied into the notebook's `sources/` directory.
- API keys are not kept in SQLite. They are encrypted into `secrets/secret-store.enc` with a local AES-GCM key stored beside it.

## Repo Guide

The important parts of this repo are:

- `src/` - React UI
- `src-tauri/src/` - Rust backend
- `src-tauri/vendor/` - vendored Rust libraries used by the app
- `.github/workflows/ci.yml` - current CI contract
- `SPEC-gloss.md` - broader product spec / target surface
- `AUDIT_BASIS_AND_FINDINGS.md`
- `FEATURE_COVERAGE_MATRIX.md`
- `FINISHLINE_SPEC.md`
- `IMPLEMENTATION_PLAN.md`
- `RELEASE_GATES.md`
- `MASTER_ISSUE_MATRIX.md`

The repo root also contains archive snapshots from recent audit work. The live application code is in `src/` and `src-tauri/`.

## Contributor Notes

If you are changing scheduler, chat, or notebook-switch behavior, read `AGENTS.md` first. The current codebase treats these as non-negotiable invariants:

- single-flight inference
- chat preemption plus the 15 second grace window
- no summaries before notebook selection
- notebook switch correctness via active notebook + epoch checks
- provider errors as separate UI state
- notebook-scoped frontend reset on notebook change
