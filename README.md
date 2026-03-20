# Gloss Codex fix pack (2026-03-20)

## What this pack is
This is a **current-code bug-fix pack** for the extracted `bebop.zip` repository. It is intentionally focused on the problems that affect the shipped app today:
- source-selection scoping
- chat latency caused by background work
- summary startup behavior
- background provider compatibility
- queue diagnostics
- Codex patch safety

It is **not** a roadmap/spec pack.

## Files
- `AUDIT_FINDINGS.md` — narrative audit and the highest-confidence root causes
- `MASTER_ISSUE_MATRIX.md` — patchable issue matrix in Markdown
- `MASTER_ISSUE_MATRIX.csv` — same issue matrix in CSV form
- `PATCH_ORDER_AND_TEST_PLAN.md` — recommended sequencing and required regression tests
- `AGENTS.md` — workstream ownership and invariants for Codex
- `CODEX_HANDOFF_PROMPT.md` — ready-to-use Codex handoff prompt

## What was actually verified here
- `npm ci`
- `npm run build`

## What was not verifiable here
- `cargo fmt --check`
- `cargo test`
- Tauri desktop build/package

Reason: `cargo` is not installed in this container.

## Recommended use
Give Codex this entire pack, then start with `CODEX_HANDOFF_PROMPT.md`.
