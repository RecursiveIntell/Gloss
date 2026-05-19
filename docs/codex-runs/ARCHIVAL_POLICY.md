# Codex Run Archival Policy

`z.py` archives stale Codex-run prompts, tasks, handoffs, and evidence before normal packaging.
Normal `release-context`, `next-codex-context`, and `codex-run-full` packages exclude `docs/codex-runs/archive/` unless `--include-codex-archive` or `--mode audit-full` is explicit.
Existing archive manifests are not rewritten; new collisions are routed to fresh paths.
