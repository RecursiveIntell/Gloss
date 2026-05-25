# AGENTS.md — Gloss P34 Runtime Truth and Ingestion Repair

You are working on Gloss, a local-first Tauri + React RAG notebook app.

## Non-negotiable rules

- Current source files outrank prior run receipts, README claims, and old Codex artifacts.
- Do not claim release readiness while active validation scripts are missing, stale, path-broken, or contradicted by CURRENT_RUN.md.
- Do not treat semantic-memory, TurboQuant, dense retrieval, BM25, source-order fallback, or provider-only answers as interchangeable.
- Every user-visible answer must disclose requested backend, effective backend, fallback/degradation, citation validity, source scope integrity, and receipt ID.
- No source-scope widening. Invalid explicit source IDs resolve to no/partial scoped sources, not all sources.
- No raw UUID floods in normal UI. Full IDs belong only in collapsed diagnostics / copied JSON.
- Notebook/import jobs must not run against missing or superseded notebooks.
- Semantic-memory projection must be chunk/token-budgeted and failure must be per source/chunk, not global false failure.
- Do not add broad new product features before fixing P0-P6 blockers.

## Current pass id

Use `GLOSS_P34_RUNTIME_TRUTH_INGESTION_REPAIR_20260524` unless the operator explicitly changes it.

## Required final report

List changed files, commands run, tests passed/failed/skipped with reasons, invariant decisions, release decision, rollback path, unresolved risks, and exact next pass.
