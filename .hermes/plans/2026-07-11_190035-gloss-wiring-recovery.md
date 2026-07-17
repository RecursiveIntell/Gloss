# Gloss Wiring Recovery Implementation Plan

> For Hermes: use subagent-driven-development to implement this plan task-by-task. Preserve the user’s dirty tree and require two-stage review for each lane.

Goal: Restore Gloss to a reproducible, truth-owned, end-to-end working desktop application before adding more features.

Architecture: Keep Tauri 2 + React 19 + Zustand. Consolidate command/event contracts, operation lifecycle, backend selection, runtime status, and verification into single authoritative paths. Remove shadow truth rather than layering compatibility shims over it.

Tech stack: Rust 2021, Tauri 2, Tokio, SQLite/FTS5, semantic-memory, usearch, React 19, Zustand 5, TypeScript, Vite, Vitest.

Evidence basis: `/home/sikmindz/Coding/Gloss/docs/audits/2026-07-11-full-wiring-audit.md`

---

## Phase 0 — Freeze and preserve the current integration batch

Objective: Make the 57-path dirty tree recoverable before changing source.

Files:
- Create: `docs/receipts/2026-07-11-recovery-baseline/STATUS.txt`
- Create: `docs/receipts/2026-07-11-recovery-baseline/DIFF.stat`
- Create: `docs/receipts/2026-07-11-recovery-baseline/DIFF.patch`
- Create: `docs/receipts/2026-07-11-recovery-baseline/COMMANDS.md`

Steps:

1. Capture `git status --short`, `git diff --stat`, `git diff`, HEAD, branch, toolchain versions, and checksums.
2. Create a local safety branch or stash only with explicit preservation labels; do not reset or discard user changes.
3. Classify every dirty path into build baseline, chat, provider runtime, Studio, semantic memory, DB, frontend, gates, or docs.
4. Record pre-repair failures: Rust parse/check/format and npm audit.
5. Commit only the receipts and audit/plan if the user wants an audit checkpoint; do not mix source repair into that commit.

Acceptance:

- Every pre-existing dirty path is represented in the preserved patch.
- `git apply --check DIFF.patch` succeeds against the recorded HEAD in a temporary worktree.

## Phase 1 — Restore a compilable baseline

Objective: Remove malformed source and make format/check the first hard gate.

Files:
- Modify: `src-tauri/src/commands/studio.rs:612-614`
- Modify if retained: `src-tauri/src/lib.rs:191-195`
- Modify if retained: `src/lib/tauri.ts`
- Modify if retained: Studio/Quiz UI call site
- Test: Rust Studio command tests

Steps:

1. Add a regression check that rejects literal escaped newline sequences in Rust source outside string literals.
2. Decide whether `explain_quiz_question` belongs in this stabilization release.
3. Recommended: remove the malformed, unregistered feature from the baseline lane; reintroduce it later with TDD.
4. If retained, rewrite as valid Rust, register it, add a typed frontend wrapper, and add an invocation test.
5. Run `cargo fmt --all -- --check`.
6. Run `cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant`.
7. Run targeted Studio tests.
8. Commit: `fix(build): restore parsable Studio command module`.

Acceptance:

- Rust parser, format, and check pass.
- No command exists in only one of definition/registration/wrapper/use layers without an explicit internal-only designation.

## Phase 2 — Make dependency resolution reproducible

Objective: Build Gloss from a clean checkout without `/home/sikmindz/Coding/Libraries` already present.

Files:
- Modify: `src-tauri/Cargo.toml`
- Modify: `Cargo.lock`
- Modify: `.github/workflows/ci.yml`
- Delete/archive after proof: inactive `src-tauri/vendor/*` snapshots
- Create: `docs/dependency-source-policy.md`

Steps:

1. Write a clean-clone test that confirms all Cargo dependency sources are available from the checkout/network contract.
2. Choose canonical delivery for `llm-pipeline`, `tauri-queue`, and `semantic-memory`:
   - preferred exact published versions;
   - otherwise pinned Git revisions;
   - otherwise a declared monorepo checkout.
3. Replace undeclared sibling paths.
4. Regenerate and inspect `Cargo.lock`.
5. Make CI use the same source route as developers.
6. Prove a clean temporary clone can run Cargo metadata/check.
7. Archive inactive vendor trees only after hash/index preservation and zero active refs.
8. Commit: `build: make canonical Libraries dependencies reproducible`.

