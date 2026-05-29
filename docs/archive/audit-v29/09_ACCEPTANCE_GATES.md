# Acceptance Gates

## Gate 0 — Evidence basis

- Current repo inspected before edits.
- Git/toolchain state recorded.
- Existing package sidecars and issue ledgers ingested.
- No release claim based on README/spec alone.

## Gate 1 — Run/package truth

- One run ID everywhere.
- Package scope gate passes.
- Fresh unzip replay passes.
- Stale artifacts quarantined.

## Gate 2 — Source-of-truth boundaries

- Canonical owner map exists.
- No duplicate durable truth store.
- No hidden compatibility layer.
- Canonical library semantics are not forked locally.

## Gate 3 — Receipts

Every material operation has a receipt path: ingestion, extraction, chunking, embedding, retrieval, prompt, generation, provider route, timeout, queue, export/import, repair, egress, redaction, packaging.

## Gate 4 — Broad spec complete

All feature matrix rows are closed by implementation + tests + receipts, not by “deferred” labels. Unsupported features require explicit capability-degraded UI and blocker, not hidden omission.

## Gate 5 — Semantic-memory/TurboQuant proof

- semantic-memory live smoke passes.
- Projection sources >0 in live fixture.
- TurboQuant exact proof exists or all TQ claims are demoted.

## Gate 6 — UI/API truth

Inspector Dock, footer/status, settings, answer evidence, and receipts agree. No fake compatibility UX.

## Gate 7 — Security/privacy

Cloud egress is opt-in, secrets are redacted, network/filesystem policies are explicit, and unreviewed secret warnings are gone.

## Gate 8 — Validation/release

All validation commands pass, or final handoff lists exact blockers. Release/public claims are permitted only after release_candidate, broad_spec, fresh_unzip, and public_claim gates pass.
