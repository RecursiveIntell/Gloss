# AGENTS.md — current-code bug batch

## Non-negotiable rules
- Do not weaken tests to make the patch pass.
- Do not widen source scope implicitly.
- Do not remove the single-flight invariant for real model work unless the runtime contract is deliberately redesigned and tested.
- Do not hide background-work failures behind optimistic UI counts.
- Do not "solve" startup latency by deleting summary functionality; make it explicit/manual and correctly scheduled.

## Canonical source paths for this batch
- Frontend: `src/`
- Backend: `src-tauri/src/`

## Non-canonical trees to ignore
- `.files/`
- `s/`

If these trees must remain in the repo, they are **not** the patch target for this bug batch.

## Workstream ownership

### 1) `scope-contract`
**Owns**
- `src/stores/sourceStore.ts`
- `src/components/chat/ChatPanel.tsx`
- `src/lib/tauri.ts`
- `src-tauri/src/commands/chat.rs`
- `src-tauri/src/retrieval/context.rs`
- `src-tauri/src/retrieval/hybrid_search.rs`
- any shared types introduced for source scope

**Must deliver**
- explicit scope enum / payload contract
- `None` != `All`
- subset-scoped prompt assembly
- subset-scoped retrieval filtering
- tests for manifest/passages leakage

### 2) `scheduler-startup`
**Owns**
- `src/stores/notebookStore.ts`
- `src/App.tsx`
- `src/components/layout/StatusBar.tsx`
- `src-tauri/src/state.rs`
- `src-tauri/src/lib.rs`
- `src-tauri/src/commands/sources.rs`

**Must deliver**
- manual summary mode persisted in settings
- no auto-queue on notebook activation
- no auto-resume on startup in manual mode
- idle auto-queue disabled in manual mode
- truthful queue counts

### 3) `suggestions-latency`
**Owns**
- `src/components/layout/PanelLayout.tsx`
- `src/App.tsx`
- `src/stores/chatStore.ts`
- `src-tauri/src/commands/chat.rs`

**Must deliver**
- no uncached LLM suggestion generation on notebook open by default
- no auto-trigger on job completion by default
- cancellation / epoch / scheduler conformance if suggestion generation remains automatic in any mode

### 4) `background-provider-compat`
**Owns**
- `src/components/settings/SettingsDialog.tsx`
- `src/components/layout/StatusBar.tsx`
- `src-tauri/src/commands/sources.rs`
- `src-tauri/src/jobs/mod.rs`
- provider/config helpers as needed

**Must deliver**
- compatible model filtering and validation
- no background queueing against incompatible providers/models
- readiness UI based on the real background backend

### 5) `verification`
**Owns**
- tests touched by the above
- any new integration/unit tests needed for scope, startup, scheduler, and provider compatibility

**Must deliver**
- passing frontend build
- passing Rust tests/format in Codex environment
- new regression coverage for every P0 issue

## Patch sequence
1. `scope-contract`
2. `scheduler-startup`
3. `suggestions-latency`
4. `background-provider-compat`
5. `verification`

## Stop conditions
If a proposed patch:
- reintroduces all-source widening,
- auto-starts summaries on notebook activation,
- or lets suggested-question work block chat again,
it is not acceptable.
