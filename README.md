# Gloss

Gloss is a local-first desktop notebook for grounded chat over local sources. It is built with Tauri 2, Rust, React, TypeScript, SQLite/FTS5, `usearch`, and local model providers, with Ollama as the primary runtime target.

The current repository state includes chat runtime reliability work from `CHAT_RUNTIME_FIX_20260518`. The code-level fixes are present, provider-only Ollama smoke passed in prior evidence, and automated build/tests passed in that run. The app is not release-certified yet because live desktop chat smoke was blocked and still needs to be completed.

## What Works

- Create and manage local notebooks.
- Import sources into per-notebook storage.
- Build local retrieval context with SQLite/FTS5 and vector search.
- Chat with configured models through the provider registry.
- Stream chat events with durable attempt tracing for no-response diagnosis.
- Configure Ollama, OpenAI, Anthropic, and llama.cpp provider records.
- Use `gloss-local` memory by default.
- Build optional `semantic-memory-preview` support from vendored source crates.

## Current Limits

- Release readiness is blocked until a live desktop smoke proves visible streamed tokens, visible errors, visible timeouts, or persisted `ChatAttemptTraceV1` for real chat attempts.
- `semantic-memory-preview` is preview-only and not the default backend.
- TurboQuant remains optional candidate acceleration inside semantic-memory and must remain exact-reranked.
- Ollama model availability is external to the repo; users need a running Ollama server and pulled models.
- CI/build checks do not prove the interactive desktop path.

## Repository Layout

```text
src/                         React frontend
src-tauri/                   Tauri/Rust backend
src-tauri/vendor/            Vendored local path crates used by Cargo
src-tauri/vendor/semantic-memory
src-tauri/vendor/forge-memory-bridge
src-tauri/vendor/semantic-memory-forge
src-tauri/vendor/turbo-quant
src-tauri/vendor/stack-ids
scripts/                     Validation, smoke, package, and audit helpers
fixtures/                    Test and audit fixtures
schemas/                     Receipt and validation schemas
docs/codex-runs/CHAT_RUNTIME_FIX_20260518/
                             Latest chat runtime fix receipts and blockers
```

Generated archives, local databases, build output, Cargo vendor cache, and historical extracted run archives are intentionally ignored.

## Prerequisites

- Node.js 22 or newer
- npm
- Rust stable with `rustfmt`
- Linux Tauri build dependencies:

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential curl file libayatana-appindicator3-dev librsvg2-dev \
  libssl-dev libwebkit2gtk-4.1-dev libxdo-dev pkg-config wget
```

- Ollama for local chat, for example:

```bash
ollama serve
ollama pull cogito:3b
ollama pull qwen3.5:4b
```

## Install

```bash
npm ci
```

No sibling checkout is required for the optional semantic-memory feature. The local path crates needed by Cargo are vendored under `src-tauri/vendor/`.

## Development

Run the web frontend:

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

Build a Tauri bundle:

```bash
npm run tauri build
```

The Tauri config currently targets RPM packaging on Linux.

## Validation

Core checks:

```bash
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml --check
cargo test --manifest-path src-tauri/Cargo.toml
```

Chat runtime static checks:

```bash
python3 scripts/chat_runtime_static_audit.py --repo .
python3 scripts/chat_runtime_preflight.py --repo .
```

Optional semantic-memory checks:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

Manual desktop smoke remains required before claiming release readiness:

```bash
scripts/chat_runtime_smoke_manual.sh
```

Use `docs/codex-runs/CHAT_RUNTIME_FIX_20260518/DESKTOP_SMOKE_BLOCKER.md` and `FINAL_RECEIPT.json` for the exact remaining proof gap.

## Provider Configuration

Provider records are the source of truth for provider base URLs and API-key references. The legacy setting keys may still appear in UI compatibility paths, but chat/model refresh/provider test should resolve through provider rows.

Default local Ollama URL:

```text
http://localhost:11434
```

For remote Ollama over a LAN or VPN, configure the Ollama provider base URL in Settings, refresh models, then select the model.

## License

Gloss is licensed under `AGPL-3.0-only`. Some vendored crates carry their own licenses; see each crate under `src-tauri/vendor/`.
