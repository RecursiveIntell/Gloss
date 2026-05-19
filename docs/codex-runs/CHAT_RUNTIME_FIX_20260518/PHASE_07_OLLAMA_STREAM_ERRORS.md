# Phase 07 - Ollama Stream-Error Hardening

Files inspected:
- `src-tauri/src/providers/ollama.rs`
- `src-tauri/src/commands/chat.rs`

Files changed:
- `src-tauri/src/providers/ollama.rs`
- `src-tauri/src/commands/chat.rs`

Commands run:
- `python3 scripts/chat_runtime_static_audit.py --repo .`
- Direct Ollama smoke files under this run directory.
- `cargo test --manifest-path src-tauri/Cargo.toml providers::ollama::tests -- --nocapture | tee docs/codex-runs/CHAT_RUNTIME_FIX_20260518/cargo_test_ollama_provider.log`

Tests passed/failed/skipped:
- Static audit detected Ollama stream JSON error handling.
- Direct Ollama smoke showed `think:false` is required for `qwen3.5:4b` to return assistant content.
- Targeted Ollama provider tests passed for `think:false`, stream error conversion, and normal content extraction.

Unresolved risks:
- Synthetic Ollama NDJSON error-frame unit test was not added.

Exact blockers:
- None.
