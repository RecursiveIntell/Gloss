# Gloss

Gloss is a local-first desktop notebook for research, source-grounded chat, notes, and retrieval experiments. It is built with Tauri, React, TypeScript, Rust, SQLite, local indexing, and optional semantic-memory integration.

The product direction is large: NotebookLM-style workspaces, scoped RAG answers, local-first data ownership, model-provider flexibility, semantic-memory projection, and TurboQuant-assisted retrieval. The current repository is not presented as release-ready. Current source files, active gates, and fresh receipts outrank old README claims and historical run artifacts.

## Current Truth

Gloss is in runtime-truth and ingestion repair. Do not ship, promote, or demo it as a completed release unless the current gates and live desktop receipts prove that claim.

- Operator pass for this work: `GLOSS_P34_RUNTIME_TRUTH_INGESTION_REPAIR_20260524`.
- `docs/codex-runs/CURRENT_RUN.md` currently reports `P30`, which conflicts with the P34 operator pass and must be resolved before any release claim.
- P34 and P35 receipts explicitly mark the app as not release-ready while live desktop GUI smoke remains unproven.
- `docs/CURRENT_FEATURE_MATRIX.md` is the product-surface source of truth for implemented, partial, degraded, deferred, and blocked capabilities.
- Every user-visible answer must disclose requested backend, effective backend, fallback/degradation, citation validity, source-scope integrity, and receipt identity.

## What Gloss Is

Gloss is designed for users who want to work with local notebooks and ask questions against their own sources without silently widening source scope or hiding retrieval failures.

Core surfaces:

- Notebook management with local per-notebook storage.
- Source import for text-like files, folders, and pasted content.
- Queue-backed ingestion, chunking, embedding, and summary jobs.
- Source-scoped chat with streaming provider responses.
- Citation and evidence envelopes attached to assistant answers.
- Notes created manually or from assistant responses.
- Provider settings for local and remote model backends.
- Runtime diagnostics for chat, retrieval, source scope, and memory backends.
- Optional semantic-memory preview and TurboQuant candidate acceleration.

Current feature boundaries:

- Text, Markdown, and code import are partial but active.
- BM25/local retrieval is the stable implemented fallback.
- Semantic-memory preview is opt-in and degraded until runtime smoke proves end-to-end quality.
- TurboQuant acceleration is partial: it may produce candidates, but exact rerank remains required.
- PDF, DOCX, XLSX, URL import, YouTube, audio, rich Studio outputs, and portable notebook export/import are deferred unless current source and fresh receipts say otherwise.

## Runtime Architecture

Gloss has four major layers:

- Frontend: React 19, TypeScript, Zustand stores, Tauri invoke wrappers, and event listeners for chat, ingestion, jobs, and evidence.
- Desktop host: Tauri 2 command surface for notebooks, sources, chat, notes, settings, provider tests, and memory diagnostics.
- Rust runtime: SQLite-backed app and notebook databases, local retrieval, ingestion, provider orchestration, summary scheduling, source-scope resolution, and evidence construction.
- Vendored research/runtime libraries: `llm-pipeline`, `tauri-queue`, `semantic-memory`, and `turbo-quant`.

The default desktop binary compiles semantic-memory support through `semantic-memory-backend`, but runtime activation remains controlled by settings and feature flags. Compiled availability is not treated as consent to use preview behavior.

## Answer Contract

Gloss must not treat all retrieval paths as equivalent. A normal answer is only trustworthy when the evidence contract is explicit.

Each answer should make these runtime facts inspectable:

- Requested backend: what the user or settings asked for.
- Effective backend: what actually served retrieval.
- Fallback and degradation: whether BM25, source-order, raw-content, or provider-only behavior was used.
- Citation validity: how many citations were anchored, filtered, or invalid.
- Source scope: whether selected sources were preserved and whether invalid explicit source IDs were excluded.
- Receipt ID: the traceable runtime receipt for the answer or diagnostic path.

Invalid explicit source IDs must resolve to no or partial scoped sources. They must never widen to all sources.

## TurboQuant Backend

Gloss includes a vendored `turbo-quant` Rust crate under `src-tauri/vendor/turbo-quant`. In Gloss, TurboQuant is used as an experimental candidate accelerator for semantic-memory vector artifacts. It is not the canonical source of evidence, and it does not replace exact `f32` vectors or exact rerank.

What the current crate verifies:

- Deterministic PolarQuant-style packed angle payloads.
- Optional QJL residual sign sketches.
- Codec profiles and compression receipts.
- Explicit `TurboMode::PolarOnly` and `TurboMode::PolarWithQjl` behavior.
- FastHadamard or stored-QR rotation selection depending on dimension support.
- Asymmetric key/value policy structures for experiments.
- Benchmark receipt generation for reproducibility, not deployment proof.

Gloss runtime policy:

- TurboQuant can only be treated as candidate generation.
- Exact rerank remains required before answer evidence is trusted.
- Vector artifacts must carry generation IDs and receipts.
- Stale or missing artifacts are degradation, not invisible success.
- TurboQuant, semantic-memory, dense retrieval, BM25, source-order fallback, and provider-only answers must be disclosed separately.

### Research And Coverage

The TurboQuant algorithm family has been discussed in research and technical press. These references describe the broader algorithmic work; they do not prove Gloss's integration or release quality.