Acceptance:

- Clean clone resolves Cargo metadata and checks without external sibling directories.
- One canonical source exists per internal crate.

## Phase 3 — Replace false-green gates with one canonical verifier

Objective: A green command must mean the application builds and its core contracts pass.

Files:
- Modify: `package.json`
- Modify: `validation/run_all_gloss_repair_gates.sh`
- Create: `scripts/verify.sh`
- Modify: `.github/workflows/ci.yml`
- Modify: `validation/README.md`

Steps:

1. Add a test proving the verifier exits nonzero when Rust source does not parse.
2. Change `npm test` to run Vitest plus frontend contract tests, or rename scripts so semantics are unambiguous.
3. Implement `scripts/verify.sh` with fail-fast ordered stages:
   - npm clean install/lock consistency in CI;
   - frontend typecheck/build;
   - Vitest;
   - static frontend contracts;
   - Rust format;
   - Rust check with release features;
   - targeted lifecycle tests;
   - full Rust tests;
   - policy/static gates;
   - npm audit policy;
   - Rust advisory/license policy;
   - Tauri package build;
   - desktop smoke where display/runtime is available.
4. Emit machine-readable stage receipts with command, exit code, duration, and artifact digests.
5. Make CI call only the canonical verifier plus platform-specific smoke jobs.
6. Commit: `ci: make canonical verification build-bearing`.

Acceptance:

- Deliberate syntax/test failures make local and CI verification red.
- Static gates cannot emit overall PASS when compile/test stages fail.

## Phase 4 — Establish one typed IPC/event contract

Objective: Eliminate definition/registration/wrapper/listener drift.

Files:
- Create: `schemas/tauri-contract-v1.json` or equivalent Rust-owned schema
- Modify: `src-tauri/src/lib.rs`
- Modify: `src/lib/tauri.ts`
- Modify: `src/lib/events.ts`
- Test: contract parity test

Steps:

1. Encode each command’s name, request, response, errors, and availability.
2. Encode each event’s name, payload, sequence scope, and terminal semantics.
3. Add parity tests for Rust command definitions, Tauri registrations, TypeScript wrappers, emitters, and listeners.
4. Runtime-trace `queue:job_completed` before deciding whether the external queue crate satisfies the listener.
5. Either wire or remove `chat:stream_event`, `sources:folder_scan`, and `compare_memory_backends` based on product need.
6. Generate TypeScript types/wrappers where practical.
7. Commit: `refactor(ipc): establish one command and event contract`.

Acceptance:

- Zero unexplained command/event drift.
- Runtime smoke observes every required terminal event family.

## Phase 5 — Unify operation lifecycle and cancellation

Objective: Chat, Studio, ingestion, indexing, and jobs share the same attempt/epoch/deadline rules.

