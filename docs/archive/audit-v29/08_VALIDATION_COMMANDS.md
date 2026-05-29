# Validation Commands

Run from Gloss repo root unless noted. Tie every command to a gate; do not run commands as decoration.

## Preflight

```bash
pwd
git status --short
git rev-parse --show-toplevel
git rev-parse HEAD
node --version
npm --version
cargo --version
rustc --version
tauri --version || npx tauri --version || true
```

## Install/frontend

```bash
npm ci --no-audit --no-fund
npm run build
npm test
```

## Rust/Tauri

```bash
cargo fmt --all -- --check
cargo check --workspace --all-targets
cargo test --workspace --all-targets
cargo clippy --workspace --all-targets -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
npm run tauri:build:release
npm run desktop-smoke
```

## Existing Gloss gates

```bash
python3 validation/gloss_current_run_truth_gate.py --repo .
python3 validation/gloss_package_scope_gate.py --repo .
python3 validation/gloss_decoding_settings_gate.py --repo .
python3 validation/gloss_prompt_generation_receipts_gate.py --repo .
python3 validation/gloss_timeout_partial_continuation_gate.py --repo .
python3 validation/gloss_live_semantic_memory_smoke_gate.py --repo .
python3 validation/gloss_turboquant_runtime_gate.py --repo .
python3 validation/gloss_inspector_dock_gate.py --repo .
python3 validation/gloss_dense_tq_release_gate.py --repo .
python3 validation/gloss_embedding_provider_gate.py --repo .
python3 validation/gloss_p36_static_gate.py --repo .
python3 validation/gloss_release_candidate_gate.py --repo . --run-id GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526
```

## New gates Codex must add if missing

```bash
python3 validation/gloss_boundary_format_gate.py --repo .
python3 validation/gloss_broad_spec_gate.py --repo .
python3 validation/gloss_web_media_egress_gate.py --repo .
python3 validation/gloss_studio_schema_gate.py --repo .
python3 validation/gloss_export_import_gate.py --repo .
python3 validation/gloss_db_doctor_gate.py --repo .
python3 validation/gloss_security_privacy_gate.py --repo .
python3 validation/gloss_public_claim_gate.py --repo .
python3 validation/gloss_fresh_unzip_replay_gate.py --repo .
```

## Assertions

```bash
grep -R "TODO\|FIXME\|TBD\|@filename\|{feature}\|<placeholder>" -n . --exclude-dir=node_modules --exclude-dir=target || true
find . -name SKILL.md -maxdepth 6 -print
python3 scripts/gloss_issue_ledger_gate.py --repo . --ledger docs/codex-runs/GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526/ISSUE_LEDGER.csv
```
