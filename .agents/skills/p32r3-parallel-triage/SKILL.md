---
name: p32r3-parallel-triage
description: Use for P32R3 parallel read-only discovery with subagents over independent Gloss surfaces.
---

When triggered, read `P32R3_PARALLEL_AGENT_TASKS.csv`, spawn one subagent per row, wait for all results, and write `docs/codex-runs/P32R3/reports/SUBAGENT_FINDINGS_SUMMARY.md`. Do not start implementation until this summary exists. Each subagent must inspect only its scope unless it discovers a concrete cross-boundary dependency.
