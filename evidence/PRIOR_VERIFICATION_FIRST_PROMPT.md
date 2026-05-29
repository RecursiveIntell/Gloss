# Verification-First Gloss Chat/Ollama Closing Pass

You are executing a narrow verification-first closing pass for Gloss. Do not broaden scope.

## Primary objective

Prove the exact branch that prevents Ollama chat from succeeding, then apply the minimum code repair. The symptom is: Ollama works outside Gloss, but Gloss still does not let Ollama respond.

## Hard rules

- Do not assume the root cause.
- Inspect current files before edits.
- Do not refix Ollama done-frame handling unless the new fake-provider test fails.
- Do not implement LAN provider support unless trace proves configured provider URL is LAN and validation blocks it.
- Every backend spawned chat exit must produce a frontend-clearable terminal event.
- Do not emit successful `chat:done` unless assistant persistence succeeded or a typed partial/cancellation artifact is emitted.
- End with changed files, commands run, tests passed/failed/skipped, trace result, rollback notes, and remaining blockers.

## Phase 0 — Preflight / evidence capture

1. Record git status, branch, commit, dirty files.
2. Capture provider config summary with secrets redacted: selected provider, selected model, provider URL class only (loopback/LAN/public/cloud/invalid), model registry entries.
3. Copy last ChatAttemptTraceV1 if present.
4. Inspect these files before editing:
   - `src/components/chat/ChatPanel.tsx`
   - `src/stores/sourceStore.ts`
   - `src/stores/chatStore.ts`
   - `src/App.tsx`
   - `src/lib/tauri.ts`
   - `src-tauri/src/commands/chat/mod.rs`
   - `src-tauri/src/providers/mod.rs`
   - `src-tauri/src/providers/ollama.rs`
   - `src-tauri/src/commands/settings.rs`
   - `src-tauri/src/db/notebook_db/mod.rs`

## Phase 1 — Add operator-visible diagnostics first

1. Add Settings/Chat controls:
   - Run Ollama provider smoke.
   - Copy last ChatAttemptTraceV1.
   - Copy redacted provider/model config summary.
2. Provider smoke must call existing `debug_chat_provider_smoke` with selected model.
3. UI must show: phase, provider, URL class, model, first_token_seen, done_seen, assistant_persisted, error.

## Phase 2 — Fix verified frontend pre-send blocker

1. Remove source-list loading/partial/error from hard send/input disable path.
2. Let `getSourceScope()` degrade to `{ kind: 'none' }`.
3. Add visible warning instead of blocking chat.

## Phase 3 — Fix terminal-event invariant

1. Audit every return path after the spawned chat task starts.
2. Add one terminal helper and enforce exactly one terminal event per stream attempt.
3. Cover GPU gate cancellation, LLM gate cancellation, notebook epoch changes, provider start timeout, first token timeout, stream idle timeout, incomplete stream, empty response, DB insert failure, and cancellation.

## Phase 4 — Fix active-event routing

1. Do not drop terminal chat events in `App.tsx` solely because payload notebook is not the active notebook.
2. Route events to `chatStore` first; match against active stream identity.
3. Keep active-view filtering only for display/toasts after lifecycle cleanup.

## Phase 5 — Fix persistence success semantics

1. Do not emit success done before assistant persistence or explicit partial/cancel artifact.
2. DB insert failure must become typed error/partial, not success.
3. Receipt fields must distinguish provider_stream_complete, assistant_persisted, partial_persisted, and persistence_failed.

## Phase 6 — Conditional provider policy repair

Only if trace proves provider_config_error from LAN URL:
- Add `allow_lan_local_providers`, default false.
- Allow private LAN only with opt-in.
- Reject public IPs/credentials/query/fragment.

If trace proves model_missing/default_model issue:
- Refresh model registry after provider save.
- Auto-select valid model only with user-visible notice, or block with explicit model-selection error.

## Phase 7 — Tests / gates

Required:
- `npm run build`
- `npm test`
- `cargo fmt --all -- --check`
- `cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests`
- `cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests`
- fake-provider stream test: token -> done=true -> no EOF -> backend terminalizes -> UI clears streaming
- cancellation/notebook-switch terminal event test
- DB insert failure no-success-done test
- source-list-error no-retrieval chat test
- live Ollama smoke or exact reason skipped

## Final report

Return:
1. Exact failing branch proved.
2. Whether Ollama received a request.
3. Whether first token was seen.
4. Whether done frame was seen.
5. Whether assistant was persisted.
6. Whether frontend cleared streaming.
7. Changed files.
8. Commands run.
9. Tests passed/failed/skipped.
10. Remaining blockers.
11. Rollback instructions.
