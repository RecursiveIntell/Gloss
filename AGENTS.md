# AGENTS.md — Gloss Release Candidate Governance

## Project purpose

Gloss is a local-first Tauri + React notebook/RAG desktop application. Release work must preserve local-first behavior, truthful retrieval status, source citations, and explicit degradation/receipt evidence.

## Current run

Active release-candidate run: `GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519`.

Codex must use `docs/codex-runs/CURRENT_RUN.md` and `docs/codex-runs/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/FINAL_RECEIPT.json` as run truth. Historical run folders are evidence only.

## Canonical owners

- Feature definitions: `src-tauri/src/features.rs`.
- Settings commands/persistence: `src-tauri/src/commands/settings.rs`, `src-tauri/src/db/app_db.rs`.
- semantic-memory adapter: `src-tauri/src/memory/semantic_memory_adapter.rs`.
- TurboQuant policy: semantic-memory runtime config only when user setting is active.
- Retrieval outcome: `src-tauri/src/retrieval/hybrid_search.rs::local_retrieval_outcome`.
- Source scope: `src-tauri/src/retrieval/source_scope.rs` and SQL-scoped DB queries.
- Chat evidence: `ChatAttemptTraceV1` and retrieval outcome receipts.
- GUI production wiring: existing React/Zustand/Tauri files under `src/`; GUI reference under `docs/design/GLOSS_GUI_REFERENCE_20260519` is reference only.

## Hard rules

- Gloss local remains default.
- Experimental features, semantic-memory preview, and TurboQuant default off.
- Build feature availability is not runtime consent.
- TurboQuant is candidate-only and exact rerank remains required.
- Do not claim dense hybrid unless dense+BM25 actually ran.
- Do not silently widen source scope.
- Do not use provider-only smoke as full RAG proof.
- Do not paste standalone prototype HTML/Babel/global React into production.
- Do not claim release readiness without final receipt and all gates.

## Validation commands

Run relevant commands before final handoff:

```bash
python3 scripts/p33_release_preflight.py --repo . --run-id GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519
python3 scripts/p33_current_run_gate.py --repo . --run-id GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519
python3 scripts/p33_sm_tq_settings_gate.py --repo .
python3 scripts/p33_gui_asset_gate.py --repo .
npm ci
npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
python3 scripts/p33_desktop_smoke_gate.py --repo . --receipt docs/codex-runs/GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519/desktop_smoke/final_desktop_smoke.json
bash scripts/p33_package_replay_gate.sh .
python3 scripts/p33_release_final_gate.py --repo . --run-id GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519
```

## Final response requirement

The final response must include changed files, commands run, pass/fail/skipped checks with reasons, release decision, blockers, rollback path, and exact next pass if needed.
