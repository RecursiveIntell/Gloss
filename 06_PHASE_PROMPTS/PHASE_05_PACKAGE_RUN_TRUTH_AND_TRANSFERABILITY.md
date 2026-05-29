# Phase 05 — Package, Run Truth, Transferability

Tasks:
1. Add single RunTruthV1 source.
2. Add PackManifestV1 inside package root.
3. Add GitStateReceiptV1 and ToolAvailabilityReceiptV1.
4. Separate Codex context package from release/source package.
5. Preserve required receipts and sanitized evidence summaries.

Acceptance:
- fresh unzip package gate passes;
- no run truth drift across CURRENT_RUN, sidecars, final handoff;
- package contains no unrelated root docs in release mode.
