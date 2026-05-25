# Gloss

Gloss is a local-first AI notebook for serious research work: collect sources, organize notebooks, ask grounded questions, inspect citations, save notes, and choose the model backend that fits the job.

It is built as a desktop product with Tauri, React, TypeScript, Rust, SQLite, local retrieval, provider configuration, and guarded experimental memory acceleration. The goal is a research workspace that feels like a full product, not a chat window with file upload bolted on.

## Why Gloss

Most AI tools treat documents as temporary prompt material. Gloss treats them as a durable research library.

- Local-first notebooks: source files, metadata, indexes, notes, and conversations live on the local machine.
- Source-grounded answers: chat responses can carry citations and retrieval evidence that show what context was used.
- Explicit source scope: ask across a whole notebook, selected sources, or no sources.
- Provider choice: work with local providers such as Ollama or llama.cpp-compatible servers, or configure hosted providers when appropriate.
- Notebook workflow: import sources, chat with them, save useful answers, pin notes, and keep related work together.
- Runtime transparency: Gloss exposes backend selection, fallback, citation health, and source-scope state instead of hiding degraded behavior.
- Experimental acceleration: semantic-memory and TurboQuant paths exist behind explicit settings while the stable local retrieval path remains conservative.

## Product Status

Gloss is under active development. The current product surface focuses on local notebooks, text-like source ingestion, scoped chat, citations, notes, provider configuration, retrieval diagnostics, and experimental semantic-memory integration.

Current boundaries:

- Text, Markdown, code, folders, and pasted content are the primary source types.
- BM25/local retrieval is the stable fallback.
- semantic-memory is an opt-in preview backend.
- TurboQuant is an experimental candidate accelerator and still requires exact rerank before answer evidence is trusted.
- PDF, DOCX, XLSX, URL import, YouTube import, audio workflows, rich study outputs, and portable notebook packages are future-facing surfaces unless current code proves otherwise.

## Core Experience

### Notebooks

Create separate notebooks for projects, clients, papers, codebases, classes, or investigations. Each notebook has local storage for sources, chunks, conversations, notes, summaries, retrieval state, and diagnostics.

### Sources

Gloss supports practical research ingestion:

- Text and Markdown.
- Code and config files.
- Pasted text.
- Recursive folder import for supported text-like files.
- Partial image and video job surfaces while richer media workflows remain experimental.

Unsupported binaries, generated artifacts, archives, office documents, and noisy machine output should not be treated as supported research inputs until a current extractor exists and has been validated.

### Chat With Evidence

Gloss chat is designed around answer accountability.

- Streamed assistant responses.
- Stop and retry controls.
- Conversation history per notebook.
- Suggested questions.
- Source citations.
- Evidence details for backend, fallback, source scope, context count, citation validity, and diagnostics.
- Save useful assistant responses as notes.

### Source Scope

Source scope is a first-class control:

- All sources: search the whole notebook.
- Selected sources: constrain chat to specific files or pasted items.
- No sources: talk to the model without retrieval context.

Invalid explicit source selections must not silently widen into a broader search.

### Notes

Gloss keeps research output close to the material that produced it:

- Create and edit notes.
- Pin important notes.
- Save assistant responses.
- Keep citation-linked findings alongside the original notebook.

### Providers

Gloss is designed for model flexibility:

- Ollama.
- llama.cpp-compatible local servers.
- OpenAI.
- Anthropic.

Provider URLs, enabled state, model lists, selected models, context windows, and capability metadata are tracked locally. Hosted provider usage is explicit configuration, not an assumption.

## Architecture

Gloss has four major layers:

- Frontend: React 19, TypeScript, Zustand stores, Tauri invoke wrappers, and event listeners for chat, ingestion, source updates, and evidence.
- Desktop host: Tauri 2 commands for notebooks, sources, chat, notes, settings, provider checks, and diagnostics.
- Rust runtime: SQLite-backed app and notebook databases, local retrieval, ingestion, provider orchestration, summary scheduling, source-scope resolution, and citation construction.
- Vendored libraries: `llm-pipeline`, `tauri-queue`, `semantic-memory`, and `turbo-quant`.

The default desktop build can include semantic-memory support, but runtime activation is controlled by settings. Build availability is not user consent.

## Retrieval Contract

Gloss should make retrieval behavior inspectable instead of treating all answer paths as equivalent.

An answer is only trustworthy when these facts are clear:

