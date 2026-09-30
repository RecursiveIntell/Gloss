# Gloss hostile repair pass — 2026-09-30

## Current verdict

Work in progress. Twelve separately scoped hostile audits examined clean baseline
`66399d330b8e28d8d8ec3c6abdd48d96b122a8a7` and found 21 bounded remediation roots.
The first repair batch below is implemented and has focused source-level proof.
Native desktop, real-model, extracted AppImage and independent final-review gates
must run on the final commit before candidate validation. No merge or release is
part of this pass. No real user notebook was mutated.

The acceptance path is existing notebook → import/embed → scoped question →
inspect evidence → save note → restart, including failure/retry/rebuild. This is
not a rewrite or a new retrieval architecture. Candidate proof and daily use on
an existing user notebook are separate gates.

## Audit inventory

| Audit | Boundary inspected | Principal evidence / outcome |
|---|---|---|
| F1 | Frontend types, state and event routing | `App.tsx`, `lib/events.ts`, `chatStore.ts`, notebook/source stores, backend emit/replay owners; duplicate live/replay and stream identity defects |
| F2 | Notebook/import/question/evidence/save workflows | SourcesPanel, sourceStore, ChatPanel, evidence/prompt/receipt panels, notes/source viewer; lost drafts and evidence misattribution |
| F3 | Accessibility, keyboard and modal behavior | Settings/source viewer, command palette/global shortcuts, inspector tabs, composer, widgets; missing modal contract and unnamed controls |
| F4 | Repeated, interrupted and stale UI operations | Overlapping replay, same-notebook/A→B→A loads, import completions, Studio output switch; stale ownership and widget crashes |
| M1 | Ingestion, embedding caches, scope and evidence integrity | embed/dense owners, semantic adapter/backend, source scope, strict-profile gates; cache mismatch and prospective TQ admission defect |
| M2 | Persistence, migrations, deletion and data loss | notebook DB/migrations, portable export/import, doctor, notebook commands, saved notes; WAL omission and multi-resource failure consistency |
| M3 | Concurrency, cancellation, restart and recovery | notebook pool, startup, queue policy/supervision, retry/reset/publication; interrupted pending imports stranded after restart |
| S1 | IPC, filesystem and network authority | Tauri contract, URL/YouTube DNS/redirects, portable archive validation; connection binding and incomplete payload inventory |
| S2 | Provider configuration, errors and timeout contracts | settings contract/UI/writers, model refresh, provider HTTP owners, terminal persistence; timeout edge and masked model-list errors |
| S3 | Large-input/resource bounds and backpressure | context sizing/disclosures, provider streams/model lists, replay buffer; false trimming disclosure and missing aggregate bounds |
| B1 | Build, dependencies and test truth | manifests/locks, vendored closure, compiler, source identity, canonical verify/CI and owner harness; no additional build defect in executed baseline gates |
| B2 | Packaging/install and evidence integrity | locked AppImage build, extracted AppRun, source/artifact hashes, nested receipt checks, CI; no additional integrity defect in inspected current gate, local execution capability blocked |

## Root dispositions

“Implemented” below is not a substitute for final native or independent validation.

