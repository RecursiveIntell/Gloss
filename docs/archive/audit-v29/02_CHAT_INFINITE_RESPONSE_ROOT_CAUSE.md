# Chat Infinite Response Root Cause

## Finding

The LLM itself is not the likely culprit. The current Gloss chat runtime can keep the UI in a streaming state because the backend treats **HTTP stream EOF** as completion rather than treating the provider's **terminal done frame** as completion.

## Current observed backend contract

In `src-tauri/src/commands/chat/mod.rs`:

- `stream_chat_response` receives `ChatToken { token, done }`.
- If `done` is true, it sets `sent_done = true`.
- It does **not** break the loop on `done`.
- The emitted `chat:token` payload hardcodes `done: false`.
- `emit_chat_done(...)` is called only after the token loop exits.

This creates the failure mode:

```text
Ollama sends final frame: { done: true, ... }
Gloss records sent_done=true
Gloss keeps polling stream.next()
EOF is delayed / stream stalls / total body remains open
UI never receives terminal done event
User sees infinite generation
```

## Correct provider contract

For Ollama streaming APIs, the final chunk is the authoritative semantic terminal event. Usage fields are included in the final chunk where `done` is true. Therefore Gloss should terminate the generation state machine on `done=true`, not on response body EOF.

EOF remains useful for cleanup diagnostics, but it is not the user-visible completion boundary.

## Intended state machine

```text
Queued
→ WaitingForGate(optional)
→ ProviderStarting
→ Streaming
→ TerminalCompleted(provider_done_frame)
     OR TerminalPartial(first_token_timeout | stream_idle_timeout | provider_error | cancelled)
```

Hard rules:

1. A generation attempt has an `attempt_id` before provider execution begins.
2. Every attempt writes a start receipt.
3. Every material provider frame updates a bounded chunk digest / partial content record.
4. `done=true` causes immediate terminalization:
   - append final content if present;
   - capture usage/final metadata;
   - persist assistant message;
   - persist generation receipt;
   - emit terminal event;
   - break the loop.
5. EOF after done is cleanup-only.
6. If timeout/error/cancel happens after partial tokens, persist partial content and emit continuation plan.
7. UI must never invent durable assistant messages locally; it renders backend terminal state.

## Concrete backend patch direction

Replace the loop branch around `if done` with a terminal path:

```rust
if done {
    sent_done = true;
    terminal_cause = Some("provider_done_frame");
    done_frame_seen = true;

    // If token is non-empty, append and emit its final delta first.
    // Then emit exactly one terminal event.
    emit_chat_terminal_done(...).await?;
    break;
}
```

The emitted terminal payload must include:

```json
{
  "notebook_id": "...",
  "conversation_id": "...",
  "message_id": "...",
  "attempt_id": "...",
  "status": "completed",
  "terminal_cause": "provider_done_frame",
  "done_frame_seen": true,
  "eof_seen": false,
  "partial_persisted": true,
  "content_digest": "sha256:...",
  "receipt_id": "..."
}
```

## Required regression fixtures

1. **Done without EOF**
   - Mock provider yields `hello`, then `done=true`, then never returns EOF.
   - Gloss must finish within one second.
   - Receipt has `terminal_cause=provider_done_frame` and `eof_seen=false`.

2. **Token then idle timeout**
   - Mock provider yields two tokens, then stalls.
   - Gloss persists partial output and emits `TerminalPartial(stream_idle_timeout)`.

3. **Cancel after token**
   - User clicks stop after one token.
   - Backend cancels provider future, persists partial once, emits `cancelled` terminal state.

4. **Background gate preemption**
   - Summary job holds LLM gate.
   - Foreground chat preempts or receives a bounded clear error/status, not an invisible wait.

5. **Frontend lifecycle**
   - UI leaves streaming state only after backend terminal event.
   - UI never creates final assistant truth locally.
