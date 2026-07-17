# Command Results From This Audit

Working extraction root: `/mnt/data/gloss_repo/Gloss`.

## Archive/package extraction

- Extracted `/mnt/data/a653b55f-f5fb-48ae-9775-6f307657c248.7z` into `/mnt/data/gloss_extract`.
- Extracted nested `Gloss-generic-rust-next-codex-context-20260526T161148Z.zip` into `/mnt/data/gloss_repo`.
- Gloss repo root used for inspection: `/mnt/data/gloss_repo/Gloss`.
- Top-level extracted package included both `Gloss/` and `Libraries/`, plus `/Coding`-level docs from the package root.

## Git/toolchain

```text
git status --short             -> fatal: not a git repository
git rev-parse --show-toplevel  -> fatal: not a git repository
git rev-parse HEAD             -> fatal: not a git repository
cargo fmt/check/test/clippy     -> not run/proven; cargo not available in this environment
```

## Node/frontend

```text
npm run build       -> initially failed because dependencies were not installed
npm ci --no-audit --no-fund -> succeeded; 172 packages added
npm run build       -> passed after npm ci
npm test            -> passed; node scripts/run_frontend_contract_tests.mjs
```

## Validation gates sampled

```text
python3 validation/gloss_current_run_truth_gate.py --repo .                -> pass
python3 validation/gloss_package_scope_gate.py --repo .                    -> fail: no Gloss package manifest found
python3 validation/gloss_decoding_settings_gate.py --repo .                -> pass
python3 validation/gloss_prompt_generation_receipts_gate.py --repo .       -> pass
python3 validation/gloss_timeout_partial_continuation_gate.py --repo .     -> fail: TimeoutChangeReceiptV1 missing
python3 validation/gloss_live_semantic_memory_smoke_gate.py --repo .       -> fail: LIVE_SEMANTIC_MEMORY_SMOKE_RECEIPT.json missing
python3 validation/gloss_turboquant_runtime_gate.py --repo .               -> fail: TURBOQUANT_RUNTIME_RECEIPT.json missing
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT -> fail: release_ready=false
python3 validation/gloss_inspector_dock_gate.py --repo .                   -> pass
python3 validation/gloss_dense_tq_release_gate.py --repo .                 -> pass
python3 validation/gloss_embedding_provider_gate.py --repo .               -> pass
python3 validation/gloss_p36_static_gate.py --repo .                       -> pass with warning: run ID not visible in AGENTS.md/README.md
python3 validation/gloss_next_release_gate.py --repo .                     -> timed out in this audit environment
```

Do not convert these sampled command results into release proof. The Rust/Tauri/live model paths were not proven here.