- Requested backend: what settings or the user asked for.
- Effective backend: what actually served the context.
- Fallback: whether local BM25, source-order, raw-content, or provider-only behavior was used.
- Citation validity: how many citations were grounded, filtered, or invalid.
- Source scope: whether selected sources were preserved.
- Diagnostics: the trace data needed to reproduce or debug the answer path.

TurboQuant, semantic-memory, dense retrieval, BM25, source-order fallback, and provider-only answers are not interchangeable.

## TurboQuant Backend

Gloss vendors `turbo-quant` under `src-tauri/vendor/turbo-quant`. It is an experimental Rust backend for TurboQuant-family vector-compression work, including deterministic PolarQuant-style codes, optional QJL residual sketches, codec profiles, packed storage, and benchmark artifacts.

In Gloss, TurboQuant is used as candidate acceleration for semantic-memory vector artifacts. It is not the canonical evidence store and does not replace exact vectors or exact rerank.

Current Gloss policy:

- TurboQuant can produce candidates.
- Exact rerank remains required.
- Stale or missing vector artifacts are a degraded state.
- TurboQuant acceleration must be disclosed separately from semantic-memory, BM25, dense retrieval, and provider-only answers.
- Benchmark artifacts are reproducibility aids, not product proof.

### Publications And Coverage

TurboQuant has been discussed in academic and technical publications. These links describe the broader algorithmic work and public coverage; they do not by themselves validate Gloss integration.

- Google Research announced TurboQuant, PolarQuant, and QJL for KV-cache compression and vector search in March 2026: <https://research.google/blog/turboquant-redefining-ai-efficiency-with-extreme-compression/>
- The paper "TurboQuant: Online Vector Quantization with Near-optimal Distortion Rate" is available on arXiv: <https://arxiv.org/abs/2504.19874>
- OpenReview lists the TurboQuant paper as an ICLR 2026 conference paper: <https://openreview.net/forum?id=tO3ASKZlok>
- InfoQ covered Google's TurboQuant memory and inference claims: <https://www.infoq.com/news/2026/04/turboquant-compression-kv-cache/>
- Tom's Hardware covered TurboQuant's reported KV-cache compression and H100 attention-logit speedups: <https://www.tomshardware.com/tech-industry/artificial-intelligence/googles-turboquant-compresses-llm-kv-caches-to-3-bits-with-no-accuracy-loss>
- Aetos/ArshTechPro listed `RecursiveIntell/turbo-quant` as a Rust implementation of the TurboQuant, PolarQuant, and QJL family: <https://aetos.ai/posts/e33eff3a1374b370>
- DevPik also identified `RecursiveIntell/turbo-quant` as a Rust implementation in its TurboQuant coverage: <https://www.devpik.com/blog/google-turboquant-pied-piper-ai-compression>

## Repository Layout

```text
.
|-- README.md
|-- package.json
|-- docs/
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

## Development

Prerequisites:

- Node.js and npm.
- Rust stable and Cargo.
- Tauri 2 prerequisites for your operating system.
- Optional local Ollama endpoint for local LLM and embedding flows.

Install dependencies:

```bash
npm ci
```

Run the frontend:

```bash
npm run dev
```

Run the Tauri app with semantic-memory compiled:

```bash
npm run tauri:dev:sm
```

Run the Tauri app with TurboQuant support compiled:

```bash
npm run tauri:dev:sm-tq
```

Build frontend assets:

```bash
npm run build
```

Build desktop bundles:

```bash
npm run tauri:build:sm
npm run tauri:build:sm-tq
```

## Testing

Frontend and contract tests:

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

Rust checks:

```bash
cargo fmt --all -- --check
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

Desktop smoke:

```bash
npm run desktop-smoke
```

## Data And Privacy

Gloss is local-first:

- Notebook metadata and source state are stored locally.
- Per-notebook data lives in local notebook directories.
- Provider settings are local.
- Remote calls happen only through configured providers.
- Local-first does not mean provider-free: hosted provider use depends on the selected configuration.

The product should disclose provider, backend, fallback, source scope, and citation state so users can judge the answer they are reading.

## Roadmap Themes

The long-term product direction includes:

- Richer import coverage for PDFs, Office files, URLs, media, and notebook packages.
- Stronger live desktop validation for import, chat, delete, restart, and recovery workflows.
- More complete semantic-memory indexing and repair tools.
- TurboQuant artifact management and benchmark comparison views.
- Studio-style outputs such as reports, flashcards, quizzes, timelines, and maps.
- Better diagnostics for provider configuration, model context limits, and retrieval quality.

## License

Gloss is licensed under `AGPL-3.0-only`.
