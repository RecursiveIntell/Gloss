# Validation Commands

## Baseline static checks

```bash
node scripts/run_frontend_contract_tests.mjs
npm run build
python3 scripts/check_gloss_sm_tq_fix.py .
python3 scripts/gloss_current_run_truth_gate.py --repo .
python3 scripts/gloss_receipt_integrity_gate.py --repo .
python3 scripts/gloss_feature_matrix_gate.py --repo .
python3 scripts/gloss_issue_ledger_gate.py --ledger ISSUE_LEDGER.csv
python3 scripts/gloss_static_runtime_truth_gate.py --repo .
python3 scripts/audit_tauri_security.py .
python3 scripts/audit_ui_disclosure_a11y.py .
python3 scripts/gloss_evidence_ui_gate.py --repo .
python3 scripts/gloss_retrieval_gate.py --repo .
python3 scripts/gloss_validator_path_gate.py --repo .
```

## Rust/Tauri checks

```bash
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets -- -D warnings
npm run tauri:build:release
```

If any command cannot run, record exact environment reason and keep as blocker unless explicitly non-applicable.

## New gates

```bash
python3 validation/gloss_current_run_truth_gate.py --repo .
python3 validation/gloss_stale_pass_surface_gate.py --repo .
python3 validation/gloss_package_scope_gate.py --repo .
python3 validation/gloss_semantic_memory_runtime_truth_gate.py --repo .
python3 validation/gloss_retrieval_decision_gate.py --repo .
python3 validation/gloss_generation_receipt_gate.py --repo .
python3 validation/gloss_prompt_receipt_gate.py --repo .
python3 validation/gloss_decoding_settings_gate.py --repo .
python3 validation/gloss_timeout_partial_continuation_gate.py --repo .
python3 validation/gloss_inspector_dock_gate.py --repo .
python3 validation/gloss_live_semantic_memory_smoke_gate.py --repo .
python3 validation/gloss_turboquant_runtime_gate.py --repo .
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT
```