| Root | Confirmed baseline failure | Current disposition / regression owner |
|---|---|---|
| FE-01 | Live tokens plus replay duplicate output; overlap regresses cursor | Implemented: sequenced notifications, coalesced serial replay, monotonic applied cursor, explicit eviction gap; `hostileFrontend.test.ts`, native replay test |
| FE-02 | New Chat bypasses stream guard; notebook B displays A's stream | Implemented: store guard, scoped projection and owner-correct Stop; frontend rendered/store probes |
| FE-03 | Failed paste/URL imports resolve successfully and erase drafts | Implemented coordinated rejecting mutation contract and catching callers; `hostileInteractions.test.ts` |
| FE-04 | Retry All leaks rejected event promise | Implemented handler catch; real callback regression |
| FE-05 | Evidence inspector silently substitutes older response | Implemented latest-assistant ownership and previous-response streaming label; rendered regression |
| FE-06 | Visual modals lack keyboard/focus/accessible contract | Implemented shared modal focus/inert/Escape/return owner and names; markup gates pass, native keyboard gate passed at first head; final-head rerun required |
| FE-07 | Studio stale load/export response replaces newer state | Implemented epochs and success/error ownership; cancellation acknowledgement kept separate from terminal completion; store regressions |
| FE-08 | Switching progressed quiz/cards to shorter output crashes | Implemented keyed session and guarded identity reset; callback regressions |
| FE-09 | Slow successful import clears newer draft edits | Implemented synchronous in-flight owner and draft versions; callback regression |
| FE-10 | Unlayered global reset overrides compiled Tailwind utility padding/margins | Found while inspecting first-head native screenshots. Reset moved to base layer; native computed padding and 24px close-hit-target gates added for Settings and Source viewer; final-head execution pending |
| FE-11 | Focus/rail action before first ResizeObserver uses non-compact cached width and leaves a restored drawer covering chat | Native hit-testing and screenshot confirmed occlusion. Three delayed-observer callback probes fail before repair and pass after live host-width decisions; wide-layout preservation control passes |
| FE-12 | Empty virtualizer mounts before asynchronous history and opens old rows at the top; changed conversation identity temporarily retains previous rows | Native restart screenshot showed history present but list-end unacknowledged. Deferred nonempty list mount uses LAST/end; identity changes clear rows until canonical hydration, same identity preserves optimistic content. Render/store regressions and existing navigation cancellation gates pass |
| M1-01 | Recognized legacy-only/sharded cache cannot load | Implemented one read-only resolver for supported paths; HF/legacy, refs, ambiguity, partial/shard/direct-layout tests |
| M1-02 | Strict TQ admission requires TQ already active | Implemented prospective proof readiness; exact-rerank/generation/digest gates retained |
| M2-01 | Export copies main SQLite file and loses committed WAL rows | Implemented and isolated native regressions passed; integrated full gate pending |
| M2-02 | Create/import/delete failure loses registry/filesystem consistency | Implemented staging and recoverable quarantine; isolated failure-injection tests passed, integrated full gate pending |
| M2-03 | Doctor(false) runs migrations/FTS writes | Implemented read-only diagnostics; isolated byte/schema/data_version tests passed, integrated full gate pending |
| M3-01 | Restart leaves pending imports permanently “already running” | Implemented startup recovery before worker admission, durable queue exclusions, panic terminalization; actual process-exit DB fixtures at registration/extraction/chunk stages |
| S1-F1 | DNS check is not bound to connection; mapped IPv6 bypass | Implemented pinned checked DNS answers per redirect with no proxies; actual transport fixture passes |
| S1-F2 | Portable manifest ignores missing/unmanifested payloads | Implemented exhaustive bounded inventory and rehashed consumption; isolated rejection regressions passed |
| S2-F1 | 296–300s embedding timeout loses promised grace; scalar bypass | Implemented 305000 ms ceiling, atomic mutation ownership and runtime cross-field validation; owner/UI tests pass |
| S2-F2 | llama.cpp model refresh masks connection/HTTP failures | Implemented explicit llama.cpp and Anthropic refresh failures; actual HTTP fixtures pass |
| S3-F1 | Context disclosure claims trimming that never happened | Implemented truthful untrimmed over-limit disclosure and receipt flag; request text unchanged |
| S3-F2 | Total stream/model-list/replay bytes are unbounded | Implemented 8 MiB response/model-list bounds and 16 MiB/4096-event replay bound; native boundary tests pass |

## Evidence so far

Baseline: 159 frontend tests, 122 script tests, 49 validation tests, 178 native
owner unit tests and one native recovery integration passed. The real Ollama
canary was explicitly ignored locally. Frontend build, Cargo format, vendor
closure, Tauri contract and repair static gates passed; production npm audit
reported zero vulnerabilities. Those green checks did not detect the 21 roots.

Hostile probes: 16 frontend acceptance assertions failed with one no-retrieval
positive control passing; six security acceptance assertions failed plus three
counterexample witnesses; an actual NotebookDb WAL fixture held one committed
saved note while the raw copied database held zero. These were disposable
fixtures, not production data or performance benchmarks.

First batch: 179 frontend tests pass. New native cache/restart owner tests pass;
process-exit integration preserves registered/extracted/chunked source state,
marks abandoned work retryable and leaves queue-owned work alone. Script suite
122 and validation suite 51 pass. Full aggregate results will be updated after
integration rather than summing overlapping repeated runs.

## Limits and required remaining gates

- Local cloud machine lacks GTK/WebKit desktop dependencies, WebDriver and a
  display. A supported cloud-browser attempt at the local frontend was blocked
  by `ERR_BLOCKED_BY_CLIENT`; it was not bypassed or counted as UI proof
- Existing CI includes actual native WebDriver interaction, isolated real
  Ollama, and extracted AppRun replay. Those jobs must pass for the final head;
  an old master success or a compile alone is insufficient
- Browser markup/callback tests do not prove screen-reader, geometry or native
  focus behavior. A settings Tab/Shift+Tab/Escape/focus-return gate now runs in
  the existing native desktop workflow
- Full source/embedding publication races, real Nomic inference, large real
  corpora and the user's existing notebook are not certified by tiny fixtures
- No performance, security, daily-use or release-readiness blanket claim is made

## Rollback and hostile-auditor handoff