Files:
- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/providers/mod.rs`
- Modify: `src-tauri/src/commands/chat/*`
- Modify: `src-tauri/src/commands/studio.rs`
- Modify: `src-tauri/src/jobs/mod.rs`
- Modify: relevant frontend stores

Steps:

1. Add tests for one terminal outcome per attempt: completed, failed, cancelled, partial, timed out.
2. Create one `OperationContext` with notebook ID, epoch, attempt ID, cancellation token, provider/model authority, deadlines, and receipt sink.
3. Enforce a single gate acquisition order across all inference paths.
4. Key frontend active Studio attempts by notebook/attempt instead of one module-global promise.
5. Make terminal receipts persist before terminal events emit.
6. Add crash/restart replay tests.
7. Commit by operation family, keeping each commit green.

Acceptance:

- Cancellation reaches provider work.
- Notebook switching cannot contaminate another notebook.
- Every attempt has exactly one durable terminal truth.

## Phase 6 — Fix chat replay and source-scope authority

Objective: Make replay and retrieval intent explicit and race-safe.

Files:
- Modify: `src/stores/chatStore.ts`
- Modify: `src/stores/sourceStore.ts`
- Modify: `src-tauri/src/commands/chat/*`
- Modify: retrieval decision types/receipts
- Test: frontend store and backend contract tests

Steps:

1. Determine and document whether chat event sequence IDs are global or conversation-scoped from DB schema/query behavior.
2. If scoped, replace `lastChatEventSeq` with a keyed cursor map.
3. Test switch-away/back, concurrent conversations, duplicate events, gaps, and restart replay.
4. Replace silent `{kind:'none'}` downgrades with a typed requested-scope plus source-health request.
5. Make backend return proceed/degrade/refuse decision with reason code.
6. Render scope decision in chat evidence.
7. Commit: `fix(chat): make replay and source scope authoritative`.

Acceptance:

- Retrieval is disabled only by user choice or an explicit disclosed backend decision.
- No replay is skipped because another conversation advanced an unrelated cursor.

## Phase 7 — Collapse semantic-memory shadow truth

Objective: One runtime owner decides retrieval/index behavior and reports it.

Files:
- Modify: `src-tauri/src/features.rs`
- Modify: `src-tauri/src/state.rs`
- Modify: `src-tauri/src/memory/*`
- Modify: settings commands/UI
- Modify: status/diagnostics UI
- Add migration for obsolete settings

Steps:

1. Write a state matrix test covering compiled availability, selected mode, active strategy, fallback, index health, and TurboQuant codec state.
2. Adopt recommended model: semantic-memory is the service; BM25, dense, hybrid, source-order are strategies.
3. Remove unconditional preview helper functions and obsolete preview/profile flags.
4. Migrate legacy `memory_backend` settings once, recording a migration receipt.
5. Return one backend decision/status payload from Rust.
6. Remove frontend-manufactured backend fallback objects; render unavailable/stale explicitly.
7. Commit: `refactor(memory): establish one retrieval authority`.

Acceptance:

- Build flags, settings, active runtime strategy, and UI report cannot disagree.
- Every fallback has a backend-issued reason and receipt.

## Phase 8 — Centralize runtime health and status

Objective: Stop components from independently polling and synthesizing overlapping truth.

Files:
- Create: `src/stores/runtimeStatusStore.ts`
- Modify: `src/components/layout/StatusBar.tsx`
- Modify: `src/components/inspector/DiagnosticsPanel.tsx`
- Modify: backend status event emission
- Test: status-store dedup/cancellation/staleness tests

Steps:

1. Add tests showing one request per resource despite multiple subscribers.
2. Separate cheap reachability checks from model smoke tests.
3. Centralize queue, provider, model, memory, embedding, and source-stat timestamps/errors.
4. Prefer backend change events; retain slow visibility-aware fallback polling.
5. Cancel stale polls on notebook/provider change.
6. Commit: `refactor(ui): centralize runtime health ownership`.

Acceptance:

- Opening Diagnostics does not double provider/memory polling.
- UI shows last-updated and stale/error states without fabricated defaults.

## Phase 9 — Build real end-to-end tests

Objective: Prove the workflows users depend on, not just static strings.

Files:
- Create: `tests/e2e/` harness and fixtures
- Modify: `scripts/gloss_desktop_smoke_harness.py`
- Modify: CI platform matrix

Test workflows:

1. Create notebook.
2. Import fixture document.
3. Observe ingestion/index terminal state.
4. Query retrieval and verify cited fixture chunk.
5. Chat with retrieval.
6. Chat with explicit no-retrieval mode while source loading is degraded.
7. Cancel chat and Studio generations.
8. Switch notebooks mid-operation.
9. Restart app and replay durable terminal state.
10. Export/import notebook and verify data/index integrity.
11. Exercise local provider with a deterministic mock server.
12. Package and launch AppImage.

Acceptance:

- Tests run from a clean clone and fresh data directory.
- Failures retain logs, receipts, DB snapshot, and event trace.

## Phase 10 — Security and dependency refresh

Objective: Clear known vulnerabilities without destabilizing baseline repair.

Files:
- Modify: `package.json`, `package-lock.json`, Cargo policy/config
- Modify: CI

Steps:

1. Upgrade Vite 7.3.3 to the patched 7.3.x line and verify.
2. Apply compatible patch/minor updates for Tauri JS packages, React, Tailwind, Vitest, Zustand, and related types.
3. Run `npm audit` and document any exception.
4. Add `cargo-deny` or `cargo-audit` with pinned CI installation/version.
5. Evaluate major upgrades separately: Vite 8, plugin-react 6, react-markdown 10, TypeScript 7.
6. Do not add TTS yet.
7. Commit patch/minor updates separately from each major migration.

Acceptance:

- No unhandled high/critical advisories.
- Canonical verifier remains green after each dependency batch.

## Phase 11 — Performance and UX hardening

Objective: Improve measured startup/runtime behavior after correctness is stable.

Files:
- Modify: `vite.config.ts`
- Modify: route/panel imports
- Add benchmark receipts

Steps:

1. Measure current cold launch, first interaction, memory, IPC frequency, import latency, retrieval latency, and first token.
2. Lazy-load Studio, D3, diagnostics, receipts, settings, and markdown-heavy views.
3. Split vendor chunks intentionally.
4. Move blocking DB/embed work behind `spawn_blocking` where traces prove contention.
5. Re-measure against the same workload.

Acceptance:

- Bundle warning is resolved by real splitting, not threshold suppression.
- Performance claims include before/after receipts and identical workloads.

## Phase 12 — Reconcile docs, evidence, and release truth

Objective: Make public/operator documentation match the active build.

Files:
- Rewrite: `README.md` capability/architecture/dependency sections
- Archive: obsolete active-looking audit/fix documents
- Create: generated/current capability matrix
- Create: release hostile-auditor handoff

Steps:

1. Generate a capability matrix with states: planned, compiled, configured, runtime-verified, packaged.
2. Remove absent dependency and feature claims.
3. Update semantic-memory terminology to match Phase 7.
4. Consolidate `EVIDENCE/evidence`, `VALIDATION/validation`, run logs, and audit indexes without deleting unindexed proof.
5. Move bulky immutable artifacts out of the active source tree after digest/index preservation.
6. Produce final clean-clone, verify, package, and E2E receipts.

Acceptance:

- README claims are traceable to current manifests and runtime receipts.
- No current document says PASS when the canonical verifier is red.

## Mandatory issue-to-phase additions from parallel hostile review

The following are not optional refinements; they must be absorbed into the phases above:

1. Phase 1: repair or remove malformed Quiz Explain command, then add definition/registration/wrapper/UI parity tests.
2. Phase 3: expand Vitest discovery to include `src/**/*.test.{ts,tsx}` and assert the expected test-file count.
3. Phase 5: register chat attempts before user-message persistence, retrieval, or provider setup; make early cancellation possible and prevent rejected orphan turns.
4. Phase 5: classify queue jobs by resource requirements. Summary pause/grace must not suspend indexing/media work, and non-LLM jobs must not acquire the LLM gate.
5. Phase 6: remove localStorage as source-selection notebook authority; key debounce and in-flight persistence explicitly by notebook.
6. Phase 6: make provider/model selection one transactional setting operation and remove mutable `activeModel` shadow state.
7. Phase 7: create one embedding runtime configuration used by native ingestion, semantic-memory, jobs, diagnostics, and reindexing; propagate model-download consent.
8. Phase 7: define one canonical dense-index artifact path and commit protocol. Either really enqueue `IndexChunks` or remove the dead lifecycle.
9. Phase 9: add Rust-generated StudioArtifact fixtures proving Timeline/DataTable renderers parse `artifact.content`.
10. Phase 10: patch `anyhow`, `crossbeam-epoch`, `quick-xml`, `tar`, and `rustls-webpki`; add `cargo deny` policy and archive extraction adversarial tests.
11. Phase 10: remove or justify the stale nested `src-tauri/Cargo.lock` and verify one dependency graph.
12. Phase 10: test whether fastembed’s ORT feature can be removed. Do not claim Candle-only packaging while `cargo tree -i ort` still resolves ORT.
13. Phase 10: disable redirects or revalidate every target and resolved address under provider egress policy.
14. Phase 11: fix Timeline/DataTable payload parsing before any additional Studio output type is built.
15. Phase 12: reconcile the three conflicting active-run identifiers and ensure old release receipts cannot certify a dirty/newer tree.

## Explicit deferrals

Defer until the entire recovery plan is green:

- Audio/TTS output types.
- Slide deck and infographic outputs.
- Quiz Explain feature if removed in Phase 1.
- Multi-angle query rewriting.
- Major framework rewrites.
- Vite/TypeScript major upgrades.

These are not the reason the current app is broken.

## Final completion gate

Run from a clean clone:

```bash
npm ci
bash scripts/verify.sh
npm run tauri:build:release
python3 scripts/gloss_desktop_smoke_harness.py --repo .
```

Completion requires:

- clean-clone dependency resolution;
- frontend build and all tests;
- Rust format/check/tests with release features;
- security/policy gates;
- packaged AppImage launch;
- fixture import/index/retrieval/chat/no-retrieval/cancel/restart/export-import proof;
- final changed-file list, command receipts, unresolved blockers, and rollback notes.
