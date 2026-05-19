---
name: codex-control-pack
description: Maintain and audit SCR Codex control files, including prompts, auto-injections, hooks, rules, skills, and phase runner receipts.
---

Use this skill when changing `.codex/`, `.agents/skills/`, phase prompts, hooks, rules, or runner scripts.

Checklist:
1. Keep `MASTER_AUTOMATED_COMPLETION.md` as the single entrypoint.
2. Keep `.codex/prompt_manifest.json` as phase-order source of truth.
3. Keep injections automatic under `.codex/auto_gates/`.
4. Run `python scripts/assert_codex_active_pack.py`.
5. Run `python .codex/tools/auto_phase_runner.py --dry-run --print-prompts --receipt .codex/runs/P0-completion/auto_phase_dry_run.json`.
6. Do not reintroduce manual operator-paste gates.
