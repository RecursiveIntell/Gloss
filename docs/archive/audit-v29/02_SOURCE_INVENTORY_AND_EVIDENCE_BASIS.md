# Source Inventory and Evidence Basis

## Current high-trust evidence

- Current repo/package snapshot: `Gloss-generic-rust-next-codex-context-20260525T215913Z.zip`.
- Current sidecars: report, findings, excluded, codex-archive JSON for `20260525T215913Z`.
- Current root package evidence: strict package, 29,475 included files, 0 errors, 68 warnings, Rust/Node/Git detected.
- Current run marker: `docs/codex-runs/CURRENT_RUN.md` says `GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525`.
- Current codex-archive sidecar still reports `current_run: P30`; this is stale package truth.
- Current source shows `ChatRequest` includes `temperature`, `stream`, `num_ctx`, but not top-k/top-p/min-p/typical-p/repeat/seed controls.
- Current chat path hardcodes `temperature: 0.7` for main answer generation.
- Current smoke path hardcodes `temperature: 0.0`.
- Current summarization/vision paths have additional hardcoded temperatures.
- Current semantic-memory path includes FastEmbed provider via `MemoryStore::open_with_embedder`.
- Current live receipt gate is negative for dense chunks, semantic projection, TurboQuant, and desktop smoke.

## Session-derived evidence

- Semantic-memory hard spec identified overbroad `semantic_memory_feature_disabled` fallback reason.
- Inspector Dock update pack promoted Prompt/Evidence/Receipt/Sources/Notes tabs and GenerationReceipt/PromptReceipt.
- User reports semantic-memory settings are enabled; therefore next pass must prove backend DB/runtime state instead of assuming missing settings.

## Missing evidence to preserve as unresolved until Codex obtains it

- Fresh local Cargo/Tauri build/test outputs from the actual development machine.
- Live desktop smoke video/screenshot/receipt after final implementation.
- Live semantic-memory strict fixture receipt.
- Live TurboQuant exact rerank/artifact receipt.
- Fresh package generated after cleanup proving scope and current-run truth.

## Public dependency anchors checked during planning

- Ollama API docs: `/api/generate` accepts `options`; examples show `temperature`; docs state streaming can be disabled with `stream: false`; `keep_alive` controls loaded model lifetime. URL: https://github.com/ollama/ollama/blob/main/docs/api.md
- FastEmbed Rust docs: `fastembed` crate and embedding model APIs. URL: https://docs.rs/fastembed/latest/fastembed/
- Tauri 2 security docs: local app permission/security posture. URL: https://v2.tauri.app/security/
- SQLite FTS5 official docs: FTS5 full-text indexing/search. URL: https://www.sqlite.org/fts5.html
- OpenAI Responses API docs show response fields including `temperature`, `top_p`, `max_output_tokens` and incomplete reason `max_output_tokens`. URL: https://platform.openai.com/docs/api-reference/responses/create
- Anthropic Messages API should be rechecked by Codex at implementation time because provider parameter support is current/frequently updated. URL: https://docs.anthropic.com/en/api/messages

See `EVIDENCE/LOCAL_STATIC_GATE_RESULTS.md` for local static validation observations.
