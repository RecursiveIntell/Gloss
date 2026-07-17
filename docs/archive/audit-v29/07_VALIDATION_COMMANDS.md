# Validation Commands

Run these from the Gloss repo root after each relevant phase.

## Frontend

```bash
npm ci --no-audit --no-fund
npm test
npm run build
```

## Rust / Tauri

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
```

## Chat runtime

```bash
cargo test --workspace chat_done_frame_without_eof
cargo test --workspace chat_partial_persist_on_idle_timeout
cargo test --workspace chat_stop_cancels_provider_and_persists_once
cargo test --workspace chat_preempts_background_summary_gate
python3 validation/live_ollama_chat_smoke.py --repo . --model <installed-model>
```

## Validation gates

```bash
python3 scripts/chat_runtime_static_audit.py --repo .
python3 validation/gloss_timeout_partial_continuation_gate.py --repo .
python3 validation/gloss_package_scope_gate.py --repo .
python3 validation/gloss_legacy_office_extractors_gate.py --repo .
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT --max-subgate-seconds 30
```

## Release proof

```bash
python3 validation/live_desktop_smoke.py --repo .
python3 validation/live_semantic_memory_smoke.py --repo .
python3 validation/gloss_turboquant_runtime_gate.py --repo .
python3 validation/gloss_installer_smoke_gate.py --repo .
python3 validation/gloss_public_claim_gate.py --repo .
```

## Package replay

```bash
python3 z.py --root . --mode release-source --strict
rm -rf /tmp/gloss-fresh-replay
mkdir -p /tmp/gloss-fresh-replay
unzip <release-package>.zip -d /tmp/gloss-fresh-replay
cd /tmp/gloss-fresh-replay/Gloss
python3 validation/gloss_package_scope_gate.py --repo .
npm ci --no-audit --no-fund
npm test
npm run build
cargo check --workspace --all-targets
```
