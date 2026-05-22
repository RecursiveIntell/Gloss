# Gloss

Gloss is a local-first desktop research notebook for people who work with dense source material and want AI help without losing control of the evidence.

It combines notebooks, source ingestion, retrieval-augmented chat, citations, notes, summaries, media handling, provider configuration, and runtime diagnostics in one Tauri desktop app. The design goal is simple: keep your research workspace on your machine, let you choose the model backend, and make every answer accountable to the sources that produced it.

## Why Gloss

Most AI chat tools treat your documents as disposable context. Gloss treats them as a durable research library.

- **Local-first by default**: notebooks, source copies, SQLite databases, retrieval indexes, settings, and provider metadata live on the local machine.
- **Citation-grounded chat**: assistant answers can carry source citations and retrieval evidence, so you can inspect what context was used instead of trusting a black box.
- **Explicit source scope**: ask over all sources, no sources, or a selected subset when a question needs a tight evidence boundary.
- **Notebook-native workflow**: collect sources, chat with them, save useful responses as notes, pin important notes, and keep related conversations together.
- **Model choice without lock-in**: run with local providers like Ollama or llama.cpp-compatible servers, or configure hosted providers when you decide they are appropriate.
- **Truthful retrieval status**: Gloss reports fallback, degraded retrieval, source scope, backend selection, and citation health instead of claiming dense hybrid search when dense retrieval did not run.
- **Media-aware ingestion**: import text, code, images, pasted text, folders, and supported video workflows with local tool checks.
- **Experimental acceleration path**: semantic-memory and TurboQuant candidate generation are available as guarded preview surfaces while the stable app remains conservative and exact-rerank oriented.

## Core Experience

### Notebooks

Create separate notebooks for projects, clients, papers, codebases, classes, or investigations. Each notebook gets its own local data directory, source store, per-notebook SQLite database, conversations, notes, chunks, summaries, and retrieval state.

### Sources

Gloss can ingest:

