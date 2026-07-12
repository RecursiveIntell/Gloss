# Gloss Full Wiring Audit — 2026-07-11

## Verdict

Gloss is not currently shippable. The frontend compiles and its narrow tests pass, but the Rust backend does not parse. The existing static “repair gates” all report PASS despite that hard failure. The repository also has contradictory backend ownership, non-portable path dependencies, an incomplete IPC/event contract, stale public documentation, and a dirty 57-path working tree containing 5,432 insertions and 1,665 deletions that has not passed a backend compile gate.

This is primarily a control-plane and integration failure, not a lack-of-features problem. Stop adding Studio outputs until the build, truth ownership, and end-to-end contract are repaired.

## Scope and evidence basis

Inspected current working tree at:

- Repository: `/home/sikmindz/Coding/Gloss`
- Branch: `perf-slowdown-fix-20260610`
- HEAD: `93864835b21a52d4e504cb97c650d90abfc2f082`
- Audit timestamp: `2026-07-11T19:00:35-05:00`
- Dirty paths: 57
- Tracked files: 880
- Tracked size: approximately 294.7 MiB
- Application source: 12,286 TypeScript/TSX LOC and 37,868 Rust LOC

Current files and fresh command output were treated as authoritative. Prior audit prose was used only as a lead and was rechecked where practical.

## Fresh gate results

| Gate | Result | Evidence |
|---|---|---|
| `npm run build` | PASS with warning | 2,003 modules; 654.39 kB main JS chunk; Vite warns chunk exceeds 500 kB |
| `npm test` | PASS, but not unit tests | Runs `scripts/run_frontend_contract_tests.mjs`; 12 static string/contract checks |
| `npm run test:unit` | PASS | 3 files, 16 tests |
| `bash validation/run_all_gloss_repair_gates.sh .` | PASS | Five static Python gates pass |
| `cargo fmt --all -- --check` | FAIL | `src-tauri/src/commands/studio.rs:612` contains literal escaped `\n` source text |
| `cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant` | FAIL | 38 parser errors; backend cannot compile |
| `npm audit --json` | FAIL | 3 advisories: 1 high, 2 low; Vite fix available |
| `cargo audit` | NOT RUN | `cargo-audit` is not installed |

## Prioritized findings

### P0-1 — The backend does not compile

Evidence:

- `src-tauri/src/commands/studio.rs:612-614` contains literal text such as `}\n\n#[tauri::command]\npub async fn...` rather than Rust newlines.
- Fresh `cargo check` reports 38 parser errors.
- Fresh `cargo fmt --check` fails on the same file.

Impact:

- No Tauri application build.
- No Rust tests can run.
- No runtime or installer claim is valid for the current tree.

Repair:

- Preserve the current diff as a receipt.
- Replace the malformed escaped insertion with valid Rust source.
- Register `explain_quiz_question` only if the UI actually invokes it; otherwise remove the half-added feature from this repair pass.
- Run format, check, targeted Studio tests, full Rust tests, and a packaged smoke test before any further feature work.

### P0-2 — The canonical validation path produces false green results

Evidence:

- `validation/run_all_gloss_repair_gates.sh` passed while the Rust parser failed.
- `package.json:10` maps `npm test` to a static contract script, not Vitest.
- Actual unit tests live behind the separate `npm run test:unit` command.
- `.github/workflows/ci.yml:55-57` runs `cargo test`, but local repair gates do not compile Rust first and do not run frontend unit tests.

Impact:

- “All gates pass” currently does not mean the app builds.
- Existing `AUDIT.md:3` and `FIX_PLAN.md:7,103-107` claim passing tests/gates that are false for the current tree.

Repair:

Create one canonical `verify` command that fails fast and runs, in order:

1. TypeScript typecheck/build.
2. Vitest unit tests.
3. Frontend contract tests.
4. Rust format.
5. Rust check with release features.
6. Targeted Rust lifecycle tests.
7. Full Rust test suite.
8. Static policy gates.
9. Tauri package build.
10. Desktop smoke test.

Static gates must never be presented as substitutes for compilation or runtime proof.

### P0-3 — CI is not reproducible from the Gloss repository

Evidence:

- `src-tauri/Cargo.toml:28-29,58` uses:
  - `../../Libraries/llm-pipeline`
  - `../../Libraries/tauri-queue`
  - `../../Libraries/semantic-memory`
