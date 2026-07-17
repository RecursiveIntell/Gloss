# Phase 01 — Chat Stream Termination

Fix the core chat infinite-response bug.

Tasks:
1. Inspect `src-tauri/src/commands/chat/mod.rs` and provider stream code.
2. Terminate on provider `done=true`, not HTTP EOF.
3. Emit exactly one terminal event.
4. Preserve final-frame metadata.
5. Add done-without-EOF test.

Acceptance:
- mock stream with `done=true` and no EOF completes within one second;
- receipt records `done_frame_seen=true`, `terminal_cause=provider_done_frame`, `eof_seen=false`;
- frontend leaves streaming state.
