# Detailed Fix Instructions by Root Cause

## Root Cause Group 1 — Chat is blocked before provider execution

**Fix:**
- In `ChatPanel.tsx`, remove `sourceListStatus === loading|partial|error` from `handleSend`, input `disabled`, and send button `disabled`.
- Keep a visible retrieval degradation warning.
- Send `getSourceScope()`; it already returns `{ kind: 'none' }` for non-ready source states.

**Acceptance:** source-list error/partial/loading never blocks no-retrieval chat.

## Root Cause Group 2 — Terminal events are not guaranteed

**Fix:**
- Add backend `emit_chat_terminal(...)` helper.
- Every spawned chat task exit after streaming state begins must call exactly one terminal event: `chat:done`, `chat:error`, or `chat:cancelled`.
- Do not suppress `CHAT_CANCELLED_NOTEBOOK_SWITCH` without emitting a frontend-clearable event.

**Acceptance:** static gate proves no early return after stream start without terminal emission; notebook-switch cancellation clears UI.

## Root Cause Group 3 — Active notebook filtering drops terminal events

**Fix:**
- In `App.tsx`, forward `chat:token`, `chat:error`, and terminal status to `chatStore` regardless of current active notebook.
- Let `chatStore` decide whether payload matches `streamingNotebookId` and `streamingMessageId`.
- UI display/toasts can still respect active notebook, but lifecycle cleanup cannot.

**Acceptance:** switch notebook mid-stream; terminal event still clears the original stream state.

## Root Cause Group 4 — Done is emitted before durable persistence is proven

**Fix:**
- Backend must persist assistant message or partial/error artifact before success done.
- DB insert failure must emit `chat:error` or `chat:partial_failed`, not `chat:done`.
- Generation receipts must separate provider stream completion from assistant persistence.

**Acceptance:** mock DB insert failure produces no successful assistant done event.

## Root Cause Group 5 — Provider/model diagnosis is not operator-visible

**Fix:**
- Expose existing `debugChatProviderSmoke` and `getLastChatAttemptTrace` in Settings and/or Chat.
- Show redacted provider URL class, selected model, phase, first_token_seen, done_seen, assistant_persisted, and error.
- Provider Test must not be treated as Chat Smoke.

**Acceptance:** a failed user chat can be classified as provider_config_error, model_missing, provider_start_timeout, first_token_timeout, stream_idle_timeout, done_missing, persistence_failed, or frontend_event_dropped.

## Root Cause Group 6 — Conditional LAN provider support

**Fix only if trace proves LAN URL is used:**
- Add `allow_lan_local_providers`, default false.
- Allow RFC1918/private IPs only when opt-in is enabled.
- Reject public IPs, credentials, query strings, and fragments.

**Acceptance:** explicit tests for loopback default, LAN rejected default, LAN accepted with opt-in, public/credential/query/fragment rejected.

## Root Cause Group 7 — Release/package proof drift

**Fix:**
- Final receipt must be generated from gate results.
- Fresh package must include every script referenced by validation commands.
- Package scope gate must pass from fresh unzip.
- Current run truth must be one source projected to all sidecars.

**Acceptance:** `FINAL_RECEIPT.json.release_candidate_gate_passed == RELEASE_CANDIDATE_GATE_RESULTS.json.release_candidate_gate_passed`; no missing validation script; no unrelated top-level package paths.
