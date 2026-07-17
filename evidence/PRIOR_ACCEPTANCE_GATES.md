# Acceptance Gates and Commands

## Non-negotiable gates

1. **No-retrieval chat gate** — source list loading/partial/error does not block chat; request sends with `SourceScope {kind:'none'}`.
2. **Terminal event gate** — every spawned chat attempt emits one terminal event that clears frontend streaming.
3. **Active stream identity gate** — terminal event routing is keyed by streaming message/notebook identity, not only active notebook view.
4. **Persistence gate** — success `chat:done` requires assistant message persistence or an explicit partial/cancel artifact.
5. **Provider smoke gate** — operator can run Ollama selected-model smoke and copy last trace from UI.
6. **Done-frame regression gate** — fake stream token -> done=true -> no EOF finalizes without waiting for EOF.
7. **Model registry gate** — selected/default model must exist before chat; stale model yields explicit UI error or auto-repair with notice.
8. **Receipt consistency gate** — final receipt cannot contradict release candidate gate results.
9. **Package replay gate** — fresh package includes all validation scripts it references.
10. **Cargo proof gate** — Rust checks must run on a machine with cargo; sandbox skip is not release proof.

## Commands

```bash
npm run build
npm test
cargo fmt --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
python3 scripts/audit_chat_path_integration.py .
python3 scripts/audit_retrieval_disclosures.py --repo . --latest
python3 scripts/audit_tauri_security.py .
python3 validation/gloss_package_scope_gate.py --repo .
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT --timeout-per-gate 30
```

## Required live operator proof

Run Ollama smoke from the app UI or backend command. Attach `ChatAttemptTraceV1` showing:

- provider URL class
- selected model
- provider_start phase reached or provider_config_error surfaced
- first_token_seen
- done_seen
- assistant_persisted
- terminal event emitted
- frontend streaming cleared