Review the exact diff against the baseline, then rerun the canonical commands in
AGENTS.md and `npm run verify`. Check the new failure tests against both baseline
and repaired owners. Falsify live/replay overlap, lost-prefix handling, terminal
cleanup after notebook switches, WAL snapshot roundtrip, package inventory,
create/import/delete injected failure, read-only diagnostics, restart/retry and
network connection binding. Retain failed receipts. Inspect native screenshots,
DOM traces and artifact/source hashes rather than accepting job labels alone.

Rollback is a focused revert of the task commits in a separate clean checkout.
Keep original notebooks and existing exports untouched; tests use disposable
profiles. Do not purge recovered/quarantined canonical data to make a gate pass.

Portability integration review additionally fences native inference/active attempts during deletion, restores scheduling after a failed deletion, and changes confirmation text to disclose recoverable local retention. Three identical portability regressions fail on baseline and pass on the isolated repair; final integrated native evidence remains pending. TurboQuant registry harness passed 6 owner tests plus 7 transport tests locally before final integration.

## Integrated local checkpoint

After forced owner-package clean and explicit compilation from this checkout, the integrated native harness passes 227 unit tests and five integration tests. The spawned crash-fixture helper and real Ollama canary are intentionally not standalone local gates. Python suites pass 122 script + 51 validation tests. Frontend build and repair static gates pass. All 21 roots now have implemented repairs; exact-head native/Tauri CI and independent post-audit are still mandatory.


## First-head CI and bounded follow-up

At `475ae8c527fca6d01cb2ccee27845e4cbc773af7`, frontend, native C++
ASan/LSan, real Ollama owner canary and integrated native desktop jobs passed.
The twelve native desktop cases include scoped question/evidence, failed
embedding recovery, no-retrieval chat, saved notes, cancellation/retry and
restart. The native settings keyboard assertions executed successfully.
Artifact source tree and content digest matched that head. These are hosted
fresh-profile fixture observations, not validation of an existing user notebook.

Canonical verify failed because the dependent TurboQuant harness lock was not
updated after native harness integration. The follow-up synchronizes the lock
with the root workspace owner, adds the direct serde derive dependency consumed
by the shared proof type, and passes the unchanged locked suite: 8 owner tests
and 7 transport tests. The original failed CI receipt is retained.

Native screenshot inspection also found FE-10, bringing the remediation map to
22 roots across the same twelve audit scopes. The global reset was unlayered,
so it beat Tailwind 4's layered spacing utilities. Moving only that reset into
the base layer restores the intended utility precedence. The follow-up requires
computed header/body padding and minimum close-control geometry in the actual
native WebView, in addition to the existing keyboard gate. Driver policy tests
reject observations with zero padding or collapsed 16px controls; they do not
substitute for the required final-head native run.


At follow-up head `e0b93e0`, native Settings and Source viewer both measured
16px horizontal/body padding, 12px header vertical padding and 24px close
controls. The native run then failed restart conversation selection: the
Sources overlay covered a focused, enabled, in-viewport selector. This failed
receipt is retained. FE-11 reproduces the delayed-measurement decision race
with three failing callback probes and a passing wide-layout control; the
repair uses current host geometry for focus and both drawer-open actions.
The package run on that same head passed, showing why repeated fresh runs
matter. There are now 23 remediation roots across the original twelve scopes.

The final package job schedules three independent runner/profile repetitions
with fail-fast disabled and distinct repetition/attempt artifact names. These
are planned acceptance repetitions, not automatic retries until green.


The same follow-up's canonical verification passed all three Tauri compile
configurations, then reached 376 passing full Tauri tests and one failing
legacy fixture. That fixture called `turbo_quant_sidecar` proven even though
the shared owner now requires the canonical backend identity. Its positive
case now uses `TURBO_QUANT_BACKEND`; an explicit negative case retains rejection
of the old spelling. The product predicate is unchanged. The failed full-suite
receipt remains authoritative until the next exact-head full Tauri run passes.


At `3633f85`, all three planned package repetitions passed while the native
debug workflow exposed FE-12 after a Notes restart. Its screenshot showed
saved history at the top and a visible Jump to latest, not data loss. The
empty list had mounted before asynchronous history arrived. The follow-up
mounts the virtualizer when history exists and starts at LAST/end, while
retaining user-controlled navigation thereafter. Changing conversation clears
old projected rows immediately; selecting the same identity preserves optimistic
rows. Three new probes fail on the prior owner, a same-owner control passes,
and all 189 frontend tests pass after repair. The full native workflow remains
the decisive regression gate. The map now contains 24 roots.


That head's full Tauri tests then passed. Canonical verification stopped at one
new Clippy lint in portable schema range validation. The equivalent inclusive
range expression passes the twelve portability owner tests; no lint is disabled
in the application gate. A populated-history background-stream isolation probe
also passes. The final frontend rerun has 190 passing tests plus contracts and
production build; exact-head native/verification/package repetitions must rerun.
