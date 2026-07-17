# Fix Architecture

## 1. Chat lifecycle contract

Introduce a strict lifecycle model:

```text
queued
→ context_built | retrieval_degraded
→ provider_configured | provider_config_error
→ provider_started | provider_start_timeout
→ first_token_seen | first_token_timeout
→ provider_done_frame | incomplete_stream | stream_idle_timeout
→ assistant_persisted | assistant_persist_error | partial_persisted
→ terminal: done | error | cancelled | partial
```

### Terminal law

After frontend has set `isStreaming=true`, every backend path must produce exactly one frontend-clearable terminal event:

- `chat:done`
- `chat:error`
- `chat:cancelled`
- `chat:partial`

No `return;` in the spawned chat task is legal unless the terminal event has been emitted or no frontend stream was ever started.

## 2. Active stream identity

Add/normalize an `ActiveStreamState` in `chatStore`:

```ts
interface ActiveStreamState {
  notebookId: string;
  conversationId: string;
  messageId: string;
  startedAt: string;
  provider?: string;
  model?: string;
  terminal?: 'done' | 'error' | 'cancelled' | 'partial';
}
```

`App.tsx` must forward chat lifecycle events to `chatStore` regardless of `activeNotebookId`. `chatStore` decides whether they match `streamingNotebookId` / `streamingMessageId`. UI/toasts may still be active-notebook scoped.

## 3. Provider diagnosis contract

Add operator-visible diagnostics:

- Run selected-provider chat smoke.
- Copy last `ChatAttemptTraceV1`.
- Copy redacted provider config summary.
- Show provider URL class: `loopback`, `lan_private`, `public`, `invalid`, `redacted_unknown`.
- Show selected model availability.

## 4. Retrieval degradation contract

`sourceListStatus` is not a chat-send precondition. It only affects retrieval scope:

```text
loading | partial | error | idle -> SourceScope { kind: 'none' }
ready + selected/all -> requested scope
```

The UI must show a warning, not block chat.

## 5. Persistence and receipt contract

Provider stream completion is not user-visible completion. Durable message persistence must happen before `chat:done`.

Required artifacts:

- `ProviderStreamReceiptV1`
- `AssistantPersistenceReceiptV1`
- `PartialGenerationReceiptV1`
- `CancellationReceiptV1`
- `ChatAttemptTraceV1`

## 6. Provider/network model contract

Default local provider policy remains loopback. LAN support is explicit:

```text
allow_lan_local_providers=false by default
if true: allow RFC1918/private IPs and safe LAN hostnames
always reject: credentials, query strings, fragments, public IPs for local provider mode
```

Do not implement LAN until the diagnostics prove it is the failing branch or the user explicitly wants remote Ollama support.

## 7. Package/release contract

Separate package types:

- **Codex context pack:** may include selected `Libraries/` and evidence docs.
- **Gloss release/source package:** rooted at `Gloss/` only, with external dependency manifest.

Final receipt is generated from gate results, never hand-written independently.
