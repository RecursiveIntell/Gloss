# Acceptance Gates

## S0 release-blocking gates

1. **Trace gate**: UI can run provider smoke and copy `ChatAttemptTraceV1`.
2. **No-retrieval chat gate**: source list loading/partial/error does not block chat; it sends with `SourceScope {kind:'none'}`.
3. **Terminal event gate**: every spawned chat task exit emits exactly one terminal event.
4. **Event routing gate**: terminal chat events are routed by active stream identity, not active notebook view.
5. **Persistence gate**: `chat:done` occurs only after assistant persistence or explicit partial/cancel artifact.
6. **Provider/model gate**: selected model exists and chat smoke uses it.
7. **Done-frame gate**: fake provider token -> done=true -> no EOF finalizes.
8. **Package replay gate**: no validation script referenced by `run_all_checks.sh` is absent from fresh package.
9. **Run-truth gate**: CurrentRunTruthV1 projections agree across docs, final receipt, and sidecars.
10. **Aggregate validation gate**: release gate has per-child timeouts and cannot hang.
11. **Cargo proof gate**: Rust checks run locally; skipped sandbox checks are not release proof.
12. **Live Ollama gate**: live local/LAN/tunnel Ollama smoke reaches first token, done frame, assistant persisted, frontend cleared.

## Required commands

```bash
npm run build
npm test
cargo fmt --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets -- -D warnings
python3 validation/validate_source_send_gate.py .
python3 validation/validate_frontend_event_routing.py .
python3 validation/validate_chat_terminal_contract.py .
python3 validation/validate_provider_lan_policy.py .
python3 validation/validate_release_receipt_consistency.py .
python3 validation/gloss_package_scope_gate.py --repo .
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT --timeout-per-gate 30
```

## Live smoke command equivalent

From UI or backend command, run:

```text
provider: ollama
model: selected active model
prompt: Reply exactly: gloss smoke ok
```

Required trace fields:

```text
provider URL class
selected model
provider_configured or provider_config_error
first_token_seen
done_seen
assistant_persisted
terminal event emitted
frontend streaming cleared
```
