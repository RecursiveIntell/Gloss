# Gloss Five-Angle Hostile Audit Addendum — Executive Verdict

Generated UTC: 2026-05-27T23:20:32.748711+00:00

## Decision

**NO-GO for release or public-ready claims.** The latest post-run package improved some surfaces, but the Ollama/chat failure is still not verified closed.

The verified current failure pattern is not one single "Ollama is broken" bug. The five audits found a failure tree:

1. **Chat can be blocked before Ollama is called** because the UI disables send when `sourceListStatus` is `loading`, `partial`, or `error`, even though the source store already has a no-retrieval fallback.
2. **Terminal events can be dropped or suppressed** when active notebook state changes or backend cancellation paths return without `chat:done` / `chat:error`.
3. **Backend can emit success/done before durable assistant persistence**, which creates false completion and data-loss risk.
4. **Provider/model diagnostics exist but are not exposed to the operator**, so the app cannot distinguish wrong model, rejected URL, provider config error, first-token timeout, done-frame failure, or persistence failure from the UI.
5. **Release receipts/package validation are inconsistent**, including a final receipt claiming a gate passed while the gate results show failure.

## Scope actually inspected

- Latest extracted source package: `/mnt/data/Gloss-generic-rust-next-codex-context-20260527T064920Z.zip`
- Frontend chat/event/store code
- Backend chat command/provider code
- Provider validation/model registry/settings code
- DB message insertion and receipt surfaces
- Current run receipts and validation scripts
- Existing package sidecars and warnings

## Local validation run here

- `npm run build`: **PASS**
- `npm test`: **PASS**
- Rust/cargo checks: **not runnable in this sandbox** (`cargo` unavailable)
- Several Python validation/static gates: mixed PASS/FAIL/TIMEOUT; see `EVIDENCE/COMMAND_RESULTS.md`

## Highest-confidence fixes

1. Remove retrieval/source-list status as a hard chat send blocker.
2. Add terminal-event invariant for every backend spawned stream exit.
3. Route terminal events by active stream identity, not current notebook view.
4. Treat assistant persistence failure as error/partial failure, not success.
5. Expose existing provider smoke and last ChatAttemptTraceV1 in Settings/Chat.
6. Add explicit LAN provider opt-in only if trace proves configured URL is LAN.
7. Fix release/package receipt contradictions before any release claim.