- Those paths resolve only because `/home/sikmindz/Coding/Libraries` exists locally.
- GitHub Actions checks out Gloss alone and does not fetch the sibling Libraries workspace.
- The repo contains `src-tauri/vendor/*`, but the active manifest does not use those copies.

Impact:

- A clean clone cannot resolve active Rust dependencies.
- CI/release behavior depends on undeclared filesystem state.
- Vendored and sibling copies create two potential truths.

Repair options, ranked:

1. Best long-term: publish/version the canonical Libraries crates and depend on exact crate versions plus lockfile.
2. Acceptable monorepo option: make Gloss and Libraries members of one canonical workspace with one checkout.
3. Temporary: pin Git revisions in `Cargo.toml`.
4. Avoid: restoring hidden local path assumptions or maintaining active duplicate vendored code.

Delete/archive inactive vendor snapshots after the canonical dependency route is proven.

### P0-4 — The dirty integration batch is too large to reason about safely

Evidence:

- 57 dirty paths.
- `git diff --stat`: 46 tracked files changed, 5,432 insertions, 1,665 deletions, plus untracked files.
- Changes simultaneously touch chat lifecycle, provider cancellation, Studio, semantic memory, migrations, settings, retrieval, jobs, state, and frontend stores.
- The malformed Studio insertion is inside this uncommitted batch.

Impact:

- Root-cause isolation is poor.
- Partial rollback is dangerous.
- Claims from earlier passes no longer describe the present tree.

Repair:

- Freeze feature work.
- Capture full status/diff and checksums.
- Split the batch into independently buildable lanes: build baseline, provider execution context, chat lifecycle, Studio lifecycle, semantic-memory ownership, DB migrations, frontend contracts, docs/release.
- Require green canonical verification after each lane.

### P1-1 — Semantic-memory ownership and feature semantics contradict themselves

Evidence:

- `src-tauri/Cargo.toml:14-20` says semantic-memory is always compiled and the only backend.
- `src-tauri/src/state.rs:365-369` initializes `memory_backend = "gloss-local"` with fallback enabled.
- `src-tauri/src/features.rs:363-368` makes preview availability and activity unconditional (`Ok(())`, `Ok(true)`).
- The same file still defines preview and TurboQuant feature flags and can reset the selected backend to `gloss-local`.
- `src-tauri/src/lib.rs:173-180` exposes both backend status and comparison commands.
- Frontend status code synthesizes fallback backend objects on errors (`src/components/layout/StatusBar.tsx:96-103`).

Impact:

- Build feature, user setting, active runtime backend, and reported backend can disagree.
- “Preview,” “always-on,” and “fallback” coexist without a single authoritative state machine.
- UI can display manufactured fallback state rather than a durable backend decision receipt.

Repair:

Choose one model:

- Recommended: semantic-memory is the sole durable retrieval/index backend; BM25/dense/source-order are explicit strategies inside it. Remove the backend profile switch and “preview” terminology.
- Alternative: keep two real backends, but make availability, activation, fallback, and migration explicit state-machine transitions with one backend-decision receipt.

Do not preserve the current hybrid of unconditional functions, legacy settings, and UI-synthesized state.

### P1-2 — IPC commands align, but event wiring is incomplete and unproven

Fresh static extraction found:

- 76 frontend invoke names.
- 77 registered Tauri commands.
- No frontend invoke missing from registration.
- `compare_memory_backends` is registered but not invoked.
- `explain_quiz_question` is defined in malformed source but not registered or invoked.
- Rust emits `chat:stream_event` and `sources:folder_scan` with no direct frontend listener found.
- Frontend listens for `queue:job_completed` with no direct Rust emission found in application source.

Caveat: `tauri-queue` may emit events from a dependency, so the queue event mismatch needs runtime verification rather than immediate deletion.

Repair:

- Define commands and events once in a typed contract module/schema.
- Generate or validate both Rust registrations and TypeScript wrappers from that contract.
- Add a runtime IPC smoke test that invokes every supported command with fixture state and observes every terminal event family.
- Remove dead registrations and listeners only after runtime tracing confirms they are dead.

### P1-3 — Chat event replay uses one global sequence cursor

