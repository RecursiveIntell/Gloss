# AGENTS.md — Gloss Sub-Agent Responsibilities

## Test Integrity Contract (NON-NEGOTIABLE)
- Do NOT modify tests to make them pass.
- Only modify tests if demonstrably wrong per SPEC.
- Any test change must include written justification and must not weaken coverage.

## Non-negotiable invariants
1) Single-flight LLM/GPU gate (no concurrent inference).
2) Chat preemption + 15s grace window (from user message arrival; resets on new message).
3) No summaries before notebook selection.
4) Notebook switching correctness (cancel or epoch/soft-cancel + filtering).
5) Provider errors are separate UI state (never inserted into assistant text).
6) Notebook-scoped frontend state resets on notebook change.

## Roles

### backend-scheduler (owner)
Scope:
- src-tauri/src/state.rs
- src-tauri/src/lib.rs (worker loop)
- src-tauri/src/commands for set_active_notebook
- minimal glue in src-tauri/src/commands/chat.rs for grace bump + gate usage
Responsibilities:
- Implement llm_gate, chat_grace_until, active_notebook_id, active_epoch.
- Enforce gating in summary loop.
- Enforce notebook selection gating and notebook switch behavior.
- Ensure pause/resume stops summary work immediately.

### backend-chat
Scope:
- src-tauri/src/commands/chat.rs
- DB persistence helpers used by chat
Responsibilities:
- Acquire llm_gate for full chat stream duration.
- Emit chat:error (or structured error) on provider failure.
- Persist assistant message at end of stream.
- Ensure token stream is consistent and cancel-safe.

### frontend-core
Scope:
- src/stores/*
- src/lib/tauri.ts
- src/components/layout/StatusBar.tsx
Responsibilities:
- On notebook selection change: call set_active_notebook and reset notebook-scoped stores.
- Abort streaming UI on notebook change.
- Token handler ignores stale message_id.
- Pause/resume control always visible and accurate.

### backend-providers (optional if needed)
Scope:
- provider HTTP code
Responsibilities:
- timeouts and robust error mapping (CUDA illegal memory access => clean error)

## Execution order for this bug batch
1) backend-scheduler
2) frontend-core
3) backend-chat
4) backend-providers (if required)
