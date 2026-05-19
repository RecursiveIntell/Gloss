---
name: phase-gate
description: Enforce automated phase boundary checks and prevent phase drift during SCR completion.
---

Use this skill at every phase transition.

Phase gates are automatic. They are loaded from `.codex/auto_gates/`, not pasted manually.

Checklist:
1. Confirm the previous phase's required commands ran.
2. Confirm receipts were emitted.
3. Confirm tests were not weakened.
4. Confirm no scope widening occurred.
5. Confirm the next phase is loaded from `.codex/prompt_manifest.json`.
