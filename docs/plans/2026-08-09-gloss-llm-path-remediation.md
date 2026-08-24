# Gloss LLM-Path Remediation Plan

> **For Hermes:** Execute only after the Phase 0 baseline is refreshed. This document is a plan, not a claim that the defects are fixed.

**Goal:** Make Gloss chat/provider execution observable, terminally correct, cancellation-safe, and test-proven across backend streaming and frontend recovery.

**Architecture:** Preserve the existing owners: Rust chat lifecycle in `src-tauri/src/commands/chat/`, provider contracts in `src-tauri/src/providers/`, and UI lifecycle in `src/stores/chatStore.ts` plus `src/App.tsx`. The backend remains authoritative for attempt state and terminal events; the frontend renders/replays that state and must not invent cancellation completion.

**Tech stack:** Tauri 2, Rust/Tokio, `CancellationToken`, React/TypeScript/Zustand, Vitest.

---

## Current evidence — observed 2026-08-09

- Repository snapshot at planning: `/home/sikmindz/Coding/Gloss`, `main`, `b81f3bc9806d76cb8289f07841ef1dd1b1f17466`; the tree had this untracked plan artifact. Refresh the source snapshot before Phase 0.
- `npm test` passed: 26 unit tests and frontend contract tests.
- `cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests` passed: 3 tests only.
- Required static validators for source-send, frontend event routing, terminal contract, and LAN policy passed.
- `python3 scripts/audit_chat_path_integration.py .` failed: it expects stale retrieval/evidence markers (`chat:evidence`, `GlossLocalMemoryBackend::new`, `citations: pendingEvidence`) that current source no longer uses.
- Current source proves assistant DB insertion precedes `chat:done` (`commands/chat/mod.rs:2639-2765`) and provider smoke/trace controls are present in Settings (`SettingsDialog/index.tsx:695-779`).
- Current source also proves `chatStore.stopStreaming` clears UI state in a `finally` block immediately after issuing `stop_chat` (`src/stores/chatStore.ts:305-327`), before a backend `chat:cancelled` receipt/event is observed. This is the highest-confidence remaining LLM lifecycle defect.

**Claim boundary:** historical ledgers are leads only. Any task below must re-prove its defect with a current RED test before modifying production code.

## Coverage contract

| ID | Current classification | Owner | Plan tasks | Acceptance evidence |
|---|---|---|---|---|
| LLM-01 | controller-verified | `chatStore.ts`, chat command | 1.1–1.3 | cancel receipt/event and UI test |
| LLM-02 | controller-verified gate failure | retrieval/chat audit script | 2.1–2.2 | semantic behavior audit passes |
| LLM-03 | coverage gap | provider + chat streaming | 3.1–3.4 | fake-provider integration matrix |
| LLM-04 | coverage gap | `settings.rs`, settings UI | 4.1–4.2 | stale-model deterministic fixture |
| LLM-05 | release-evidence gap | `validation/`, `scripts/` | 5.1–5.2 | rerunnable gate manifest |

## Phase 0 — freeze truth before repair

### Task 0.1: Capture an LLM-path baseline
**Owner/files:** no production edits. Create `evidence/llm-path/2026-08-09-baseline.md`.

**RED:** Run:
```bash
npm test
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
python3 scripts/audit_chat_path_integration.py .
```
Expected: current integration audit fails; preserve exact JSON.

**GREEN:** Record command, exit status, tool versions, HEAD, and unmodified-tree status. Do not edit code in this task.

**Gate:** `git status --short --branch` remains clean except the evidence artifact.

**Evidence:** raw command logs and baseline markdown.

**Migration:** none.

**Rollback:** delete only the newly created evidence projection.

**Claim:** establishes a reproducible baseline, not runtime correctness.

## Phase 1 — restore backend-authoritative cancellation

### Task 1.1: Define the cancellation terminal contract
**Owner/files:** modify `src-tauri/src/commands/chat/emit.rs`; test in its existing test module or `src-tauri/src/commands/chat/mod.rs` tests.

**RED:** Write a test that creates/records a cancellation terminal event and asserts exactly one terminal event, `kind == "cancelled"`, attempt/message identity, and replayability through `get_chat_events_since`.

**GREEN:** Add only typed cancellation payload fields needed by the test. Do not add a second event store or frontend-owned receipt.

**Gate:**
```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
```

**Evidence:** focused test log and serialized replay-event fixture.

**Migration:** additive payload fields only; old consumers tolerate missing optional fields.

**Rollback:** revert the isolated event-contract commit; retained DB/replay records are not deleted.

**Claim:** backend cancellation is represented by one replayable terminal event.

