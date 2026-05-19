---
name: p32r3-package-scope
description: Use to create a small runtime-debug package and avoid vendor-heavy Codex context.
---

Run `python3 scripts/p32r3_package_runtime_debug_context.py --run-id P32R3`. Ensure the package includes first-party source, scripts, receipts, and docs, but excludes vendor crates, target, node_modules, logs, and archives unless explicitly requested.