Evidence:

- `src/stores/chatStore.ts:35,75` stores a single `lastChatEventSeq`.
- `replayChatEvents` accepts notebook and conversation IDs but queries with that global cursor at line 147.
- The store handles notebook/conversation switching and rehydration in the same singleton.

Impact:

- If event sequences are scoped per notebook/conversation, a high cursor from one conversation can suppress replay in another.
- If they are globally scoped, the ownership is undocumented and still vulnerable to reset/switch races.

Repair:

- Make the cursor key explicit: `Record<notebookId:conversationId, seq>` unless the DB proves a globally monotonic stream.
- Persist cursor and terminal attempt state together.
- Add switch-away/switch-back, crash/restart, duplicate-event, and out-of-order replay tests.

### P1-4 — Source scope silently degrades to no retrieval

Evidence:

- `src/stores/sourceStore.ts:28-31` says explicit retrieval should not silently downgrade.
- `buildSourceScope` returns `{ kind: 'none' }` for idle state, empty source arrays, and filtered-empty explicit selections at lines 32-46.

Impact:

- The implementation contradicts its own comment.
- A user asking for retrieval can receive free-form chat without an explicit refusal/degradation decision.

Repair:

- Represent `unavailable`, `partial`, `explicit-empty`, and `none-by-user` separately.
- Send the user’s requested scope plus source-list health to the backend.
- Let the backend return a typed scope decision: proceed, degrade with consent, or fail loud.
- Surface that decision in chat evidence and UI.

### P1-5 — Studio’s concurrency guard is process-global on the frontend

Evidence:

- `src/stores/studioStore.ts:44` uses module-global `activeGenerationPromise`.
- Any generation request reuses that promise regardless of notebook/output identity at lines 82-85.
- Backend intent is per-notebook attempt tracking (`active_studio_attempts`).

Impact:

- A request in notebook B can receive notebook A’s promise/result.
- Frontend and backend concurrency scopes differ.

Repair:

- Key active attempts by notebook ID and attempt ID.
- Reject or explicitly join only the same attempt.
- Add cross-notebook generation and cancellation tests.

### P1-6 — Provider health polling is duplicated and can create load/races

Evidence:

- `StatusBar.tsx:54-77` polls chat and background providers every 30 seconds.
- `StatusBar.tsx:93-138` separately polls queue, memory, semantic profile, and source stats every 5 seconds.
- `DiagnosticsPanel.tsx:41-49` repeats provider, memory, profile, queue, and source-stat requests.

Impact:

- Opening diagnostics duplicates health traffic.
- Provider “tests” may be nontrivial model/network operations rather than cheap health probes.
- Multiple components own overlapping runtime truth.

Repair:

- Introduce one health/status store with deduplicated queries, cancellation, visibility awareness, timestamps, and stale/error states.
- Prefer backend-pushed state changes plus slow fallback polling.
- Separate cheap endpoint reachability from actual model smoke tests.

### P1-7 — Frontend test coverage is too thin for the lifecycle complexity

Evidence:

- 12,286 frontend LOC.
- 3 Vitest files and 16 tests.
- `npm test` does not run those tests.
- Chat alone contains replay, optimistic writes, dual message IDs, switching, cancellation, evidence attachment, and terminal-state logic.

Impact:

- Core race behavior is mostly protected by comments and static checks.
- The most complex wiring is not exercised through rendered components or Tauri mocks.

Repair:

Add contract and integration tests for:

- notebook switch during chat/send/replay;
- conversation switch during streaming;
- global/per-conversation event sequence behavior;
- source load partial/error versus requested scope;
- provider timeout/cancel/error terminal events;
- Studio cross-notebook concurrency;
- queue completion refresh;
- settings/provider/model authority;
- import/index/retrieval/chat end-to-end fixture.

### P1-8 — Security dependency baseline is red

Evidence:

- `npm audit` reports Vite 7.3.3 affected by a high-severity Windows path-deny bypass and a moderate UNC/NTLM issue; fixed versions are available.
- Two low-severity transitive advisories also have fixes.
- Rust advisory status was not checked because `cargo-audit` is absent.

Repair:

