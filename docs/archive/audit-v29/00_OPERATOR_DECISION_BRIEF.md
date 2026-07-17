# Operator Decision Brief

## Decision

Run a **closing/hardening pass**, but make it verification-first and chat/Ollama centered. Do not run another broad super-pass before the chat branch is proven.

## Why

The current code inspection shows several verified defects that can create the observed symptom or hide its true cause:

1. Chat can be blocked before provider execution by `sourceListStatus`.
2. Backend spawned chat exits can return without a terminal event.
3. Frontend event listeners drop lifecycle events when the current active notebook differs from the event notebook.
4. Provider stream completion can be reported as done even if assistant persistence fails.
5. Operator-visible provider smoke / trace tools exist but are not exposed.
6. LAN Ollama is rejected by current provider policy unless loopback/tunnel is used.
7. Package/run-truth/validation gates remain broken.

## Non-goals for this pass

- Do not redesign all Gloss UX.
- Do not add new ingestion formats.
- Do not rework semantic-memory internals.
- Do not claim release-ready until live smoke and package gates pass.
- Do not turn LAN provider access on silently.

## Success definition

A local user can click **Run Ollama Chat Smoke**, receive a terminal trace with `first_token_seen=true`, `done_seen=true`, `assistant_persisted=true`, and the UI clears streaming. If it fails, the UI classifies the failure branch precisely and emits/copies `ChatAttemptTraceV1`.
