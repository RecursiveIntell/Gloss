# AGENTS.md — Gloss Release-Proof Repair

## Project

Gloss is a local-first Tauri + React + Rust notebook/RAG desktop app. The current task is a release-proof repair pass, not a feature expansion.

## Current run

`GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525`

Write all run receipts/reports under:

```text
docs/codex-runs/GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525/
```

Historical run material is evidence only. It must not act as active instruction.

## Source hierarchy

1. Current repo source files, manifests, tests, receipts, and screenshots.
2. Current run bundle under `docs/codex-runs/GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525/`.
3. Existing product docs only when consistent with current source.
4. Historical Codex run docs only as evidence, never as active instruction.
5. README claims are not proof.

## Hard rules

- Do not add broad features.
- Do not weaken or bypass release gates.
- Do not claim release readiness without live receipts.
- Do not leave old pass artifacts active at repo root.
- Do not keep P31/P32/P33/P34/P35/P36 instructions as active guidance.
- Do not silently fallback from semantic-memory/TurboQuant to local retrieval.
- Do not keep native dense embeddings and semantic-memory embeddings as uncoordinated source-of-truth paths.
- Do not report pending queue items as running work.
- Do not report folder import complete unless terminal receipts count ready, failed, skipped, and cancelled states.
- Do not make vector indexes, caches, UI state, or generated docs source of truth.

## Canonical owners

| Concept | Owner |
|---|---|
| Product app workflow | `Gloss/src-tauri/src/*`, `Gloss/src/*` |
| Native dense indexing | `src-tauri/src/ingestion/embed.rs`, `src-tauri/src/state.rs`, notebook DB chunk embedding fields |
| semantic-memory projection/search | `Libraries/semantic-memory`, with app adapter in `src-tauri/src/memory/semantic_memory_adapter.rs` |
| Embedding provider boundary | Must be a single app-level provider abstraction used by native dense and semantic-memory projection |
| TurboQuant candidate artifacts | `Libraries/semantic-memory` + `Libraries/turbo-quant`; Gloss only routes/configures/receipts |
| Release proof | `docs/codex-runs/GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525/FINAL_RECEIPT.json` and validation scripts |
| Active Codex instruction | This `AGENTS.md` + current run prompts only |

## Required final artifacts

```text
docs/codex-runs/GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525/
  STARTUP_PREFLIGHT.md
  STALE_PASS_CLEANUP_MANIFEST.json
  CHANGED_FILES.txt
  COMMANDS_RUN.log
  VALIDATION_RESULTS.md
  EMBEDDING_PROVIDER_RECEIPT.json
  EMBEDDING_DIAGNOSTICS_RECEIPT.json
  DENSE_INDEXING_RECEIPT.json
  SEMANTIC_MEMORY_PROJECTION_RECEIPT.json
  TURBOQUANT_RUNTIME_RECEIPT.json
  LIVE_DESKTOP_SMOKE_RECEIPT.json
  PACKAGE_WARNING_REVIEW.md
  PUBLIC_CLAIM_DIFF.md
  FINAL_AUDITOR_HANDOFF.md
  FINAL_RECEIPT.json
  REMAINING_DELTA.md
  ROLLBACK_PLAN.md
```

## Required validation

Run and record results for:

```bash
npm run build
node scripts/run_frontend_contract_tests.mjs
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
python3 validation/gloss_stale_pass_surface_gate.py --repo .
python3 validation/gloss_embedding_provider_gate.py --repo .
python3 validation/gloss_live_receipt_gate.py --repo . --run-id GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525
python3 validation/gloss_next_release_gate.py --repo . --run-id GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets -- -D warnings
npm run tauri:build:release
```

Skipped checks require exact reason and must be listed in `FINAL_RECEIPT.json`.
