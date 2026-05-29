# AGENTS.md — Gloss Closing Pass Rules

Active run: `GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526`

## Project purpose

Gloss is a local-first notebook/RAG/chat application. Chat must work without retrieval when retrieval/source state is degraded. Retrieval, semantic-memory, TurboQuant, and release claims must be proof-bearing.

## Source-of-truth ownership

- Frontend chat lifecycle: `src/stores/chatStore.ts`, `src/components/chat/ChatPanel.tsx`, `src/App.tsx` event forwarding.
- Backend chat lifecycle: `src-tauri/src/commands/chat/mod.rs`.
- Provider validation and model registry: `src-tauri/src/providers/*`, settings commands, settings UI.
- Source/retrieval selection: `src/stores/sourceStore.ts`, retrieval backend modules.
- Package/release proof: `scripts/`, `validation/`, `docs/codex-runs/*`, package sidecars.

## Forbidden behavior

- Do not hide provider errors behind a spinner.
- Do not block no-retrieval chat because source list loading/partial/error.
- Do not emit `chat:done` before assistant persistence succeeds unless explicit partial/cancel artifact is emitted.
- Do not silently allow LAN or cloud endpoints; provider authority must be explicit.
- Do not claim semantic-memory/TurboQuant/dense indexing proof from dependencies alone.
- Do not add compatibility shims or shadow truth stores.
- Do not update release docs by hand if gate JSON says failed.

## Required validation

Run targeted tests first, then full gates:

```bash
npm run build
npm test
cargo fmt --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
python3 validation/validate_source_send_gate.py .
python3 validation/validate_frontend_event_routing.py .
python3 validation/validate_chat_terminal_contract.py .
python3 validation/validate_provider_lan_policy.py .
python3 validation/validate_release_receipt_consistency.py .
```

## Completion rule

Receipts or it did not happen. Final answer must include hostile-auditor handoff.
