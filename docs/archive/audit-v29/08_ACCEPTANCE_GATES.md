# Acceptance Gates

## Absolute release blockers

Gloss is **not closed** until all of these are true:

1. Chat completes immediately on provider `done=true`, even if HTTP EOF never arrives.
2. Partial output persists on timeout/error/cancel.
3. Stop/cancel is backend-authoritative and creates no duplicate messages.
4. Background LLM work cannot make foreground chat appear infinite.
5. Package scope gate passes from a fresh unzip.
6. Release candidate gate cannot hang and emits structured JSON.
7. Missing validation script references are fixed.
8. Rust fmt/check/test/clippy results are current and recorded.
9. Frontend build/test results are current and recorded.
10. Live or mock Ollama chat smoke receipt proves the user-reported bug is closed.
11. Live desktop smoke is release-grade or release claim is blocked.
12. TurboQuant and semantic-memory runtime claims have current receipts or claims are demoted.
13. Secret scan findings are triaged.
14. Public claims match receipts.

## Chat-specific gates

- `done_frame_seen=true` is sufficient to terminalize a completed generation.
- `eof_seen=false` is allowed for a completed generation if `done_frame_seen=true`.
- EOF without done is degraded/error unless provider contract explicitly permits it.
- Terminal event is emitted exactly once.
- UI leaves streaming state only on backend terminal event.
- Every terminal state has a receipt.

## Handoff gate

Final response must include:

- changed files;
- commands run;
- passing checks;
- failing checks;
- skipped checks with reasons;
- receipts/artifacts added;
- known risks;
- rollback plan;
- exact remaining blockers.
