# Gloss Recovery Master Implementation Plan

> For Hermes: execute with `subagent-driven-development`, one work package at a time. Every package requires spec-compliance review, code-quality review, and its stated gate before the next dependent package begins.

Goal: Return Gloss to a reproducible, secure, truth-owned desktop application whose core notebook → ingest → index → retrieve → chat → persist workflow is proven from a clean checkout and packaged artifact.

Architecture: Retain Tauri 2, React 19, Zustand 5, Rust/Tokio, SQLite/FTS5, semantic-memory, usearch, and fastembed 5. Repair authority boundaries rather than replacing the stack. Commands/events, operation lifecycle, embedding/indexing, retrieval decisions, runtime health, and release verification each get one canonical owner.

Evidence basis:

- `/home/sikmindz/Coding/Gloss/docs/audits/2026-07-11-full-wiring-audit.md`
- Branch at audit: `perf-slowdown-fix-20260610`
- HEAD at audit: `93864835b21a52d4e504cb97c650d90abfc2f082`
- Current tree at audit: 57 dirty paths, 5,432 insertions, 1,665 deletions

Tech stack: Rust 2021, Tauri 2, Tokio, rusqlite/FTS5, semantic-memory, usearch, fastembed, React 19, Zustand 5, TypeScript, Vite, Vitest, Python validation scripts, GitHub Actions.

---

## 1. Current state and claim boundary

Verified pass:

- `npm run build`: passed; emitted a 654.39 kB main JavaScript chunk warning.
- `npm test`: passed 12 static source-text contract checks.
- `npm run test:unit`: passed 3 discovered files / 16 tests.
- `bash validation/run_all_gloss_repair_gates.sh .`: five static gates passed.
- `npm audit --omit=dev`: zero production npm advisories.

Verified fail:

- `cargo fmt --all -- --check`: failed because `src-tauri/src/commands/studio.rs:612-614` is malformed.
- `cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant`: failed with 38 parser errors.
- Full `npm audit`: one high and two low development-chain advisories.
- `cargo deny check advisories`: failed with actionable Rust advisories.

Not certified:

- Rust tests.
- Tauri desktop build.
- AppImage package.
- Live desktop launch.
- Provider cancellation.
- Import/index/retrieval/chat workflow.
- Restart/replay behavior.
- Clean-clone dependency resolution.

Until the final gate passes, the only safe claim is: “Gloss is under recovery; the frontend builds, but the audited working tree is not a buildable or release-certified desktop application.”

## 2. Non-negotiable rules

1. Preserve the user’s current dirty work before source edits.
2. No feature work until P0 and P1 recovery gates pass.
3. No compatibility shims or second truth stores.
4. No “PASS” from token-presence/static gates when compilation or runtime failed.
5. No terminal event before durable terminal state is persisted.
6. No retrieval downgrade without a typed, visible decision.
7. No model download or network egress without the governing policy/consent.
8. No old receipt can certify a different commit or dirty tree.
9. No package claim without building and launching that package.
10. Commit locally after each green work package; do not push without explicit instruction.

## 3. Dependency graph

Critical path:

`R0 preserve` → `R1 compile` → `R2 security baseline` → `R3 reproducible dependencies` → `R4 canonical verifier` → `R5 IPC contract` → `R6 lifecycle` → `R7 indexing/embedding` → `R8 retrieval/memory authority` → `R9 frontend ownership` → `R10 E2E` → `R11 package/release truth`

Parallel opportunities after R4:

- R5 IPC parity and R9 renderer parser tests touch mostly separate files.
- R7 embedding/indexing and R9 runtime-status frontend work can proceed in parallel after shared types are frozen.
- Documentation cleanup starts only after runtime architecture is settled.

---

# Phase R0 — Preserve and classify the current tree

## R0.1 Capture baseline receipts

Objective: Make every pre-repair byte recoverable.

Create:

- `docs/receipts/2026-07-11-recovery-baseline/HEAD.txt`
- `docs/receipts/2026-07-11-recovery-baseline/STATUS.txt`
- `docs/receipts/2026-07-11-recovery-baseline/DIFF.stat`
- `docs/receipts/2026-07-11-recovery-baseline/DIFF.patch`
- `docs/receipts/2026-07-11-recovery-baseline/UNTRACKED.sha256`
- `docs/receipts/2026-07-11-recovery-baseline/ENVIRONMENT.txt`
- `docs/receipts/2026-07-11-recovery-baseline/BASELINE-GATES.txt`

Steps:

1. Record branch, HEAD, timestamp, Rust/Node/npm/Python versions.
2. Save full `git status --short` and `git diff --stat`.
3. Save full tracked diff with binary markers.
4. Hash every untracked file without modifying it.
5. Record the exact failing and passing commands from the audit.
6. Create a temporary worktree at audited HEAD.
7. Verify `git apply --check` against the saved patch.
8. Record verification output.

Gate:

```bash
git apply --check docs/receipts/2026-07-11-recovery-baseline/DIFF.patch
sha256sum -c docs/receipts/2026-07-11-recovery-baseline/UNTRACKED.sha256
```

Expected: patch applies to audited HEAD; all untracked hashes verify.

Commit:

```bash
git add docs/receipts/2026-07-11-recovery-baseline docs/audits docs/plans
git commit -m "docs: preserve Gloss recovery baseline and master plan"
```

## R0.2 Build a dirty-path ownership ledger

Create:

- `docs/receipts/2026-07-11-recovery-baseline/PATH-OWNERSHIP.csv`

Required columns:

- path
- preexisting status
- subsystem
- intended change
- dependency package
- rollback unit
- validation gate

Classify into:

- build/dependencies
- chat lifecycle
- provider runtime
- Studio
- indexing/embedding
- semantic memory/retrieval
- DB/migrations
- frontend state
- validation/CI
- docs/evidence

Gate: every dirty path appears exactly once.

---

# Phase R1 — Restore a compilable baseline

## R1.1 Remove the malformed Studio insertion

Files:

- Modify: `src-tauri/src/commands/studio.rs:612-614`
- Inspect: `src-tauri/src/lib.rs:191-194`
- Inspect: `src/lib/tauri.ts`
- Inspect: `src/components/studio/QuizWidget.tsx`

Decision: remove `explain_quiz_question` from the stabilization baseline. It is malformed, unregistered, unwrapped, and unused. Reintroduce only after recovery as a complete four-layer feature.

RED test:

Create `validation/gloss_rust_source_integrity_gate.py` that fails on serialized source blobs such as `}\n\n#[tauri::command]` outside Rust strings.

Run:

```bash
python3 validation/gloss_rust_source_integrity_gate.py .
```

Expected before fix: FAIL pointing to `studio.rs:612`.

Implementation:

1. Delete only the malformed function blob.
2. Restore the normal closing brace and `studio_output_view` function boundary.
3. Do not add an IPC registration or UI button in this package.

GREEN:

```bash
python3 validation/gloss_rust_source_integrity_gate.py .
cargo fmt --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

Expected: integrity gate, format, and check pass or reveal the next real compiler errors. Fix only compile errors caused by the current recovery batch; log unrelated blockers.

Commit: `fix(build): remove corrupted Studio command insertion`

## R1.2 Compile both supported feature profiles

Files:

- Modify only compiler-error locations proven by R1.1.

Commands:

```bash
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features --features semantic-memory-backend
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features --features semantic-memory-turbo-quant
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features semantic-memory-turbo-quant -- -D warnings
```

Acceptance:

- Both profiles compile.
- Clippy has no warning promoted to error.
- No feature is claimed unless its profile was checked.

Commit: `fix(rust): reconcile recovery batch with active feature profiles`

## R1.3 Run the first real Rust test baseline

Commands:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::studio
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

Output receipt:

- `docs/receipts/recovery/R1-rust-test-baseline.txt`

Acceptance: full Rust suite passes. If failures expose semantic bugs, create explicit R1.x tasks rather than weakening tests.

---

# Phase R2 — Patch security blockers before handling untrusted data

## R2.1 Add a Rust dependency policy

Create:

- `deny.toml`

Configure:

- advisories: deny vulnerabilities and unmaintained crates unless explicitly waived with reason/date
- bans: report duplicates; deny wildcard dependencies
- licenses: allow only project-compatible licenses
- sources: crates.io plus explicitly approved pinned Git sources

Add CI-pinned `cargo-deny` invocation.

RED:

```bash
cargo deny check advisories bans licenses sources
```

Expected: FAIL on current advisories.

Commit: `build(security): add Rust dependency policy`

## R2.2 Patch direct archive/XML vulnerabilities

Files:

- Modify: `src-tauri/Cargo.toml:65,74`
- Modify: root `Cargo.lock`
- Test: portable import/archive extraction tests

Updates:

- `quick-xml` to at least 0.41.0
- `tar` to at least 0.4.45

RED tests:

1. XML fixture with abusive namespace declarations must fail within bounded memory/time.
2. Tar fixture with symlink + chmod target must not mutate outside extraction root.
3. Tar path traversal fixture must remain rejected.

GREEN:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant portable
cargo deny check advisories
```

