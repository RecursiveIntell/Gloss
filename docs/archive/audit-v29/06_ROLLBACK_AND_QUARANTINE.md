# Rollback and Quarantine Plan

## Before edits

```bash
git status --short
git branch --show-current
git rev-parse HEAD
mkdir -p docs/codex-runs/CLOSING_CHAT_FIX_$(date -u +%Y%m%dT%H%M%SZ)
git diff > docs/codex-runs/CLOSING_CHAT_FIX_*/pre_edit.diff || true
```

## Rollback handles

For each phase, create:

```text
docs/codex-runs/<RUN_ID>/phase_<N>_changed_files.txt
docs/codex-runs/<RUN_ID>/phase_<N>_commands.log
docs/codex-runs/<RUN_ID>/phase_<N>_rollback.md
```

## Quarantine rules

Quarantine instead of deleting:

- stale Codex run docs that are not current truth
- old generated packages/sidecars
- contradictory final receipts
- deprecated validation scripts

Do not quarantine current source files without replacement and passing tests.

## Immediate rollback commands

```bash
git diff --stat
git restore <file>
git restore --staged <file>
```

If a migration was added, provide a down/recovery note or DB backup path before applying.