### Task 1.2: Make `stop_chat` return an acknowledged cancellation request
**Owner/files:** modify `src-tauri/src/commands/chat/mod.rs:450` and its tests.

**RED:** Fake an active attempt, call `stop_chat`, and assert it returns an attempt/message identity plus `cancellation_requested`; assert the actual terminal state is still emitted by the spawned task, not fabricated by the command.

**GREEN:** Return a typed acknowledgement from `stop_chat` without reporting completed/cancelled until the terminal emitter fires.

**Gate:** focused Rust cancellation test plus Phase 1 full chat test command.

**Evidence:** test trace showing request acknowledgement followed by exactly one terminal event.

**Migration:** update `src/lib/types.ts` and `src/lib/tauri.ts` atomically for the changed response shape.

**Rollback:** retain the existing command signature only behind an explicit versioned compatibility adapter if required; never silently treat acknowledgement as completion.

**Claim:** cancellation request and cancellation completion are distinguishable.

### Task 1.3: Remove frontend-local cancellation completion
**Owner/files:** modify `src/stores/chatStore.ts:305-327`; test `src/stores/__tests__/chatStore.test.ts`.

**RED:** Mock `stopChat` success with no terminal event. Assert `isStreaming` remains true with a `cancelling` status. Then inject `chat:cancelled` and assert cleanup. Add timeout fallback only if the backend acknowledgement cannot be observed; fallback must say `cancellation acknowledgement timed out`, not `generation stopped`.

**GREEN:** Make `stopStreaming` record request-in-flight state, preserve stream identity, and clear only in `handleChatCancelled`, `setStreamingError`, or `finalizeMessage`.

**Gate:**
```bash
npm test
python3 validation/validate_frontend_event_routing.py .
```

**Evidence:** Vitest output for cancel-request/no-terminal/terminal-arrives cases.

**Migration:** no persisted schema migration.

**Rollback:** revert store-only change; do not remove backend replay evidence.

**Claim:** the UI does not claim cancellation completion before backend truth.

## Phase 2 — replace stale static markers with behavior contracts

### Task 2.1: Classify each failed integration-audit assertion
**Owner/files:** inspect/modify `scripts/audit_chat_path_integration.py`; read `commands/chat/mod.rs`, `src/stores/chatStore.ts`, `src/lib/types.ts`.

**RED:** Add a fixture or unit-level parser test for the audit script proving that removed implementation names do not become requirements merely because old text expected them.

**GREEN:** Replace string-marker checks with structural/semantic checks for: no-retrieval fallback, emitted persisted replay evidence, current evidence payload type, and requested-vs-used backend disclosure where the feature exists. Remove checks for obsolete `GlossLocalMemoryBackend` and direct source-order assumptions only after source inspection proves replacement ownership.

**Gate:** `python3 scripts/audit_chat_path_integration.py .` exits 0 and fails when a deliberate fixture removes each required behavior.

**Evidence:** before/after JSON and negative fixtures.

**Migration:** none.

**Rollback:** restore the prior script and mark it stale; do not falsify a pass by weakening behavior assertions.

**Claim:** the audit validates current contracts, not historical spelling.

### Task 2.2: Add a retrieval-degradation chat regression
**Owner/files:** test current retrieval owner and `commands/chat/mod.rs`; frontend test only if UI behavior changes.

**RED:** Fake retrieval failure/empty context while provider is available. Assert the chat request proceeds with an explicit no-retrieval/degraded reason and does not disable sending.

**GREEN:** Make the minimum contract change only if the RED test exposes a real block or undisclosed fallback.

**Gate:** focused Rust test + `validate_source_send_gate.py .`.

**Evidence:** typed reason-code fixture and event/receipt output.

**Migration:** none.

**Rollback:** revert isolated fallback handling; preserve reason receipts.

**Claim:** degraded retrieval does not silently become unavailable chat.

## Phase 3 — prove streaming and timeout behavior through a fake provider

### Task 3.1: Introduce a test-only scripted `LlmProvider`
**Owner/files:** test support adjacent to `src-tauri/src/providers/mod.rs` or chat tests; do not alter production provider selection.

**RED:** A chat integration test cannot currently inject token/done/no-EOF/error scripts.

**GREEN:** Add a test-only provider fixture emitting deterministic sequences and respecting `CancellationToken`.

**Gate:** `cargo test ... commands::chat::tests`.

**Evidence:** fixture protocol documentation in test code.

**Migration:** test-only.

**Rollback:** delete the fixture only if all consumers are removed.

**Claim:** chat lifecycle can be tested without a live Ollama instance.

### Task 3.2: Cover terminal stream matrix
**Owner/files:** chat integration tests and frontend event/store tests.

