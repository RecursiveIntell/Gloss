---
name: source-of-truth-map
description: Identify canonical owners for SCR code, Codex control files, skills, tests, packaging policy, and release receipts.
---

Use this skill before edits.

Canonical owners:
- Tests: `tests/`.
- Release commands: `scripts/run_all_checks.sh`, `scripts/run_completion_checks.sh`.
- Codex active control: `.codex/`.
- Repo-local skills: `.agents/skills/`.
- Completion report: `docs/P31_COMPLETION_AUTOMATED_PHASES.md`.
- Historical stale control: `docs/codex-runs/archive/`.
