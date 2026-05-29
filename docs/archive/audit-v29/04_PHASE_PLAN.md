# Phase Plan

## P00 — Preflight and proof capture

Goal: prove current state before edits.

Outputs:
- `docs/codex-runs/<RUN_ID>/P00_PREFLIGHT.md`
- git state, branch, commit, dirty files
- active provider URL class, selected model, sourceListStatus
- last ChatAttemptTraceV1 if present

Gate: no code edits before P00 report exists.

## P01 — Operator-visible diagnostics

Goal: make the failing branch observable.

Implement:
- Run provider chat smoke button
- Copy last trace button
- Redacted provider config summary

Gate: failed or successful chat can produce trace from UI.

## P02 — Source-list no-retrieval chat unblock

Goal: remove verified pre-send block.

Implement:
- source list loading/partial/error no longer disables chat
- retrieval warning only

Gate: source-list error sends no-retrieval chat.

## P03 — Active stream event routing

Goal: stop dropping terminal lifecycle events.

Implement:
- forward chat events to chatStore regardless of active notebook
- chatStore filters by streaming identity

Gate: notebook switch during stream cannot orphan spinner.

## P04 — Backend terminal-event law

Goal: every spawned chat path clears frontend state.

Implement:
- terminal helper
- terminal event for gate cancellation, epoch cancellation, timeouts, provider errors, incomplete stream, empty response

Gate: static script + runtime tests pass.

## P05 — Persistence/receipt correctness

Goal: success means durable assistant state.

Implement:
- no done before DB insert
- DB insert failure emits partial/error
- split provider stream receipt from assistant persistence receipt

Gate: mock DB failure produces error/partial, not done.

## P06 — Provider/model correctness

Goal: selected model and network policy are explicit.

Implement:
- selected model validation
- provider smoke uses selected model
- optional LAN provider support only if trace proves needed or operator opts in

Gate: provider tests and live smoke pass or fail with precise branch.

## P07 — Retrieval/semantic-memory/TurboQuant truth surfaces

Goal: prevent feature claims from becoming hidden truth.

Implement:
- retrieval decision card per answer
- live receipts for dense indexing, semantic memory, TurboQuant if claimed
- demote UI labels when receipts absent

Gate: no feature label says enabled/proven without matching receipt.

## P08 — Package/release validation repair

Goal: make package replayable.

Implement:
- validation script inclusion
- package scope split
- CurrentRunTruthV1
- aggregate gate timeouts
- final receipt generated from gate JSON

Gate: fresh unzip replay returns structured pass/fail and no missing scripts.

## P09 — Final hardening and hostile handoff

Goal: close the run with receipts.

Implement:
- run full commands
- populate hostile handoff
- update public claim boundary

Gate: no release-ready claim unless all gates pass with live Ollama trace.
