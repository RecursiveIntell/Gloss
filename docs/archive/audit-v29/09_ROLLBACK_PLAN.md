# Rollback and Quarantine Plan

## Before edits

```bash
git status --short
git branch --show-current
git rev-parse HEAD
mkdir -p codex/rollback
cp -a src-tauri/src/commands/chat codex/rollback/chat-before-$(date -u +%Y%m%dT%H%M%SZ)
cp -a src/stores codex/rollback/stores-before-$(date -u +%Y%m%dT%H%M%SZ)
```

## During phase work

- Commit or stash per phase only after phase gates pass.
- If a phase fails, do not continue; either repair within phase or revert that phase.
- Keep generated run artifacts under `docs/codex-runs/<run-id>/` or agreed run dir.

## Rollback commands

```bash
git diff --stat
git diff > codex/rollback/failed-pass.patch
git checkout -- <files touched in failed phase>
```

## Quarantine rules

Quarantine instead of deleting when:

- old run artifact is historical evidence;
- local implementation may be duplicate canonical semantics;
- research-derived behavior is unpromoted;
- secret-like material requires review;
- dependency/vendor files may be needed for reproducible cargo builds.

Quarantine record must include target, reason, missing evidence, owner, and review path.
