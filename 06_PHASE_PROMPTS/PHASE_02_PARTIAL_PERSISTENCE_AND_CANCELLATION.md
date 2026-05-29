# Phase 02 — Partial Persistence and Cancellation

Tasks:
1. Add generation attempt/lifecycle persistence.
2. Persist partial content on timeout/error/cancel.
3. Make `stop_chat` backend-authoritative.
4. Remove frontend-local durable assistant creation on stop.
5. Add continuation lineage.

Acceptance:
- token then idle timeout creates persisted partial + receipt;
- stop creates one backend-sourced cancelled assistant state;
- continuation links to partial attempt.
