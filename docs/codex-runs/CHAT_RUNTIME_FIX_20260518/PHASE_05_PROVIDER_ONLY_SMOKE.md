# Phase 05 - Provider-Only Smoke Command

Files inspected:
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/lib.rs`
- `src/lib/tauri.ts`
- `src-tauri/src/providers/ollama.rs`

Files changed:
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/lib.rs`
- `src/lib/tauri.ts`

Commands run:
- Direct Ollama smoke for `cogito:3b`, saved to `provider_only_smoke_cogito_3b.json`.
- Direct Ollama smoke for `qwen3.5:4b`, saved to `provider_only_smoke_qwen3_5_4b*.json`.

Tests passed/failed/skipped:
- `cogito:3b` returned `gloss smoke ok`.
- `qwen3.5:4b` returned empty content until `think:false` was supplied; `think:false` returned `gloss smoke ok`.
- Tauri command `debug_chat_provider_smoke` was added but not invoked through a live Tauri desktop session.

Unresolved risks:
- Provider-only Tauri command path needs live command invocation in desktop smoke.

Exact blockers:
- No command-line Tauri invoke harness was available.
