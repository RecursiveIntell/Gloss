# PHASE_012 — Decoding settings source audit

Track: `rc`
RC blocking state: `YES`

## Goal

Implement or verify the narrow change implied by this phase without broadening scope. This phase must produce evidence before proceeding.

## Files likely touched

src-tauri/src/**, src/**, validation/**, scripts/**, docs/codex-runs/** as scoped by phase

## Files forbidden unless justified

- Unrelated broad-spec feature files during RC phases.
- Canonical library internals unless the phase explicitly targets a canonical integration point and records owner decision.
- Old historical Codex run docs except to quarantine/archive them.
- Public README claim upgrades unless the required receipts exist.

## Preconditions

- Startup preflight completed.
- Current run ID identified.
- Dirty state inventoried.
- Previous phase acceptance gate passed or blocker recorded.

## Implementation tasks

1. Inspect current files before editing.
2. Write the minimal change for this phase only.
3. Add or update tests/fixtures/receipts required by this phase.
4. Preserve rollback path.
5. Update current run artifacts: changed files, commands, validation result, remaining delta.

## Validation commands

Run decoding settings unit tests, provider mapping tests, and validation/gloss_decoding_settings_gate.py.

## Acceptance gate

- Phase-specific test(s) pass.
- No stale run truth introduced.
- No hidden fallback introduced.
- Any material operation has a receipt or explicit non-material classification.
- Failed/skipped checks are recorded with reason.

## Rollback/quarantine action

If this phase fails, revert changed files from this phase or quarantine the new surface behind a disabled feature flag. Keep receipts explaining the failure.

## Final receipt update

Append this phase result to:

```text
docs/codex-runs/GLOSS_TOTAL_COMPLETION_RC_PROOF_SUPERPASS_20260526/COMMAND_RECEIPTS.jsonl
docs/codex-runs/GLOSS_TOTAL_COMPLETION_RC_PROOF_SUPERPASS_20260526/VALIDATION_RESULTS.md
docs/codex-runs/GLOSS_TOTAL_COMPLETION_RC_PROOF_SUPERPASS_20260526/REMAINING_DELTA.md
```