- Text and Markdown: `txt`, `md`, `markdown`, `rst`
- Code and config: TypeScript, JavaScript, Rust, Go, Java, C/C++, Python, SQL, shell, JSON, YAML, TOML, HTML, CSS, Terraform, GraphQL, Dockerfile, Makefile, and related formats
- Images: `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `tiff`, `tif`
- Video: `mp4`, `webm`, `mov`, `avi`, `mkv`
- Pasted text entered directly in the app
- Whole folders through recursive import

Unsupported binaries, archives, model weights, local databases, object files, office documents, and noisy generated artifacts are skipped so the notebook stays focused on usable research material.

### Chat With Evidence

Gloss chat is built around source accountability.

- Streamed assistant responses
- Stop and regenerate controls
- Copy, edit-and-rerun, and save-as-note workflows
- Suggested questions from the current notebook
- Source citations attached to answers
- Evidence drawers showing retrieval backend, retrieval mode, source scope, fallback state, context counts, citation validity, and receipt references
- Chat attempt traces for diagnosing provider, retrieval, streaming, and persistence behavior

### Source Scope

Source scope is a first-class control, not an afterthought.

- **All sources**: use the whole notebook
- **Selected sources**: constrain chat to specific files or pasted sources
- **No sources**: use the model directly when retrieval would be a distraction

Gloss resolves source scope before retrieval and does not silently widen invalid explicit selections into a broader search.

### Notes And Summaries

Gloss supports both manual knowledge capture and background synthesis.

- Create and edit manual notes
- Pin important notes
- Save assistant responses as notes with citation backlinks
- Generate source summaries
- Queue summary work without blocking chat
- Pause or resume summary processing

### Providers And Models

Gloss is provider-flexible.

- Ollama
- llama.cpp-compatible local servers
- OpenAI
- Anthropic

Provider URLs, enabled state, model refresh results, selected chat model, summary model, vision model, context window, capabilities, and provider errors are tracked locally. Hosted API keys are stored through the local secret store instead of being left in ordinary settings rows.

### Status You Can Trust

The UI exposes the runtime details that matter during real work:

- Provider reachability
- Selected model state
- Queue state
- Summary pause/manual mode
- Runtime gate owners
- Memory backend selection
- semantic-memory link health
- External tool availability
- Retrieval coverage and fallback state

Gloss favors visible degradation over hidden optimism.

## Local-First Architecture

Gloss is built as a Tauri 2 desktop app with a React/TypeScript frontend and Rust backend.

Frontend:

- React 19
- TypeScript
- Vite
- Tailwind CSS
- Zustand stores
- Tauri command and event wrappers

Backend:

- Rust and Tauri commands
- SQLite app database
- Per-notebook SQLite databases
- Persistent job queue
- Local source storage
- Local secret store for provider keys
- Provider registry
- Retrieval and citation pipeline
- Runtime gates for LLM and GPU-sensitive work

By default, Gloss initializes local app data for notebooks, providers, settings, queue state, copied sources, chunks, generated summaries, and index artifacts under the operating system's application data directory.

## Retrieval

The stable retrieval backend is `gloss-local`.

Gloss uses SQLite FTS5/BM25 as the stable local retriever and records exactly which retrieval engines were attempted, available, and productive. Optional dense and semantic-memory paths are treated as explicit runtime capabilities, not marketing labels.

When retrieval cannot produce indexed context, Gloss records the degraded outcome and fallback chain. When citations are attached to an answer, the evidence payload records how the context was built and what source scope was effective.

## semantic-memory And TurboQuant

Gloss includes guarded experimental integration points for `semantic-memory` and TurboQuant candidate acceleration.

The important rule: preview acceleration is never treated as correctness. TurboQuant is candidate-only in Gloss, and exact rerank remains required. The stable local path remains the default, and build-feature availability does not imply runtime consent.

The TurboQuant-related work in Gloss is connected to the [`RecursiveIntell/turbo-quant`](https://github.com/RecursiveIntell/turbo-quant) Rust crate, an experimental sidecar-oriented implementation inspired by TurboQuant, PolarQuant, and Quantized Johnson-Lindenstrauss sketches. The crate is also published on [docs.rs](https://docs.rs/turbo-quant/latest/turbo_quant/).

External references I verified while preparing this README:

- The [`tonbistudio/turboquant-pytorch`](https://github.com/tonbistudio/turboquant-pytorch) README lists `RecursiveIntell/turbo-quant` as a Rust community implementation.
- A [Dev.to developer overview](https://dev.to/arshtechpro/turboquant-what-developers-need-to-know-about-googles-kv-cache-compression-eeg) lists `RecursiveIntell/turbo-quant` as a standalone Rust implementation for embeddings and KV cache work.
- [DevPik coverage](https://www.devpik.com/blog/google-turboquant-pied-piper-ai-compression) also identifies `RecursiveIntell/turbo-quant` as a Rust implementation and describes it as useful for vector-search applications.
- [ByteIota coverage](https://byteiota.com/google-turboquant-6x-ai-memory-compression-tanks-chip-stocks/) mentions Rust community implementation work alongside PyTorch and vLLM efforts.
- The TurboQuant research paper is available through [OpenReview](https://openreview.net/pdf/7d33913c9a4f47c8abb294d6beb85d30124747ca.pdf).

Those references are not a claim that Gloss enables TurboQuant by default. In Gloss, TurboQuant remains experimental, opt-in, and subordinate to exact retrieval correctness.

## What Makes Gloss Different

### It is a notebook, not just a chat box

Research work is not one prompt. Gloss gives you a place to build context over time: sources, conversations, saved answers, summaries, and notes all live together.

### It is local-first without being local-only

Gloss starts from local storage and local model workflows, but it does not force a single provider. You can keep sensitive notebooks local and still configure hosted providers for cases where they make sense.

### It treats citations as a runtime contract

Gloss does not just append citation-looking text. It stores citation payloads and retrieval evidence so the app can show what backend ran, what scope was searched, and how the answer was grounded.

### It is honest about retrieval quality

Gloss distinguishes BM25, dense, semantic-memory, source-order fallback, degraded states, missing embeddings, invalid source selections, and unavailable preview features. That matters when the answer is only as good as the context path.

### It is built for long-running local workflows

Background jobs, summary queues, ingestion, provider calls, embedding work, and chat compete for local resources. Gloss uses runtime gates and visible status so foreground chat remains responsive and background work does not quietly starve the app.

## Development Setup

Prerequisites:

- Node.js and npm
- Rust toolchain with Cargo
- Tauri 2 system dependencies for your OS
- Ollama or another configured provider for runtime chat
- Optional `ffmpeg` and `ffprobe` for video import paths
- Optional `tauri-driver` and WebKitWebDriver for desktop smoke validation

Install dependencies:

```bash
npm ci
```

Run the frontend:

```bash
npm run dev
```

Run the desktop app:

```bash
npm run tauri dev
```

Build the frontend:

```bash
npm run build
```

Build the Tauri app:

```bash
npm run tauri build
```

Run Rust tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml
```

Run feature-specific Rust tests:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

Run the desktop smoke harness when WebDriver tooling and a local Ollama model are available:

```bash
npm run desktop-smoke
```

## Repository Layout

```text
src/                  React frontend
src/components/       Notebook, source, chat, notes, settings, and layout UI
src/lib/              Tauri command wrappers, events, and feature helpers
src/stores/           Zustand state stores
src-tauri/            Rust/Tauri backend
src-tauri/src/db/     App and notebook SQLite layers
src-tauri/src/jobs/   Persistent background job handling
src-tauri/src/memory/ Memory backend and semantic-memory adapter
src-tauri/src/providers/
                      Provider registry and model handling
src-tauri/src/retrieval/
                      Local retrieval, source scope, citations, and outcomes
scripts/              Validation, packaging, smoke, and repository hygiene tools
docs/                 Runbooks, design references, and release evidence
```

## License

Gloss is licensed under `AGPL-3.0-only`.
