# Gloss

Gloss is a local-first desktop notebook for asking questions about your own sources. It is built as a Tauri app with a Rust backend and a React/TypeScript frontend. The primary model path is local Ollama, with provider records for OpenAI, Anthropic, and llama.cpp also present in the app.

Gloss keeps notebook data on the machine, stores source files per notebook, indexes text into SQLite/FTS5 and vector search, then streams grounded chat answers with citations and diagnostic traces.

## Project Status

The app is an active local desktop project. The latest automated validation in this repo passes for the frontend build, Rust tests, chat-runtime static checks, and optional semantic-memory feature builds. A manual desktop smoke checklist is included for proving the interactive Tauri UI path on a real desktop session.

Current defaults:

- Local memory backend: `gloss-local`
- Primary local provider: Ollama at `http://localhost:11434`
- Optional preview backend: `semantic-memory-preview`
- Optional TurboQuant path: candidate acceleration only, exact-reranked

## Features

- Notebook CRUD with per-notebook storage.
- File, folder, and paste imports.
- Source selection for scoped chat.
- Local text chunking with code-aware splitting for common programming languages.
- SQLite notebook databases with FTS5 search.
- Vector search through `usearch`.
- CPU embedding through `fastembed`.
- RAG chat with streamed tokens, citations, status events, errors, and stop support.
- Durable `ChatAttemptTraceV1` diagnostics for chat attempts.
- Conversation history and message persistence.
- Suggested questions for notebooks.
- Notes, pinned notes, and saving assistant responses as notes.
- Background summary jobs with pause/resume controls.
- Provider configuration, provider health checks, and model refresh.
- Image description jobs for vision-capable models.
- Video frame-description jobs when `ffmpeg`/`ffprobe` are available.
- Optional semantic-memory preview indexing, link status, and reindex commands.

## Architecture

```text
src/                         React 19 + TypeScript frontend
src/components/              Notebook, source, chat, note, settings, status UI
src/stores/                  Zustand state stores
src/lib/tauri.ts             Tauri invoke wrapper

src-tauri/                   Rust backend and Tauri app
src-tauri/src/commands/      Notebook, source, chat, notes, settings commands
src-tauri/src/db/            App DB and per-notebook DB layers
src-tauri/src/ingestion/     Extraction, chunking, embeddings, summaries, vision
src-tauri/src/memory/        gloss-local and semantic-memory preview adapters
src-tauri/src/providers/     Ollama, OpenAI, Anthropic, llama.cpp providers
src-tauri/src/retrieval/     Search, source scope, context assembly, citations
src-tauri/vendor/            Vendored path crates needed by Cargo

scripts/                     Validation, smoke, packaging, and audit helpers
fixtures/                    Test and audit fixtures
schemas/                     Receipt and validation schemas
docs/                        Runtime evidence and smoke checklists
```

## Data Model

Gloss uses an app-level SQLite database plus one SQLite database per notebook.

The app data directory is created through `directories::ProjectDirs::from("com", "sikmindz", "Gloss")`. On Linux this normally resolves under the user data directory, for example:

```text
~/.local/share/com.sikmindz.Gloss/
  gloss.db
  notebooks/
    <notebook-id>/
      notebook.db
      sources/
      embeddings/
      audio/
```

Source files are copied into the notebook directory. Chat conversations, messages, chunks, source metadata, notes, and semantic-memory link rows live in the notebook database.

## Supported Source Imports

Text-oriented sources:

- Plain text: `txt`
- Markdown/reStructuredText: `md`, `markdown`, `rst`
- Paste sources entered through the UI
- Code and config files, including Rust, Python, JavaScript, TypeScript, Go, Java, C/C++, C#, Ruby, PHP, Swift, Kotlin, Scala, Lua, R, SQL, shell, CSS, HTML, XML, JSON, YAML, TOML, Terraform/HCL, Dockerfile, Makefile, GraphQL, protobuf, and related formats
- Files without extensions, such as `LICENSE`, `Makefile`, and similar text files

Media sources:

- Images: `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `tiff`, `tif`
- Videos: `mp4`, `webm`, `mov`, `avi`, `mkv`

Known import limits:

- Audio files are skipped.
- PDF, DOC/DOCX, XLS/XLSX, and PPT/PPTX files are currently skipped by the importer.
- Archives, binaries, model files, local databases, lockfiles, and generated indexes are skipped.
- Images and videos need a configured vision model to become useful text context. Videos also need `ffmpeg` and `ffprobe`.

## Providers And Models

Gloss has provider records for:

- Ollama
- OpenAI
- Anthropic
- llama.cpp-compatible local servers

Provider rows are the source of truth for base URLs and enabled state. API keys for cloud providers are stored through the secret store instead of the app database.

Default provider URLs:

```text
Ollama:    http://localhost:11434
OpenAI:    https://api.openai.com/v1
Anthropic: https://api.anthropic.com/v1
llama.cpp: http://localhost:8080/v1
```

For local use, start Ollama and pull at least one chat model:

```bash
ollama serve
ollama pull cogito:3b
ollama pull qwen3.5:4b
```

Then open Settings, configure or test the provider, refresh models, and choose the model to use for chat. Vision and summary settings can be configured separately.

## Memory Backends

`gloss-local` is the default backend. It uses local notebook chunks, SQLite/FTS5, vector search, source-scope filtering, and citation validation.

`semantic-memory-preview` is optional. It is compiled with:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
```

TurboQuant support is compiled with:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

The semantic-memory crates required for these features are vendored under `src-tauri/vendor/`, so a clone of this repo does not need a sibling `Libraries/semantic-memory` checkout.

## Requirements

- Node.js 22 or newer
- npm
- Rust stable with `rustfmt`
- Ollama for local model use
- Linux Tauri/WebKit build dependencies when building on Linux
- `ffmpeg` and `ffprobe` for video source processing

Ubuntu/Debian build dependencies:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential curl file libayatana-appindicator3-dev librsvg2-dev \
  libssl-dev libwebkit2gtk-4.1-dev libxdo-dev pkg-config wget
```

## Install

```bash
npm ci
```

## Run

Run the Tauri desktop app:

```bash
npm run tauri dev
```

Run only the web frontend:

```bash
npm run dev
```

The frontend-only dev server is useful for UI work, but the real app behavior depends on the Tauri backend.

## Build

Build the frontend:

```bash
npm run build
```

Build a Tauri package:

```bash
npm run tauri build
```

The current Tauri bundle target is RPM on Linux.

## Validation

Core checks:

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
```

Chat runtime checks:

```bash
python3 scripts/chat_runtime_static_audit.py --repo .
python3 scripts/chat_runtime_preflight.py --repo .
```

Optional backend checks:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

Manual desktop smoke:

```bash
scripts/chat_runtime_smoke_manual.sh
```

The desktop smoke proves the actual UI path: prompt submission, streamed tokens or visible error, persisted assistant message or durable trace, provider settings, and semantic-memory fallback behavior.

## Chat Runtime Diagnostics

Every chat attempt should produce one of these observable outcomes:

- streamed assistant tokens
- visible provider or retrieval error
- visible timeout
- durable `ChatAttemptTraceV1`

Useful diagnostic entry points:

- `debug_chat_provider_smoke`
- `get_last_chat_attempt_trace`
- `docs/codex-runs/CHAT_RUNTIME_FIX_20260518/`
- `docs/CHAT_RUNTIME_SMOKE_CHECKLIST.md`

## Repository Hygiene

Tracked:

- Source code
- Lockfiles
- Validation scripts
- Schemas and fixtures
- Current chat-runtime evidence
- Curated vendored path crates needed to build optional features

Ignored:

- `node_modules/`
- `dist/`
- `src-tauri/target/`
- local app databases
- generated source archives and sidecars
- Python caches
- large generated Cargo vendor cache at `src-tauri/vendor/crates/`
- historical extracted run archives

## License

Gloss is licensed under `AGPL-3.0-only`.

Vendored crates under `src-tauri/vendor/` may carry their own licenses. Check each vendored crate directory for details.