Commit: `fix(security): harden XML and archive processing`

## R2.3 Patch transitive Rust advisories

Targets from audit:

- `anyhow >= 1.0.103`
- `crossbeam-epoch >= 0.9.20`
- `rustls-webpki >= 0.103.13` through owning dependency

Steps:

1. Identify inverse dependency for each with `cargo tree -i`.
2. Update the narrowest owning dependency.
3. Avoid broad unconstrained `cargo update`.
4. Re-run duplicate and feature trees.

Gate:

```bash
cargo deny check advisories
cargo tree --duplicates
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

Acceptance: no unwaived advisories.

Commit: `fix(deps): clear audited Rust advisories`

## R2.4 Patch frontend development advisories

Files:

- Modify: `package.json`
- Modify: `package-lock.json`

Action: update Vite to the patched 7.3.x line first. Do not mix Vite 8 migration into recovery.

Gate:

```bash
npm ci
npm audit
npm run build
npm run test:unit
```

Acceptance: no high advisory; build/tests pass.

Commit: `fix(deps): update Vite to patched release line`

---

# Phase R3 — Make dependency resolution reproducible

## R3.1 Select the canonical internal-crate source

Files:

- Create: `docs/dependency-source-policy.md`
- Modify later: `src-tauri/Cargo.toml`
- Modify later: `.github/workflows/ci.yml`

Decision order:

1. Exact crates.io versions if all transitive internal dependencies are published.
2. Pinned Git revisions if publication is incomplete.
3. Declared monorepo checkout if atomic cross-repo work is required.

Do not use undeclared `../../Libraries/*` paths in a standalone Gloss checkout. Do not switch to stale vendored copies.

Verification before choosing publication:

```bash
cargo search llm-pipeline
cargo search tauri-queue
cargo search semantic-memory
cargo metadata --format-version 1 --no-deps
```

Document availability and selected route.

## R3.2 Replace sibling path dependencies

Files:

- Modify: `src-tauri/Cargo.toml:28-29,58`
- Modify: root `Cargo.lock`
- Modify: `.github/workflows/ci.yml`

Steps:

1. Pin selected source and revision/version.
2. Remove local path assumptions.
3. Regenerate root lockfile.
4. Confirm no internal crate resolves from `/home/sikmindz/Coding/Libraries`.

Gate:

```bash
cargo metadata --format-version 1 --locked
cargo tree | grep '/home/sikmindz/Coding/Libraries' && exit 1 || true
```

Commit: `build: pin reproducible internal crate sources`

## R3.3 Eliminate lockfile split truth

Files:

- Remove or justify: `src-tauri/Cargo.lock`
- Keep authoritative: `/home/sikmindz/Coding/Gloss/Cargo.lock`

Recommended: delete the nested lockfile because root `Cargo.toml` owns the workspace.

Gate:

```bash
test ! -e src-tauri/Cargo.lock
cargo metadata --locked --format-version 1 >/dev/null
```

Commit: `build: use one workspace lockfile`

## R3.4 Prove a clean clone

Create script:

- `scripts/verify_clean_clone.py`

Behavior:

1. Clone/copy tracked source into a temporary directory.
2. Ensure no sibling `Libraries` directory is present.
3. Run `npm ci --ignore-scripts`, Cargo metadata, frontend build, and Rust check.
4. Emit JSON receipt with commit/tree hash and command results.

Gate:

```bash
python3 scripts/verify_clean_clone.py --repo . --receipt docs/receipts/recovery/R3-clean-clone.json
```

Acceptance: clean checkout resolves and checks without hidden local state.

Commit: `test(build): prove clean-clone dependency resolution`

---

# Phase R4 — Replace false-green validation with one canonical verifier

## R4.1 Fix Vitest discovery

Files:

- Modify: `vitest.config.ts:3-6`
- Verify: `src/lib/evidenceContract.test.ts`

RED:

```bash
npm run test:unit -- --reporter=verbose
```

Expected before change: only 3 files; evidence contract excluded.

Implementation: include both `src/**/__tests__/**/*.{ts,tsx}` and `src/**/*.test.{ts,tsx}` without double counting.

GREEN: expected at least 4 test files, including `evidenceContract.test.ts`.

Add a test-inventory assertion script so future tests cannot silently disappear.

Commit: `test(frontend): include all Vitest test files`

## R4.2 Make `npm test` behavioral

Files:

- Modify: `package.json`

Recommended scripts:

- `test:unit`: `vitest run`
- `test:contracts`: existing static script
- `test`: `npm run test:unit && npm run test:contracts`

Gate:

```bash
npm test
```

Expected: executes both behavioral and static suites.

Commit: `test(frontend): make default test command behavioral`

## R4.3 Implement canonical verifier

Create:

- `scripts/verify.sh`
- `validation/schemas/verification_receipt_v1.schema.json`

Modify:

- `validation/run_all_gloss_repair_gates.sh`
- `validation/README.md`
- `.github/workflows/ci.yml`

Ordered stages:

1. source integrity
2. lock consistency
3. frontend typecheck/build
4. frontend unit tests
5. frontend static contracts
6. Rust format
7. Rust check for both profiles
8. Clippy
9. targeted lifecycle tests
10. full Rust tests
11. static policy gates
12. npm audit policy
13. cargo deny
14. clean-clone check
15. package build when enabled
16. desktop smoke when enabled

Rules:

- Fail fast.
- Preserve every stage’s stdout/stderr.
- Overall PASS only if all required stages pass.
- Static scans are labeled `static_contract`, never `runtime`.
- Receipt includes commit, tree hash, dirty status, environment, commands, exit codes, durations, and artifact hashes.

RED test: introduce a temporary malformed Rust fixture and prove verifier returns nonzero; clean it up immediately.

Gate:

```bash
bash scripts/verify.sh --no-package --receipt docs/receipts/recovery/R4-verification.json
```

Commit: `ci: establish build-bearing canonical verification`

---

# Phase R5 — Establish one typed IPC and event contract

## R5.1 Freeze the current contract inventory

Create:

- `schemas/tauri-contract-v1.json`
- `validation/verify_tauri_contract.py`

Inventory:

- command name
- request fields and casing
- response type
- error type/reason codes
- registration status
- frontend wrapper
- caller count
- event name
- payload schema
- sequence scope
- terminal/nonterminal status

Seed from audited counts: 76 frontend invokes, 77 registrations, with known unexplained items.

## R5.2 Resolve command drift

Inspect:

- `src-tauri/src/lib.rs`
- `src/lib/tauri.ts`
- command modules

Decisions:

- Remove or wire `compare_memory_backends` based on actual product need.
- Keep Quiz Explain deferred; no partial command.
- Every public command must have definition + registration + wrapper + caller or an explicit operator-only designation.

RED/GREEN:

```bash
python3 validation/verify_tauri_contract.py .
```

Commit: `refactor(ipc): enforce command parity`

## R5.3 Resolve event drift

Inspect/runtime trace:

- `chat:stream_event`
- `sources:folder_scan`
- `queue:job_completed`
- all chat terminal events

Do not delete `queue:job_completed` until `tauri-queue` runtime emission is checked.

Add tests that subscribe, trigger fixture operations, and assert payload schema and terminal ordering.

Acceptance:

- Zero unexplained emit/listen mismatches.
- Event terminal semantics are encoded in schema.

Commit: `refactor(events): enforce typed event parity`

---

# Phase R6 — Fix operation ownership, cancellation, and persistence ordering

## R6.1 Introduce a shared operation context

Files:

- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/providers/mod.rs`
- Create or modify shared operation types module