- Upgrade Vite within the current major first (`7.3.6` was reported as wanted), rerun build/tests/audit.
- Then evaluate Vite 8 as a separate migration, not inside baseline repair.
- Install and pin `cargo-deny` or `cargo-audit` in CI.
- Add license/source/duplicate checks for Rust dependencies.

### P1-9 — README and active audit documents materially overclaim the current implementation

Evidence:

- `README.md:54-60` names ingestion crates/features not present in the active manifest.
- `README.md:145` claims Tauri SecretStore, while active code uses a custom provider config/secret store path.
- `README.md:238-248` lists fastembed 4, lopdf, calamine, whisper-rs, piper-rs, and tauri-plugin-store; active `Cargo.toml` has fastembed 5 and does not list several of those crates.
- `README.md:130` calls semantic-memory a preview profile while the manifest says it is always compiled and the only backend.
- `AUDIT.md` and `FIX_PLAN.md` report green Rust tests/gates despite the current parser failure.

Impact:

- Operators cannot tell what ships.
- Prior prose is functioning as shadow truth.

Repair:

- Generate feature/support tables from manifests plus runtime capability probes.
- Mark planned, compiled, configured, runtime-verified, and packaged capabilities separately.
- Archive old active-looking audits; keep one current status document generated from receipts.

### P2-1 — The frontend bundle needs intentional splitting

Evidence:

- Production build emits one 654.39 kB JS chunk and a Vite >500 kB warning.

Repair:

- Lazy-load Studio renderers, diagnostics/receipt inspectors, settings, D3 mind map, and markdown-heavy views.
- Measure cold start and interaction latency before/after; do not merely silence the warning.

### P2-2 — Repository evidence/archive volume obscures active source

Evidence:

- Approximately 294.7 MiB tracked.
- 137 tracked files under `docs/`, 124 paths matching archive/codex-run/log/evidence patterns, and multiple generations of audit artifacts.
- Top-level contains overlapping `EVIDENCE`, `evidence`, `VALIDATION`, `validation`, `PHASES`, run logs, receipts, and reports.

Repair:

- Keep current source, executable gates, schemas, and a small current audit index in Git.
- Move bulky immutable run artifacts to release assets/object storage or a dedicated evidence branch/repository, indexed by digest.
- Do not delete evidence until hashes and references are preserved.

## Additional verified wiring failures

### P0-5 — Rust dependency graph has actionable advisories

Fresh `cargo deny check advisories` failed. Reported runtime-relevant versions include:

- `anyhow 1.0.101` — upgrade to at least 1.0.103.
- `crossbeam-epoch 0.9.18` — upgrade to at least 0.9.20.
- direct `quick-xml 0.38.4` (`src-tauri/Cargo.toml:65`) — namespace-declaration memory-exhaustion advisory; upgrade to at least 0.41.0.
- direct `tar 0.4.44` (`src-tauri/Cargo.toml:74`) — two extraction advisories including symlink-following chmod; upgrade to at least 0.4.45.
- `rustls-webpki 0.103.9` — multiple advisories; move to at least 0.103.13 through owning TLS dependencies.

There is no `deny.toml`, and CI runs no Rust advisory gate. These need patching before processing untrusted notebook archives or network content.

Clarification: full `npm audit` reports one high and two low development-chain advisories, while `npm audit --omit=dev` reports zero production advisories. The Vite development server issues still matter for developer machines, particularly Windows.

### P1-10 — Background indexing writes a noncanonical shadow index and is not enqueued

Evidence:

- Canonical runtime index loads/saves `embeddings/chunks.usearch`: `src-tauri/src/state.rs:731,840-841`.
- `IndexChunks` writes `embeddings/hnsw.index`: `src-tauri/src/jobs/mod.rs:289`.
- It then marks canonical `NATIVE_HNSW_INDEX_ID` metadata ready: `src-tauri/src/jobs/mod.rs:334-343`.
- Portable notebook handling recognizes `chunks.usearch`, not `hnsw.index`: `src-tauri/src/db/portable.rs:558-560,638-640`.
- The job has an enum/executor but no non-test constructor/enqueue call; normal ingestion embeds synchronously in `src-tauri/src/commands/sources/mod.rs:545-592`.

Impact:

- If called, the job can mark a different artifact ready than runtime retrieval loads.
- In the current application it appears declaration-only, so its purported cancellation/persistence behavior is not exercised.

Repair:

- Define one shared canonical index path and commit protocol.
- Either wire `IndexChunks` after chunk persistence with an integration test, or delete it and keep synchronous indexing. Do not retain two apparent indexing lifecycles.

### P1-11 — Chat attempt ownership starts after persistence and expensive preprocessing

Evidence:

- User message persistence occurs at `src-tauri/src/commands/chat/mod.rs:680`.
- Active-attempt registration occurs much later at lines 2203-2208, after provider resolution, retrieval, and context construction.
- `stop_chat` can cancel only registered attempts: lines 449-462.
- Collision rejection occurs after the losing request has already persisted its user message: lines 2210-2264.

Impact:

- Concurrent sends can duplicate expensive work and leave an orphaned visible user turn.
- Cancellation cannot stop early retrieval/provider setup.

Repair:

- Register the attempt immediately after notebook/conversation validation and before persistence.
- Carry cancellation through all blocking/async phases.
- Reject before persistence or atomically persist an explicit rejected/cancelled turn.

### P1-12 — Embedding settings do not govern native ingestion consistently

Evidence:

- DB defaults still select Ollama: `src-tauri/src/db/migrations.rs:97-100,135-166` and `state.rs:395-420`.
- `AppState::ensure_embedder` claims to read provider settings but always constructs FastEmbed: `state.rs:517-561`.
- Semantic-memory projection separately honors provider settings: `commands/sources/mod.rs:693-729` and `memory/semantic_memory_adapter.rs:209-241`.
- `IndexChunks` hard-codes model downloads allowed at `jobs/mod.rs:280-286`, bypassing explicit consent.

Impact:

- Native indexing and semantic-memory projection can use different providers/models under one settings surface.
- Metadata and diagnostics may describe configuration that native ingestion ignored.
- A future wired background job could trigger network/model downloads despite refusal.

Repair:

- Create one typed embedding runtime configuration used by ingestion, jobs, semantic-memory, diagnostics, and reindexing.
- Invalidate cached embedders and dependent indices when configuration changes.
- Carry a consent/policy snapshot into jobs; never hard-code download permission.

### P1-13 — Timeline and comparison renderers parse the wrong payload level

Evidence:

- Backend stores a complete `StudioArtifact`; timeline/table fields are nested under `artifact.content`: `src-tauri/src/studio/mod.rs:438-458`.
- `TimelineView.tsx:16-23,56-60` and `DataTableView.tsx:15-23,66-70` expect those fields at the root.
- `StudioPanel.tsx:330-338` always routes matching outputs to these renderers.

Impact:

- Valid outputs display “No timeline entries available” or “No table data available.”

Repair:

- Parse the actual serialized artifact shape and test with Rust-generated fixtures. Support a legacy root shape only if real persisted data requires it.

### P1-14 — Source selection and model selection have additional shadow state

Evidence:

- Source selection persistence reads notebook ownership later from `localStorage`: `src/stores/sourceStore.ts:53-62,426-473`.
- Notebook activation updates backend state before store reset/localStorage update: `notebookStore.ts:79-88`.
- Model selection is duplicated between `settings["default_model"]` and mutable `activeModel`: `settingsStore.ts:9,11,32`.
- `ChatPanel.tsx:258-268` starts two unawaited provider/model writes and separately changes `activeModel`.

Impact:

- Debounced source selection can target the wrong notebook during activation.
- Provider/model can become a mismatched pair after partial failure.

Repair:

- Scope source debounce/in-flight state by explicit notebook ID.
- Replace split model writes with one transactional `selectProviderModel` action and derive displayed selection from canonical settings.

### P2-3 — Generic queue worker applies summary policy and both inference gates to every job

Evidence:

- `summary_job_loop` blocks all queue processing for no notebook, summary pause, and chat grace: `src-tauri/src/lib.rs:219-259`.
- It acquires GPU and LLM gates before generic `process_one`: lines 313-401.
- The queue also contains indexing, audio metadata, and media jobs: `src-tauri/src/jobs/mod.rs:19-80`.

Impact:

- Pausing summaries pauses unrelated work.
- Non-LLM jobs can unnecessarily hold inference gates.

Repair:

- Classify jobs by required resources and policy. Use separate workers/queues or inspect the next job before acquiring only the necessary gates.