- Google Research announced TurboQuant, PolarQuant, and QJL for KV-cache compression and vector search in March 2026: <https://research.google/blog/turboquant-redefining-ai-efficiency-with-extreme-compression/>
- The paper "TurboQuant: Online Vector Quantization with Near-optimal Distortion Rate" is available on arXiv: <https://arxiv.org/abs/2504.19874>
- OpenReview lists the TurboQuant paper as an ICLR 2026 conference paper: <https://openreview.net/forum?id=tO3ASKZlok>
- InfoQ covered Google's TurboQuant memory and inference claims: <https://www.infoq.com/news/2026/04/turboquant-compression-kv-cache/>
- Tom's Hardware covered TurboQuant's reported KV-cache compression and H100 attention-logit speedups: <https://www.tomshardware.com/tech-industry/artificial-intelligence/googles-turboquant-compresses-llm-kv-caches-to-3-bits-with-no-accuracy-loss>
- Developer roundups have listed `RecursiveIntell/turbo-quant` as a Rust implementation of the TurboQuant, PolarQuant, and QJL family: <https://aetos.ai/posts/e33eff3a1374b370>

## Repository Layout

```text
.
|-- README.md
|-- AGENTS.md
|-- package.json
|-- docs/
|   |-- CURRENT_FEATURE_MATRIX.md
|   `-- codex-runs/
|-- scripts/
|-- src/
|   |-- App.tsx
|   |-- components/
|   |-- lib/
|   `-- stores/
`-- src-tauri/
    |-- Cargo.toml
    |-- src/
    |   |-- commands/
    |   |-- db/
    |   |-- ingestion/
    |   |-- memory/
    |   |-- providers/
    |   `-- retrieval/
    `-- vendor/
        |-- llm-pipeline/
        |-- semantic-memory/
        |-- tauri-queue/
        `-- turbo-quant/
```

## Development Setup

Prerequisites:

- Node.js and npm.
- Rust stable and Cargo.
- Tauri 2 prerequisites for your operating system.
- Optional local Ollama endpoint for local LLM and embedding flows.

Install frontend dependencies:

```bash
npm ci
```

Run the web frontend during UI work:

```bash
npm run dev
```

Run the Tauri desktop app with semantic-memory compiled:

```bash
npm run tauri:dev:sm
```

Run the Tauri desktop app with TurboQuant support compiled:

```bash
npm run tauri:dev:sm-tq
```

Build frontend assets:

```bash
npm run build
```

Build desktop bundles with the semantic-memory profiles:

```bash
npm run tauri:build:sm
npm run tauri:build:sm-tq
```

## Validation

Run focused checks before claiming a runtime behavior. Run the broader gate set before making any release statement.

Frontend and contract checks:

```bash
npm run build
npm test
```

Semantic-memory and TurboQuant checks:

```bash
npm run check:sm-tq-profile
npm run test:tauri:sm
npm run test:tauri:sm-tq
```

Runtime-truth gates:

```bash
python3 scripts/gloss_current_run_truth_gate.py --repo .
python3 scripts/gloss_validator_path_gate.py --repo .
python3 scripts/gloss_receipt_integrity_gate.py --repo .
python3 scripts/gloss_feature_matrix_gate.py --repo .
python3 scripts/gloss_release_replay_gate.py --repo . || true
```

Desktop smoke:

```bash
npm run desktop-smoke
```

Current desktop smoke is not release-grade without a live GUI receipt proving import, query, delete, restart, source-scope, citation, fallback, and raw-ID behavior.

Rust checks:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

If root workspace validation is unavailable or contradicted by current source, use the Tauri manifest explicitly:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

## Data And Privacy Model

Gloss is local-first by design:

- Notebook metadata and source state are stored locally.
- Per-notebook data lives under local notebook directories.
- Provider API keys are managed through local settings and secret storage paths.
- Remote calls happen only through configured providers.
- Local-first does not mean provider-free: if a remote provider is selected, chat or model calls may leave the machine according to that provider configuration.

The UI and diagnostics should disclose provider, backend, fallback, and evidence state rather than hiding runtime degradation.

## Product Non-Goals For This Pass

These items are intentionally not claimed as release-ready in the current pass:

- Broad file-format support beyond text-like sources.
- Release-grade PDF, Office, URL, YouTube, audio, or video ingestion.
- Studio reports, flashcards, quizzes, mind maps, and timelines.
- Portable notebook package export/import.
- Production TurboQuant KV-cache runtime.
- Production semantic-memory default backend.
- Silent fallback from scoped retrieval to all sources.
- Raw UUID-heavy normal UI.

See `RELEASE_NON_GOALS.md` and `docs/CURRENT_FEATURE_MATRIX.md` for the active classifications.

## Release Policy

Gloss is not release-ready until the active source, current run file, feature matrix, receipts, validation scripts, and live desktop smoke all agree.

Minimum release proof requires:

- One current run identity.
- No missing or stale active validation scripts.
- Fresh-unzip release replay for the exact package being shipped.
- Live desktop GUI smoke receipt.
- Runtime evidence showing requested backend, effective backend, fallback/degradation, citation validity, source-scope integrity, and receipt ID.
- No source-scope widening.
- No notebook/import jobs running against missing or superseded notebooks.
- No broad deferred feature presented as implemented.

## License

Gloss is licensed under `AGPL-3.0-only`.
