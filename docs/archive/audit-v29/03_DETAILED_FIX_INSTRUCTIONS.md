# Detailed Fix Instructions

## A. Frontend chat send gate

### Files

- `src/components/chat/ChatPanel.tsx`
- `src/stores/sourceStore.ts`

### Change

Remove `sourceListStatus === "loading" | "partial" | "error"` from:

- `handleSend` early return
- textarea/input disabled logic
- send button disabled logic

Keep source list status warnings. Use `getSourceScope()` unchanged; it already returns `{ kind: 'none' }` for non-ready states.

### Acceptance

A source-list error/partial/loading fixture still sends chat with source scope `{ kind: 'none' }`.

---

## B. Frontend event routing

### Files

- `src/App.tsx`
- `src/stores/chatStore.ts`
- `src/lib/events.ts` if terminal event type additions are needed

### Change

Do not return early from chat lifecycle listeners based on `activeNotebookId`. Forward the event to chatStore. In chatStore, ignore only if the event does not match the current active stream identity.

Display/toasts can remain active-notebook scoped, but stream cleanup cannot.

### Acceptance

Start chat, switch notebook, backend emits done/error/cancel for original notebook. `isStreaming` clears.

---

## C. Backend terminalization

### Files

- `src-tauri/src/commands/chat/mod.rs`

### Change

Add helper:

```rust
fn emit_chat_terminal(..., terminal: ChatTerminalKind, reason: Option<&str>) { ... }
```

Every spawned-task exit after `isStreaming` can be true must call it exactly once.

Patch locations include:

- GPU gate `Ok(None)`
- LLM gate `Ok(None)`
- `CHAT_CANCELLED_NOTEBOOK_SWITCH` suppression
- provider start timeout
- first-token timeout
- stream-idle timeout
- incomplete stream
- empty response
- DB insert failure

### Acceptance

Static validation script proves no raw `return;` in the spawned block without nearby terminal emit. Runtime test proves cancellation clears UI.

---

## D. Persistence before done

### Files

- `src-tauri/src/commands/chat/mod.rs`
- database receipt modules/migrations as needed

### Change

Current behavior logs assistant persistence failure then still emits evidence and done. Replace with:

- if assistant insert succeeds: persist prompt/generation receipts, emit evidence, emit done.
- if assistant insert fails: persist partial/error receipt if possible, emit `chat:error` or `chat:partial`, do not emit done.

### Acceptance

Mock DB failure test: `chat:done` is absent; `assistant_persisted=false`; UI clears with recoverable partial/error.

---

## E. Provider/model diagnostics

### Files

- `src/lib/tauri.ts`
- `src/components/settings/SettingsDialog/*`
- `src/components/chat/ChatPanel.tsx` or a new `ChatDiagnosticsPanel.tsx`
- `src-tauri/src/commands/chat/mod.rs`

### Change

Expose existing commands:

- `debug_chat_provider_smoke`
- `get_last_chat_attempt_trace`

Add UI buttons:

- Run Ollama chat smoke.
- Copy last trace.
- Copy provider config summary.

### Acceptance

Operator can produce `ChatAttemptTraceV1` from the UI after a failed or successful attempt.

---

## F. Conditional LAN local provider support

### Files

- `src-tauri/src/providers/mod.rs`
- `src-tauri/src/commands/settings.rs`
- settings DB migration
- settings UI

### Change

Add setting:

```text
allow_lan_local_providers=false
```

Update provider validation to accept private LAN hosts only when enabled.

### Acceptance

Tests:

- loopback accepted by default
- LAN rejected by default
- LAN accepted with opt-in
- public IP rejected even with opt-in
- credentials/query/fragment rejected

---

## G. Model registry repair

### Files

- `src/stores/settingsStore.ts`
- `src/components/settings/SettingsDialog/*`
- `src-tauri/src/providers/mod.rs`
- settings commands

### Change

On provider save/model refresh:

- verify selected/default model exists for provider
- if missing, auto-select only with visible notice, or block chat with explicit `selected_model_missing` error
- provider chat smoke uses selected model, not a hardcoded default

### Acceptance

Fixture: `qwen3:8b` missing but another Ollama model exists; Gloss does not silently try missing model.

---

## H. Package, validation, and run truth

### Files

- `z.py` / packaging scripts if in repo scope
- `Gloss/scripts/run_all_checks.sh`
- `Gloss/validation/*`
- `Gloss/docs/codex-runs/CURRENT_RUN.md`
- package sidecar generation

### Change

- Rename `gloss_secret_store_permissions_gate.py` to avoid secret-like exclusion or allowlist by digest.
- Add `CurrentRunTruthV1` and generate projections from it.
- Split Codex context package vs release source package.
- Add package scope gate for top-level path allowlist.
- Add aggregate validation child timeouts.

### Acceptance

Fresh unzip replay passes:

- no missing validation script references
- package scope gate passes
- current run sidecars agree
- final receipt equals gate results