### P2-4 — Egress policy validates initial URLs but not redirects/resolved destinations

Evidence:

- URL policy validates the initial textual host: `src-tauri/src/providers/mod.rs:304-395`.
- Shared reqwest client retains default redirect behavior: lines 289-301.

Impact:

- An allowed endpoint can redirect to an unapproved destination.
- Textual localhost does not prove the resolved address remains loopback.

Repair:

- Disable redirects or revalidate every redirect target.
- Resolve local-provider hosts and enforce loopback/consented address classes with DNS-rebinding-aware tests.

### P2-5 — Test discovery and lockfile truth are inconsistent

Evidence:

- `vitest.config.ts:3-6` only includes tests under `src/**/__tests__/`; `src/lib/evidenceContract.test.ts` is therefore excluded.
- Root `Cargo.lock` represents the active workspace and fastembed 5.17.2.
- `src-tauri/Cargo.lock` is stale and still describes fastembed 4.9.1.

Repair:

- Discover `src/**/*.test.{ts,tsx}` or move the excluded test; assert expected test-file count in CI.
- Remove the nested lockfile or explicitly regenerate and document why two graphs are needed.

### P2-6 — Candle migration still retains ORT

Evidence:

- `fastembed` enables both `nomic-v2-moe` and `ort-download-binaries-native-tls`: `src-tauri/Cargo.toml:53`.
- `cargo tree -i ort` reports `fastembed 5.17.2 -> ort 2.0.0-rc.12`.

Impact:

- The claimed ORT avoidance is incomplete, and packaging still carries the native runtime.

Repair:

- Determine which embedding/reranking models are actually used. Remove the ORT feature if Nomic v2 MoE alone is sufficient; otherwise document ORT as intentional and test packaging on every supported platform.

## Architecture recommendation

Keep Tauri + React + Zustand. The framework choice is not the root problem and replacing it would create more wiring risk.

Recommended target architecture:

1. One application contract package/schema for Tauri commands, payloads, and events.
2. One backend execution context for cancellation, deadlines, attempt IDs, provider/model selection, and receipts.
3. One authoritative notebook epoch/operation registry covering chat, ingestion, Studio, indexing, and jobs.
4. One retrieval service with explicit strategies and degradation decisions; no competing “backend profile” shadow state.
5. One frontend runtime-status store; components render it but do not independently probe and synthesize truth.
6. One canonical verification entrypoint whose green result means build + tests + policy + package smoke all passed.
7. Reproducible dependency resolution from a clean clone.

## Dependency recommendations

- Keep React 19, Zustand 5, Tauri 2, Rust/Tokio, SQLite/FTS5, and usearch unless benchmarks show a concrete reason to replace them.
- Keep fastembed 5/Nomic v2 MoE for now; the old fastembed 4 versus ORT blocker in `FIX_PLAN.md` is stale because the active manifest already moved to fastembed 5 with Candle-related features.
- Do not add TTS during stabilization. When revisited, prefer a sidecar/CLI process with an explicit artifact contract over coupling another inference runtime into the main process. This isolates model downloads, crashes, and native-library conflicts.
- Upgrade Vite 7 to its patched line immediately; evaluate Vite 8, react-markdown 10, and plugin-react 6 only after baseline verification is green.
- Do not upgrade TypeScript to the reported 7.x line during repair; treat it as a separate compatibility project.

Crates.io freshness could not be independently queried in this run because those HTTP requests failed, so no Rust “latest version” claims are made here.

## Release stop conditions

Do not call Gloss complete or release-ready until all are true:

- Clean-clone dependency resolution works.
- Canonical verify command passes.
- Tauri package builds.
- Desktop smoke imports a fixture, indexes it, retrieves it, chats with and without retrieval, cancels a generation, restarts, and replays terminal state correctly.
- Backend identity and degradation are displayed from backend receipts, not frontend defaults.
- Current README capability table is generated from or cross-checked against runtime proof.
- Security audits are green or have explicit dated exceptions.

## What was not verified

- No live desktop GUI smoke was possible because the backend does not compile.
- No Rust tests ran past parsing.
- No installer/package was built.
- No cloud-provider API call was made.
- No Ollama model inference was run.
- No clean VM/container clone was created.
- No Rust advisory scan ran.
- No performance benchmark beyond bundle output was run.
