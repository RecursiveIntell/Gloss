# CLAUDE.md — Gloss Project Instructions

> Read SPEC-gloss.md first. Spec wins on conflict.

## Test Integrity Contract (NON-NEGOTIABLE)
- Do NOT change any test file to make tests pass.
- Only change tests if the test is actually wrong (violates SPEC or asserts incorrect behavior).
- If changing a test is truly required:
  - include a short justification citing SPEC + observed behavior
  - keep test intent strong (do not weaken assertions)

## Critical Spec Constraint
- No concurrent GPU inference. Assume only one GPU-backed model can run at a time.

## Scheduling + Notebook Contracts (NON-NEGOTIABLE)
### 1) Single-flight LLM/GPU gate
At most one inference request in-flight across:
- summaries
- chat streaming
- studio
Enforce via shared async gate in AppState:
- Semaphore(1) preferred, or async Mutex.
Counters are not concurrency control.

### 2) Chat preemption + grace window
On user message arrival:
- set `chat_grace_until = now + 15s`
- each new message resets timer
Summary loop must not start jobs while now < chat_grace_until.
If a summary is already running, chat waits until it completes (finish current summary first).

### 3) No summaries before notebook selection
Summary worker must idle until an active notebook is selected.
Backend exposes `set_active_notebook(Option<String>)`.
Frontend calls it on selection/unselect.

### 4) Notebook switching correctness
When active notebook changes:
- summary processing switches immediately
- jobs for other notebooks do not run
Implement cancellation if supported; otherwise soft-cancel via epoch/version checks + payload validation.

## Error Handling Rules
- Provider failures must not be appended into assistant content.
- Emit a distinct error event/state (e.g., chat:error) and render in UI.
- Add timeouts to provider calls to avoid indefinite hangs.

## Frontend Notebook Isolation
On notebook change:
- reset activeConversationId + messages + streaming state
- abort in-flight streams
- ignore tokens not matching current streaming message_id

## Build / Verify
Backend:
- cargo clippy -- -D warnings
- cargo test
Frontend:
- npm run build
Verification checklist:
- Startup idle with no notebook selected
- Summary running -> chat waits -> no overlap
- 15s grace blocks summaries; resets on new messages
- Notebook switch stops old work, starts new
- Pause/resume visible and functional
- Provider failure shows error UI, no nonsense assistant message
- Restart retains assistant messages
