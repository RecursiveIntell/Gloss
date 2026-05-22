# Gloss

Gloss is a local-first Tauri + React desktop notebook for source ingestion, local retrieval, citation-grounded chat, notes, source summaries, and media-aware research workflows.

The stable release path keeps notebook data and indexes on the local machine. Hosted providers are optional and only used after the user configures them.

## Current Release State

Active run: `GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519`

Run truth is:

- `docs/codex-runs/CURRENT_RUN.md`
- `docs/codex-runs/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/FINAL_RECEIPT.json`

The current P33 receipt says the repository is not release-ready. The release decision is blocked by missing full desktop RAG smoke proof and missing fresh-unzip/package replay proof. Provider-only smoke is diagnostic evidence, not full RAG proof.

## Product Surface

- Notebooks: create, select, rename, delete, and persist the active notebook.
- Sources: import files, folders, drag-and-drop files, and pasted text.
- Source scope: select all, no sources, or explicit source subsets for each chat turn.
- Chat: streamed assistant responses with stop, regenerate, copy, edit-and-rerun, suggested questions, status events, and evidence drawers.
- Citations: assistant answers can carry validated source citations and retrieval evidence.
- Notes: create manual notes and save assistant responses with citation backlinks.
- Summaries: queue-driven source summaries with manual or idle auto mode.
- Health/status: provider reachability, selected model state, queue state, runtime gate owners, memory backend state, and semantic-memory link health.
- Settings: provider URLs/API keys, model refresh, default chat/summary/vision models, feature flags, memory backend selection, semantic-memory settings, and external tool checks.

## Local-First Behavior

Gloss stores app state under the OS application data directory for `com.sikmindz.Gloss`.

The backend initializes:

- `gloss.db` for app-level notebooks, providers, models, and settings.
- `notebooks/<notebook-id>/notebook.db` for each notebook.
- Per-notebook copied source files and generated embedding/index files.
- `queue.db` for persistent background jobs.
- A local encrypted secret store for provider API keys.

Local-first defaults:

- Default provider: Ollama at `http://localhost:11434`.
- Default model setting: `qwen3:8b`.
- Default memory backend: `gloss-local`.
- Summary mode: `manual`.
- Experimental features: off.
- semantic-memory preview: off.
- TurboQuant candidates: off.

OpenAI and Anthropic keys are stored through the local secret store, not left in the provider table. Legacy keys are migrated out of SQLite settings/provider rows at startup.

## Architecture

Frontend:

- React 19, TypeScript, Vite, Tailwind CSS 4.
- Zustand stores under `src/stores/`.
- Tauri command wrappers in `src/lib/tauri.ts`.
- Tauri event listeners in `src/lib/events.ts`.
- UI panels under `src/components/`.

Backend:

- Tauri 2 app entry and queue loop in `src-tauri/src/lib.rs`.
- Global runtime state in `src-tauri/src/state.rs`.
- Tauri commands in `src-tauri/src/commands/`.
- SQLite schema/migrations in `src-tauri/src/db/`.
- Ingestion in `src-tauri/src/ingestion/`.
- Persistent jobs in `src-tauri/src/jobs/`.
- Providers in `src-tauri/src/providers/`.
- Retrieval and citations in `src-tauri/src/retrieval/`.
- Memory backends in `src-tauri/src/memory/`.
- Feature governance in `src-tauri/src/features.rs`.

Vendored Rust crates live under `src-tauri/vendor/`. The large generated `src-tauri/vendor/crates/` cache is ignored.

## Runtime Scheduling

Gloss uses explicit runtime gates to avoid local model contention:

- `llm_gate`: single-flight chat/summary/studio inference.
- `gpu_gate`: prevents simultaneous embedding and LLM GPU pressure.
- Chat preempts background work by bumping a grace window and cancelling processing jobs.
- Summary jobs wait for active notebook selection, honor pause/manual mode, defer during ingestion, and validate notebook epoch before completing stale work.
- Runtime gate owners are exposed in the status bar and chat status events.

## Source Ingestion

Supported source paths:

- Text and markdown: `txt`, `md`, `markdown`, `rst`.
- Code/config: common source files including TypeScript, JavaScript, Rust, Go, Java, C/C++, Python, SQL, shell, JSON, YAML, TOML, HTML, CSS, Terraform, GraphQL, Dockerfile, Makefile, and related extensions.
- Images: `png`, `jpg`, `jpeg`, `gif`, `webp`, `bmp`, `tiff`, `tif`.
- Video: `mp4`, `webm`, `mov`, `avi`, `mkv`.
- Paste: text entered directly in the UI.

Binary/archive/model/database files are skipped, including archives, object files, lock files, local DB files, ONNX/model weights, audio, office documents, and unsupported binaries.

Text/code ingestion extracts content, chunks it, stores chunks in SQLite, and may create embeddings/HNSW labels when native indexing is enabled. Current runtime constant `NATIVE_SEMANTIC_INDEXING_ENABLED` is `false`, so stable retrieval does not claim dense hybrid unless dense actually runs.

Image/video sources use the vision/background job path when configured, then finalize into text content before normal chunking/retrieval.

## Retrieval And Evidence

The stable retrieval path is `gloss-local`:

- SQLite FTS5/BM25 is the stable local retriever.
- Native dense HNSW is optional and currently disabled at runtime.
- Retrieval outcomes report which engines were attempted, available, and contributed.
- Degraded states and fallback reasons are recorded instead of silently widening or claiming hybrid retrieval.
- Source scope is resolved before retrieval. Invalid explicit scopes resolve to none or to only valid requested sources; they do not widen to all sources.

Chat attempts emit and persist `ChatAttemptTraceV1` evidence. Assistant evidence includes:

- Backend requested and backend used.
- Retrieval mode and fallback reason.
- Source scope mode, requested/effective/invalid/excluded source IDs.
- Context passage count.
- Citation valid/invalid counts.
- Retrieval outcome receipt/reference.
- semantic-memory/TurboQuant receipt fields when that preview path is active.

## semantic-memory And TurboQuant

semantic-memory preview and TurboQuant are experimental surfaces.

Activation requires:

- Rust build feature availability.
- Global Experimental Features switch enabled.
- Individual feature flag enabled.
- Runtime setting selecting `semantic-memory-preview` where applicable.

Build feature availability is not runtime consent. TurboQuant is candidate-only and exact rerank remains required. Turning off Experimental Features resets the runtime memory backend back to `gloss-local`.

Rust feature flags:

- `semantic-memory-backend`
- `semantic-memory-turbo-quant`

## Providers And Models

Supported providers:

- Ollama
- llama.cpp-compatible local server
- OpenAI
- Anthropic

Provider URLs are stored in the provider table. OpenAI and Anthropic API keys are stored in the local secret store. Model refresh writes provider/model availability, stale state, context window, capabilities, and last error into `gloss.db`.

Settings currently separate chat, background summary, and vision model choices. Background summary and vision jobs expect Ollama-backed local models.

## Development Setup

Prerequisites:

- Node.js and npm
- Rust toolchain with Cargo
- Tauri 2 system dependencies for your OS
- Ollama or another configured provider for runtime chat
- Optional `ffmpeg`/`ffprobe` for video import paths
- Optional WebDriver tooling for desktop smoke: `tauri-driver` and WebKitWebDriver

Install dependencies:

```bash
npm ci
```

Run the Vite frontend:

```bash
npm run dev
```

Run the Tauri desktop app:

```bash
npm run tauri dev
```

Build the frontend:

```bash
npm run build
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

## Validation

Release-candidate validation commands:

```bash
python3 scripts/p33_release_preflight.py --repo . --run-id GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519
python3 scripts/p33_current_run_gate.py --repo . --run-id GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519
python3 scripts/p33_sm_tq_settings_gate.py --repo .
python3 scripts/p33_gui_asset_gate.py --repo .
npm ci
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
python3 scripts/p33_desktop_smoke_gate.py --repo . --receipt docs/codex-runs/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/desktop_smoke/final_desktop_smoke.json
bash scripts/p33_package_replay_gate.sh .
python3 scripts/p33_release_final_gate.py --repo . --run-id GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519
```

Additional active static gates:

```bash
python3 scripts/check_gloss_active_validation_scope.py --repo .
python3 scripts/check_feature_flags_static.py --repo .
python3 scripts/check_release_eligibility_current.py --repo .
python3 scripts/gloss_button_up_gate.py --repo .
python3 scripts/gloss_retrieval_gate.py --repo .
```

Known current blockers:

- Full desktop RAG smoke is blocked unless `tauri-driver` and WebKitWebDriver are installed and a full desktop trace/citation proof is produced.
- Fresh-unzip/package replay evidence is absent until an archive is selected, extracted, and replayed successfully.

## Git Hygiene

The repository is configured to ignore local/generated development artifacts:

- `node_modules/`, `dist/`, `src-tauri/target/`
- logs, Python caches, temp files, local DB files
- local assistant/workbench folders such as `.claude/`, `.codex/`, `.codex-run-receipts/`, `codex/`, `p33boot/`, `reference/`, `drop_into_repo/`
- generated source/archive sidecars such as `*.zip`, `*.codex-archive.json`, `*.excluded.json`, `*.findings.json`, `*.manifest.json`, `*.report.md`
- historical run folders under `docs/codex-runs/`
- GUI design reference material under `docs/design/`

The active P33 run folder is explicitly unignored because it is current release evidence. Historical run folders remain ignored evidence unless intentionally restored.

Before pushing, inspect:

```bash
git status --short --untracked-files=all
git status --ignored --short
```

Only source, docs, validation scripts, active run receipts, and production assets should remain visible as tracked or intentionally untracked files.

## Push Checklist

1. Review `git status --short --untracked-files=all`.
2. Confirm no local secrets, databases, archives, node modules, build output, or assistant workbench files are visible for commit.
3. Run the validation commands relevant to the change.
4. Keep `release_ready=false` until desktop RAG smoke and fresh-unzip/package replay pass and receipts are updated.
5. Commit only intentional source/docs/receipt changes.

## Rollback

Use the active run receipt and changed-file list as rollback truth. For P33 runtime behavior, the high-risk rollback set is:

- `src-tauri/src/features.rs`
- `src-tauri/src/memory/semantic_memory_adapter.rs`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/commands/sources.rs`
- `src/components/settings/SettingsDialog.tsx`

Do not flip release readiness to true without passing the final gates and updating receipts.