**RED:** Add one failing test per case: token→done→no EOF; provider-start timeout; first-token timeout; stream-idle timeout; cancellation while waiting for gates; cancellation during stream; DB insert failure; notebook switch after token.

**GREEN:** For each case, assert one backend terminal event, correct typed phase/reason, no `chat:done` without durable assistant persistence, and frontend stream cleanup/replay behavior.

**Gate:** focused Rust + `npm test`.

**Evidence:** machine-readable matrix under `evidence/llm-path/`.

**Migration:** none.

**Rollback:** no production changes unless a failing case proves one.

**Claim:** enumerated scripted failure modes are regression-covered; this is not live-provider proof.

### Task 3.3: Add headed desktop smoke only after fake-provider matrix is green
**Owner/files:** create a documented manual/automated smoke procedure under `validation/`.

**RED:** no current proof that the webview receives backend events after remount/focus.

**GREEN:** exercise a real configured provider or clearly labeled local fake endpoint: normal answer, stop, notebook switch, reload/focus replay, and failed selected model.

**Gate:** saved trace plus visible UI capture for each branch.

**Evidence:** redacted `ChatAttemptTraceV1` and screenshots/logs.

**Migration:** none.

**Rollback:** quarantine failed smoke output; do not change release docs.

**Claim:** proves only the recorded environment/provider path.

## Phase 4 — provider/model readiness

### Task 4.1: Test default-model validity after model refresh
**Owner/files:** `src-tauri/src/commands/settings.rs`, DB/registry tests, `src/stores/settingsStore.ts` tests.

**RED:** refresh returns models where configured default is absent/stale; current UI has no deterministic remediation contract.

**GREEN:** choose one explicit behavior: preserve selection but block send with a typed `model_missing` error, or select a compatible model and emit a notice. Do not silently switch models.

**Gate:** Rust fixture + `npm test`.

**Evidence:** provider/model selection receipt or typed UI state fixture.

**Migration:** additive settings metadata if necessary.

**Rollback:** restore previous default only when it remains available; otherwise retain explicit blocked state.

**Claim:** stale model selection is surfaced before a chat request.

### Task 4.2: Separate reachability from selected-model chat smoke
**Owner/files:** `SettingsDialog`, `src/lib/tauri.ts`, settings command tests.

**RED:** connectivity success and selected-model chat failure are indistinguishable to a user.

**GREEN:** keep both controls/results separate, include phase/provider/model in redacted output, and ensure smoke is feature-flagged as currently designed.

**Gate:** component test plus backend smoke fixture.

**Evidence:** UI test and redacted trace sample.

**Migration:** none.

**Rollback:** hide only the experimental UI control; retain backend diagnostics.

**Claim:** operator can distinguish endpoint reachability from model chat readiness.

## Phase 5 — close evidence honestly

### Task 5.1: Run the LLM-path gate bundle
**Owner/files:** no production change; create `validation/run_llm_path_gates.sh` only if it invokes existing, named gates with strict timeout and JSON/log outputs.

**RED:** current tests can pass while the stale integration script fails.

**GREEN:** run the repaired integration audit, focused Rust matrix, frontend tests, `npm run build`, `cargo fmt --all -- --check`, and existing required validators.

**Gate:** bundle exits nonzero on any child failure and writes per-command status.

**Evidence:** timestamped gate manifest and raw logs.

**Migration:** none.

**Rollback:** delete generated logs only; do not rewrite historical receipts.

**Claim:** recorded gates passed on the recorded commit/environment.

### Task 5.2: Reconcile issue ledger and release language
**Owner/files:** `tables/MASTER_ISSUE_LEDGER.csv`, relevant evidence summaries only after gates pass.

**RED:** any closed issue lacks a current test/trace reference; any open issue lacks an owner/task.

**GREEN:** mark each item fixed, open, stale, or scope-discovery with current evidence links. Preserve historical findings rather than overwriting them.

**Gate:** ledger review plus `git diff --check`.

**Evidence:** remediation coverage table and gate manifest references.

**Migration:** none.

**Rollback:** revert ledger classification; historical logs remain immutable.

**Claim:** release/public wording reflects current evidence only.

## Final verification gauntlet

```bash
npm run build
npm test
cargo fmt --all -- --check
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
python3 validation/validate_source_send_gate.py .
python3 validation/validate_frontend_event_routing.py .
python3 validation/validate_chat_terminal_contract.py .
python3 validation/validate_provider_lan_policy.py .
python3 scripts/audit_chat_path_integration.py .
```

## Hard no list

- No rewrite of provider architecture before a RED reproduction.
- No silent model substitution, cancellation-success fiction, or terminal event suppression.
- No compatibility/shadow state store for frontend lifecycle.
- No release-ready claim from static tests alone.
- No change to security authority defaults while fixing chat flow.