Type fields:

- operation kind
- notebook ID
- conversation/output/source ID as applicable
- epoch
- attempt ID
- cancellation token
- provider/model authority snapshot
- phase deadlines
- receipt sink
- terminal state guard

Tests:

- one terminal outcome only
- drop without terminal state emits internal failure receipt
- stale epoch is rejected
- cancellation token propagates

Commit: `refactor(runtime): add shared operation context`

## R6.2 Register chat attempt before side effects

Files:

- Modify: `src-tauri/src/commands/chat/mod.rs:449-462,680,2203-2264`

RED tests:

1. Second concurrent send is rejected before user message persistence.
2. Stop during retrieval cancels before provider call.
3. Stop during provider setup cancels.
4. Rejected attempt leaves no orphaned normal user turn.
5. Exactly one terminal receipt/event exists.

Implementation order:

1. Validate notebook/conversation.
2. Register attempt.
3. Recheck epoch.
4. Persist or transactionally stage user turn.
5. Perform retrieval/context/provider phases with cancellation.
6. Persist assistant/partial/cancel state.
7. Emit terminal event.

Gate:

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
```

Commit: `fix(chat): register attempts before persistence and retrieval`

## R6.3 Make Studio state notebook-owned

Files:

- Modify: `src/stores/studioStore.ts`
- Modify: `src/stores/notebookStore.ts`
- Test: `src/stores/__tests__/studioStore.test.ts`

Replace module-global promise with `{ notebookId, attemptId, promise }` or a keyed map. Track `loadedNotebookId`. Reject stale `loadOutputs` completion. Reset active output/export receipt during notebook activation.

RED tests:

- Notebook A late load cannot overwrite B.
- B generation cannot join A promise.
- A cancellation cannot cancel B.
- stale export receipt does not appear in B.

Commit: `fix(studio): scope generation and output state by notebook`

## R6.4 Classify queue jobs by resource policy

Files:

- Modify: `src-tauri/src/lib.rs:219-401`
- Modify: `src-tauri/src/jobs/mod.rs`

Define job traits/metadata:

- requires active notebook
- obeys summary pause
- obeys chat grace
- requires GPU
- requires LLM
- requires embedding runtime

Tests:

- pausing summaries does not pause indexing/media metadata
- IndexChunks does not acquire LLM gate
- summarization obeys chat grace
- stale notebook jobs cancel correctly

Commit: `fix(queue): schedule jobs by resource class`

---

# Phase R7 — Establish one embedding and dense-index truth

## R7.1 Define canonical embedding configuration

Files:

- Modify: `src-tauri/src/state.rs:395-561`
- Modify: `src-tauri/src/commands/settings.rs`
- Modify: `src-tauri/src/commands/sources/mod.rs:693-729`
- Modify: `src-tauri/src/memory/semantic_memory_adapter.rs`
- Modify: `src-tauri/src/jobs/mod.rs`

Configuration fields:

- provider: fastembed or Ollama if both are truly supported
- model ID
- dimensions
- cache directory
- download consent
- network endpoint/egress decision
- revision/version fingerprint

Rules:

- Native ingestion, semantic-memory projection, jobs, diagnostics, and reindexing consume the same snapshot.
- Setting changes invalidate cached embedder and incompatible indices.
- No hard-coded `allow download = true`.

Tests:

- selected provider is the one invoked
- no-consent cached load succeeds
- no-consent uncached load fails without network
- settings change invalidates old runtime
- dimensions mismatch requires rebuild

Commit: `refactor(embedding): establish one runtime configuration`

## R7.2 Fix cache-root ownership

Files:

- Modify: `src-tauri/src/memory/semantic_memory_adapter.rs:76-100`

Use the same `hf_hub` cache resolver for preflight and model loading, or explicitly pass one configured cache path to both.

Tests:

- cached/no-consent
- uncached/no-consent
- consented download

Commit: `fix(embedding): use one model cache root`

## R7.3 Define one canonical dense-index artifact

Files:

- Modify: `src-tauri/src/state.rs:731,840-841`
- Modify: `src-tauri/src/jobs/mod.rs:229-359`
- Modify: `src-tauri/src/db/portable.rs:558-560,638-640`
- Modify: DB doctor/index metadata code

Create a shared path function/constant for `embeddings/chunks.usearch`.

Commit protocol:

1. Generate/update index in temporary artifact.
2. fsync/save successfully.
3. Atomically replace canonical artifact.
4. Update chunk embedding IDs and metadata transactionally.
5. Mark ready only after durable artifact exists and validates.

Tests:

- job and synchronous path produce the same artifact path
- failure before save does not mark ready
- export/import preserves canonical artifact
- doctor detects metadata/artifact mismatch

Commit: `fix(index): use one durable dense-index artifact`

## R7.4 Decide the IndexChunks lifecycle

Decision gate:

- If background indexing is needed for UX, wire it explicitly after chunk persistence.
- If synchronous indexing is acceptable and simpler, delete the dead job variant/executor.

Recommended: wire background indexing only if it can be proven with queue integration and cancellation; otherwise remove it now.

Required integration test if retained:

1. Import fixture.
2. Observe queued `IndexChunks` row.
3. Worker processes it.
4. Canonical artifact exists.
5. Metadata is ready.
6. Retrieval returns fixture chunk.

Commit: `fix(index): wire or remove IndexChunks lifecycle`

## R7.5 Remove or explicitly retain ORT

Files:

- Modify: `src-tauri/Cargo.toml:53`

Probe:

```bash
cargo tree -i ort
```

If Nomic v2 MoE is the only required path, remove `ort-download-binaries-native-tls`, clean build, load model, embed fixture, and package. If reranking requires ORT, retain it and document/test it as intentional.

Acceptance: docs and dependency tree agree; no “Candle-only” claim while ORT resolves.

Commit: `build(embedding): align fastembed features with runtime usage`

---

# Phase R8 — Collapse semantic-memory and retrieval shadow truth

## R8.1 Write the retrieval authority state matrix

Files:

- Test: `src-tauri/src/features.rs`
- Test: memory/backend status tests

Matrix dimensions:

- compiled capability
- embedding availability
- index status
- requested retrieval scope
- active strategy: semantic/hybrid/dense/BM25/source-order/raw
- fallback allowed
- fallback reason
- TurboQuant codec enabled/validated

Expected output: one `RetrievalDecisionV1` and one status payload.

## R8.2 Remove preview/profile contradiction

Files:

- Modify: `src-tauri/src/features.rs:363-368`
- Modify: `src-tauri/src/state.rs:365-369`
- Modify: `src-tauri/src/memory/*`
- Modify: settings UI/commands
- Add DB migration

Recommended architecture:

- semantic-memory is the retrieval service.
- BM25, dense, hybrid, source-order, and raw are strategies/fallbacks.
- Remove “semantic-memory-preview” as a backend identity.
- Remove unconditional availability helpers.
- Migrate obsolete `memory_backend` settings once with receipt.

Tests:

- old setting migration
- unavailable embedding → explicit fallback
- no fabricated frontend backend state
- TurboQuant status reflects actual codec activation, not dependency presence

Commit: `refactor(memory): establish one retrieval service authority`

## R8.3 Fix explicit source-scope decisions

Files:

- Modify: `src/stores/sourceStore.ts:16-47`
- Modify: `src-tauri/src/commands/chat/*`
- Modify: evidence types/UI

Request must carry:

- user-requested mode
- selected IDs
- source-list health
- expected source count

Backend returns:

- proceed
- degrade with disclosed reason
- refuse because requested sources are unavailable

No frontend conversion from explicit request to `{kind: 'none'}`.

Tests:

- user chooses none → free chat
- explicit source missing → refuse/disclose, never silently free chat
- partial source list → backend decision
- all sources unavailable → typed failure/degradation

Commit: `fix(retrieval): preserve user source-scope intent`

## R8.4 Fix chat replay cursor ownership

Files:

- Modify: `src/stores/chatStore.ts:35,75,146-150,480-495`
- Modify backend only if sequence scope changes

Backend currently uses a global sequence, but frontend still needs restart/reset handling and stream ownership. Store cursors by notebook/conversation plus backend epoch/generation, or formalize a global cursor with a restart epoch. Prefer keyed cursors for clear ownership.

Tests:

- switch A→B→A
- two notebooks
- duplicate events
- pruned buffer
- backend restart with sequence reset
- terminal event replay after app restart

Commit: `fix(chat): make replay cursor ownership explicit`

---

# Phase R9 — Repair frontend authority and broken renderers

## R9.1 Remove localStorage as source-selection authority

Files:

- Modify: `src/stores/sourceStore.ts:53-81,426-473`
- Modify: `src/stores/notebookStore.ts:79-102`

Use explicit notebook ID in selection actions and per-notebook debounce queues. On activation, flush or cancel old notebook writes before switching authority.

Tests:

- delayed A selection never writes to B
- rapid switch preserves both notebooks’ selections
- failed persistence leaves visible error and retryable state

Commit: `fix(sources): scope selection persistence by notebook`

## R9.2 Make provider/model selection transactional

Files:

- Modify: `src/stores/settingsStore.ts`
- Modify: `src/components/chat/ChatPanel.tsx:258-268`
- Modify backend settings command if atomic operation is absent

Create `selectProviderModel(providerId, modelId)` that validates pair compatibility and persists atomically in one backend transaction. Derive displayed model from canonical settings; remove independently mutable `activeModel`.

Tests:

- both settings commit
- validation failure commits neither
- backend failure rolls back UI
- model belongs to selected provider

Commit: `fix(settings): make provider-model selection atomic`

## R9.3 Fix Timeline and DataTable parsing

Files:

- Modify: `src/components/studio/TimelineView.tsx`
- Modify: `src/components/studio/DataTableView.tsx`
- Modify: `src/components/studio/StudioPanel.tsx` fallback handling
- Test: renderer/parser tests
- Fixture: Rust-generated serialized `StudioArtifact`

RED: valid backend fixture currently renders empty state.

Implementation: parse `artifact.content.entries` and `artifact.content.{columns,rows}`. Support root-level legacy shape only if persisted fixtures prove it exists. Malformed structured output must fall back to useful generic rendering, not false empty state.

Commit: `fix(studio): parse backend StudioArtifact envelopes`

## R9.4 Correct dead/misplaced UX actions

Files:

- Modify: `src/components/chat/ChatPanel.tsx`
- Modify: `src/stores/studioStore.ts`
- Modify: `src/components/studio/StudioPanel.tsx`

Tasks:

1. Move Continue from user messages to latest assistant/partial output.
2. Bind continuation to conversation/message context.
3. Remove Studio `streaming` phase and copy unless real progress events are implemented.
4. Keep Quiz Explain absent until complete end-to-end implementation.

Tests: rendered action ownership and phase transitions.

Commit: `fix(ui): align actions and progress labels with runtime behavior`

## R9.5 Centralize runtime status

Create:

- `src/stores/runtimeStatusStore.ts`
- `src/stores/__tests__/runtimeStatusStore.test.ts`

Modify:

- `src/components/layout/StatusBar.tsx`
- `src/components/inspector/DiagnosticsPanel.tsx`

Requirements:

- deduplicated requests
- per-resource timestamp
- stale/loading/error state
- cancellation on notebook/provider switch
- backend events preferred
- slow visibility-aware fallback polling
- cheap reachability distinct from model smoke test
- no fabricated backend fallback objects

Tests: two subscribers generate one request; stale response cannot overwrite new notebook state.

Commit: `refactor(status): centralize runtime health ownership`

---

# Phase R10 — Build behavior-level integration and desktop proof

## R10.1 Add deterministic provider fixtures

Create:

- `tests/fixtures/mock_provider_server.py` or Rust test server
- request/stream fixtures for success, delayed start, idle timeout, error, cancel, malformed frame

Use loopback only. Record requests and cancellation observation.

## R10.2 Add backend workflow integration test

Test:

1. Create temporary app data root.
2. Create notebook.
3. Import deterministic text source.
4. Chunk/embed/index.
5. Retrieve expected phrase.
6. Start chat through mock provider.
7. Verify citation/evidence.
8. Verify assistant persistence precedes done event.
9. Restart state and load conversation.

Gate: one command executes the complete backend workflow.

## R10.3 Add race/cancellation suite

Scenarios:

- chat cancelled during retrieval
- chat cancelled during provider start
- chat cancelled during stream
- notebook switch during chat
- concurrent send collision
- Studio cross-notebook generation
- queue summary pause with indexing still active
- settings change invalidates embedding runtime

Every scenario asserts one durable terminal outcome.

## R10.4 Add desktop smoke harness

Modify:

- `scripts/gloss_desktop_smoke_harness.py`

Create:

- `validation/schemas/gloss_desktop_smoke_receipt_v2.schema.json`

Workflow:

1. Launch built desktop application with isolated data directory.
2. Create notebook.
3. Import fixture.
4. Wait for canonical index readiness.
5. Run retrieval-backed chat.
6. Run explicit no-retrieval chat.
7. Cancel a generation.
8. Switch notebooks.
9. Restart app.
10. Verify persisted data and replay.
11. Export/import notebook.
12. Exit cleanly.

Receipt must include observed assertions, not just process exit.

Commit sequence:

- `test(e2e): add deterministic provider fixtures`
- `test(e2e): prove core backend workflow`
- `test(e2e): cover lifecycle races and cancellation`
- `test(desktop): prove packaged workflow smoke`

---

# Phase R11 — Harden egress, packaging, performance, and release truth

## R11.1 Revalidate provider redirects and resolved addresses

Files:

- Modify: `src-tauri/src/providers/mod.rs:289-395`

Tests:

- allowed loopback endpoint
- redirect loopback → public rejected
- redirect public → private rejected
- hostile hostname resolving outside approved class rejected
- query/userinfo/fragment rejection retained
- explicit LAN consent honored only for approved private classes

Implementation: disable redirects by default or install a redirect policy that validates every target. Resolve local hosts and guard against rebinding between validation and connection where feasible.

Commit: `fix(security): enforce egress policy across redirects and DNS`

## R11.2 Tighten package scope

Files:

- Modify: `validation/gloss_package_scope_gate.py`
- Modify: Tauri bundling configuration

Reject package inclusion of:

- `target`
- `node_modules`
- `.claude`
- `.hermes`
- Codex archives
- reference/evidence archives not required at runtime

No-manifest must be failure, not success.

Test package inventory against allowlist.

Commit: `fix(package): enforce minimal runtime artifact scope`

## R11.3 Build and launch AppImage

Commands:

```bash
npm run tauri:build:release
python3 validation/gloss_installer_smoke_gate.py --repo . --build
python3 scripts/gloss_desktop_smoke_harness.py --repo .
```

Record:

- artifact path
- SHA-256
- size
- linked native dependencies
- launch result
- workflow receipt

No release claim without this receipt.

## R11.4 Measure and split frontend bundle

Baseline: 654.39 kB main JS chunk.

Files:

- Modify: `vite.config.ts`
- Lazy-load Studio renderers, D3 mind map, diagnostics, receipt inspector, settings, markdown-heavy views.

Measure:

- main chunk
- total transfer/gzip
- cold launch
- first interactive action
- panel-open latency

Do not merely increase `chunkSizeWarningLimit`.

Commit: `perf(frontend): split heavy panels and renderers`

## R11.5 Reconcile active truth and archive pollution

Files:

- Modify: `AGENTS.md`
- Modify: `docs/codex-runs/CURRENT_RUN.md`
- Archive/index: stale active-looking audit and release receipts
- Rewrite: `README.md`
- Create: `docs/CAPABILITY_MATRIX.md`
- Create: final hostile-auditor handoff

Capability states:

- planned
- compiled
- unit-tested
- integration-tested
- desktop-smoked
- packaged

Repository cleanup:

- Hash/index tracked ZIP/7z/generated manifests.
- Move immutable bulky artifacts to release assets/evidence storage or dedicated branch.
- Remove stale vendor snapshots only after proving no active references.
- Do not rewrite historical evidence; remove it from active truth surfaces.

Commit: `docs: reconcile Gloss capability and release truth`

---

# Phase R12 — Deferred improvements after certification

Only after R11 is green:

1. Quiz Explain complete four-layer feature.
2. Multi-angle query rewriting with measured retrieval benefit.
3. Audio/TTS via isolated sidecar/CLI artifact contract.
4. Slide deck and infographic outputs.
5. Vite 8/plugin-react 6/react-markdown 10 evaluation.
6. TypeScript 7 evaluation.
7. Additional platform packages beyond the proven target.

Each requires its own plan and benchmark/acceptance criteria. None belongs in recovery commits.

---

# Full verification gauntlet

Run from a clean clone with no sibling Libraries checkout unless declared by policy:

```bash
npm ci
npm audit
npm test
npm run build

cargo fmt --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features --features semantic-memory-backend
cargo check --manifest-path src-tauri/Cargo.toml --no-default-features --features semantic-memory-turbo-quant
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets --features semantic-memory-turbo-quant -- -D warnings
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo deny check advisories bans licenses sources

bash validation/run_all_gloss_repair_gates.sh .
python3 scripts/verify_clean_clone.py --repo .
bash scripts/verify.sh --receipt docs/receipts/final-verification.json

npm run tauri:build:release
python3 validation/gloss_installer_smoke_gate.py --repo . --build
python3 scripts/gloss_desktop_smoke_harness.py --repo .
```

Final acceptance:

- clean working tree except final receipt outputs intended for commit
- one authoritative Cargo lockfile
- no hidden sibling dependency requirement
- no unwaived high/critical npm or Rust advisories
- all frontend tests discovered and run
- both Rust feature profiles check
- full Rust suite passes
- typed IPC/event parity passes
- import/index/retrieval/chat/cancel/restart/export-import workflow passes
- AppImage builds and launches
- README/capability claims match receipts

# Claim boundary by milestone

After R1: “The Rust backend compiles and its current tests run.” Not release-ready.

After R4: “The project has one build-bearing verification command.” Not runtime-certified.

After R7: “Embedding and dense indexing use one configured, durable artifact path.” Not desktop-certified.

After R9: “Frontend state ownership and renderer contracts pass behavioral tests.” Not packaged.

After R10: “Core workflows pass deterministic integration and desktop smoke tests.” Package claim still requires R11.

After R11: “The audited artifact was built, launched, and completed the recorded workflow on the recorded environment.” Do not generalize to all platforms/providers without separate receipts.

# Hard no list

- No wholesale rewrite.
- No Electron migration.
- No new Studio output types during recovery.
- No TTS/native inference runtime added to the main process.
- No restoring stale vendored crates as hidden active truth.
- No frontend-generated backend status defaults presented as fact.
- No silent retrieval fallback.
- No broad dependency-major upgrade batch.
- No deleting evidence before hashing/indexing it.
- No push, release, or public “fixed” claim without explicit instruction and final receipts.
