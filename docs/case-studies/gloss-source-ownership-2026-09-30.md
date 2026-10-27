# Preserving source ownership in Gloss

**Review draft. Evidence checked October 8, 2026.**  This case study examines [Gloss PR #10][pr], merged September 30, 2026.  It demonstrates a bounded repair and qualification process for a local-first Rust/Tauri research notebook.  It is not a production-readiness or release certificate.

The central problem was that several derived representations could lose their connection to their owner: an asynchronous list could overwrite newer state, a selected-source set could become an unrestricted search, an export could include an unregistered file, and extracted text could change document meaning.  The repair keeps authority in the existing stores, SQLite snapshot, document manifests, and per-job resource owners.  No new persistence layer or notebook schema migration was introduced.

**Engineering attribution:** the PR account is RecursiveIntell, and both repair commits record Josh Stevenson as author.  This case study and its additional regression review were prepared with Codex assistance.  Commit attribution does not establish sole manual implementation.  The historical audit describes twelve review scopes and a separate reviewer; those are not twelve independent auditors or evidence of independent human certification.  Gloss also incorporates upstream libraries, including its vendored memory and retrieval dependencies; this case study concerns the Gloss integration and repair delta.

## Revision and evidence boundary

| Identity | Exact value |
| --- | --- |
| Before repair, merged PR #9 | `95d99220d107635a4a313c575ffd5306c0ffbee8` |
| Main repair commit | [`67e5a3bb2fe28c27f4e4ce40f14eac7469fa9653`][repair] |
| Final PR #10 head, including dependent lockfile repair | [`9889437763f2b8e3a846dad27c8d6ed2ce50d3ee`][head] |
| Merge and inspected `master` on October 8 | `5635f809487a1e0ac0afd5919ab93caac448c5b0` |
| Final head, qualification checkout, and merge tree | `52c87e50eb19ef8e6c41fce3e03748bf9bde872b` |
| Source snapshot SHA-256 | `05f5bc242b8700c68067684f59678698867a73aa9ad5452dab7872d387c49719` |
| Qualification | [Actions run 36736177399][run], attempt 1, September 30 |
| Synthetic PR merge checkout recorded in CI | `1da3f3975a3091985c54a3ddcc6839ba43aa1c44` |

The run metadata names the final PR head; the logged build receipt names the synthetic merge checkout.  The receipt's tree and source digest match the inspected merge checkout.  The digest was independently recomputed before adding this documentation, using the repository's [source snapshot owner][snapshot].  Equality binds these results to the same source contents; it does not make every different revision string interchangeable or prove reproducible binary builds.

Evidence labels used here:

- **Reproduced:** executed during the October 8 review, with retained output.
- **Source/log verified:** code, commit diff, workflow metadata, or job output directly inspected.  This is independent inspection of first-party evidence, not an independent runtime certification.
- **PR-reported:** a result stated in PR #10 or its audit that this review could not directly inspect or rerun.

## Failure mechanisms and repair decisions

### 1. A completed request does not necessarily own the current view

Before repair, a slow notebook list read could finish after a deletion and newer refresh, then restore the deleted row in the frontend.  In the frontend counterexample, deletion was acknowledged; the failure was stale response publication, rather than evidence of a backend deletion failure.  Conversation reads checked the notebook ID, but that alone admitted an old response after an A → B → A activation cycle.  Obsolete failures could also emit error toasts or clear a newer request's loading state.

The existing [notebook store][notebook] now admits success and failure only for the latest list request.  The [conversation store][conversation] additionally binds the request epoch, active notebook, and activation request identity, and invalidates outstanding reads on reset.  An inactive background refresh returns before it can supersede the active read.  The design fences response publication rather than adding a duplicate database or relying on cancellation to complete perfectly.

The October 8 replay applied the repaired tests to unchanged baseline product code.  Six list-ownership tests failed on that baseline and passed on the repaired source.  Their assertions cover reordered responses, deleted-row resurrection, A → B → A, obsolete errors, and inactive refresh admission.

### 2. Equal set sizes cannot authorize unrestricted retrieval

Consider current sources `{A, C}` and an explicit selection `{gone, A}` retained across deletion/refresh.  Both sets have size two.  The old individual and group toggles used cardinality to infer `all`, allowing an unselected source `C` into retrieval.

The [source store][scope] now requires equal cardinality **and** membership of every current source before selecting `all`.  Stale requested IDs remain explicit for diagnostics.  The backend separately resolves explicit IDs to valid members; an all-invalid selection resolves to none, not all.  This preserves user intent at the frontend boundary without changing provider authority or backend fallback policy.

Both toggle regressions failed on baseline and passed after repair.  The expected scope is explicit `['gone', 'A']`, never unrestricted retrieval.  These are actual store tests with mocked IPC, not live model or GUI tests.

### 3. A valid manifest can still contain a file the notebook does not own

The former export copied the entire `sources/` directory after taking the SQLite snapshot.  Files left by a failed unlink or an incomplete import could enter the package, even when every manifest hash was internally consistent.  Hash integrity could not establish notebook ownership.

The [portable export owner][portable] now reads distinct source `file_path` values from the exported database snapshot and copies only those assets.  Package validation requires the manifested source-path set to equal the snapshot-owned set.  Existing path, size, hash, WAL, restore, and derived-index invalidation checks remain in place.  A stored source hash is checked when present; the implementation does not assume every row carries one.

One fixture exports a nested owned file while excluding an orphan secret and an unregistered in-flight file, verifies roundtrip, and confirms the original orphan still exists locally.  Another adds an unowned file and recomputes a self-consistent manifest; import must reject it without publication.  These tests passed in the historical native-owner job.  They were source/log verified here, not rerun locally because this review environment has no Cargo toolchain.  The repair excludes unowned export payloads; it does not purge local files.

### 4. Extraction must preserve document semantics before retrieval begins

The [extractor diff][repair] exposes three distinct failures: PPTX/XLSX parts were selected by archive filenames rather than manifest relationships; XLSX rich-text runs were counted as separate shared-string items; and XML event handling lost entities/CDATA or inserted spaces between formatting runs.

The repaired [extractor][extract] follows presentation/workbook relationships in owner-defined order, groups spreadsheet strings by `<si>` including empty entries, retains sheet/cell identity, and concatenates adjacent text events before block normalization.  Missing, unsafe, duplicate, external, or incorrectly typed referenced parts fail visibly.  The adversarial DOCX fixture expects `microscope & <secret>`; the presentation deliberately references slide 2 before slide 1; the spreadsheet references the second shared-string item after a multi-run first item.

A further fixture references a 500,000-byte shared string three times.  The worksheet owner now checks expanded output while accumulating cells, rather than only after allocating the complete result.  The relevant `String.len()` guards enforce a one-million-byte output budget, despite the constant's `CHARS` name.  This is not a general hard process-memory quota.  Historical CI logs record these tests passing.  Existing extracted text is not silently repaired; the audit identifies explicit retry/reimport as the recovery path.

### 5. Scratch identity and native input admission need their own owners

Video scratch previously joined an imported source ID into a directory path and later recursively cleaned that target.  Persisted IDs are opaque metadata and may be absolute or traversing strings.  Audio cleanup also depended on reaching specific success/error branches.  The shared [media workspace owner][workspace] creates distinct Unix mode-0700 `TempDir` workspaces with fixed purpose prefixes and RAII cleanup, independently of source IDs.  Legacy Office output captures use anonymous Unix mode-0600 handles, with child termination/reaping and observed-size/deadline checks.  Controlled fixtures verify these boundaries; they do not execute ffmpeg, Whisper, or the Tauri job runner.  Output polling can overshoot and is not an OS quota.

The native driver's Settings action also gained observable admission: source viewer absent, one stable target, enabled, non-inert, and hit-owned before a single native click.  The prior timeout supports a missing-admission hypothesis; the exact inert state at that historical click was not recorded.  The October 8 bounded JavaScript fixtures independently reproduce premature clicking on the old driver and verify no click for a persistently inert target after repair.  The original deadline and required native cases remain unchanged.

## Compact evidence map

All source links below pin the final PR head.  The historical [audit][audit] supplies the broader ten-root inventory, counterexamples, and recovery notes.

| Important claim | Owner / executable evidence | Result and verification class |
| --- | --- | --- |
| Superseded notebook and conversation reads cannot publish into newer state | [Notebook tests][notebook-tests]; [conversation freshness tests][conversation-tests] | Six new counterexamples fail on baseline and pass after repair: **reproduced** |
| Both source toggles preserve explicit membership | [Source-store tests][scope-tests], parameterized deletion/refresh fixture | Two new counterexamples fail on baseline and pass after repair: **reproduced** |
| Export excludes orphan files; valid hashes do not admit an unowned source | [Portable ownership regressions][portable-tests]; [verify job][verify-job] | Both named tests logged `ok`: **source/log verified**, not locally rerun |
| Office order, string indexing, XML content and expansion bounds are enforced | [Fresh document regressions][extract-tests] and expansion tests in the same owner; [verify job][verify-job] | Named owner tests logged `ok`: **source/log verified** |
| Imported identity cannot select media cleanup target; partial outputs have private lifetime | [Media identity integration fixture][media-test]; [workspace tests][workspace]; [verify job][verify-job] | Controlled owner fixtures logged `ok`: **source/log verified**; broad media execution unproven |
| Native driver waits for inert-state release and rejects persistent inert state | [Actual-JavaScript readiness tests][click-tests] | Same two tests fail on baseline and pass after repair: **reproduced**, bounded DOM fixture |
| Eighteen canonical gates passed on the final source | [`GlossBuildVerificationV2` in verify log][verify-job] | 18 `passed`, exit 0, no unrun build gates, source unchanged: **log verified**; scope is `build_and_contracts_only` |
| Live owners, integrated native desktop, and three planned package repetitions succeeded | [Live job][live-job], [desktop job][desktop-job], package jobs [1][package-1] / [2][package-2] / [3][package-3] | Job success and printed runtime/package `pass` statuses: **log verified**; no current runtime rerun |
| Integrated workflow and each package repetition passed 12 mandatory cases; referenced-file verification counts were 274 and 290/291/290 | [PR #10 final-candidate statement][pr] and run artifact collection | **PR-reported**. Child receipts, screenshots, and binary archive bytes unavailable to this review (download HTTP 403) |

### What the qualification actually covered

The eight successful jobs are six logical job families, with the Linux package family expanded into three planned fresh-runner/profile repetitions.  They are repeat executions of the same protocol, not three independent auditors, platforms, or test suites.

The canonical verifier's logged receipt covers formatting, contracts, script/validation suites, actual-source native and TurboQuant harnesses, three application feature checks, application tests, frontend tests/build, strict application Clippy, dependency policy/audit, and desktop compilation.  It explicitly leaves live desktop/provider, package replay, and native leak checks to separate evidence.  The application log reports 392 tests passed and three ignored; the frontend log reports 200 tests passed.  Harness and application counts overlap.  **Do not sum gates, suite counts, native cases, repetitions, or file-hash counts into a unique-test total.**

The desktop job's baseline-only step printed `baseline_status: pass` with overall/integrated status `blocked`, correctly disclosing cases not exercised by that step.  Its subsequent real-model integrated step printed `status: pass`.  A green job does not turn the earlier baseline-only receipt into complete runtime proof.

The workflow uses Ubuntu 24.04 for native desktop/package jobs, Node 22, Rust 1.98.1, `tauri-driver` 2.0.6 and `cargo-deny` 0.19.8.  The real-model canary source pins Ollama 0.11.10 by full archive SHA-256 and admits `all-minilm:22m` embeddings and `qwen3:0.6b` chat through reviewed digest-prefix and size checks, retaining full observed model digests in receipts.  These are not fully immutable model-tag pins.  Those are **source-verified protocol inputs**; the inaccessible child receipts prevented independent readback of each run's full downloaded model identities.  C++ address/leak checks succeeded in the sanitizer job; Rust was not sanitizer-instrumented.

The failed [post-merge baseline run 36724544998][failed-baseline] and [first follow-up run 36732643114][failed-followup] remain part of the engineering record.  The first timed out opening Settings; the second exposed dependent TurboQuant harness lockfile drift after extraction expanded the native harness dependency graph.  The final commit synchronizes that closure, retains `--locked`, and reruns the final-candidate workflow.  The audit also preserves five existing failures from an exploratory all-target native-harness Clippy command; it is distinct from passing canonical strict application Clippy.

## Reproduce the focused result

Use a disposable checkout.  The October 8 replay used Linux x86_64, Node 24.19.0, npm 11.9.0, Python 3.12.14 and the installed lockfile-resolved Vitest 4.1.11.  This differs from historical CI's Node 22.  Local dependency installation used `npm ci --ignore-scripts`; the commands below intentionally reproduce that lightweight owner-test setup, not a desktop installation.

```bash
git clone https://github.com/RecursiveIntell/Gloss.git gloss-case-study
cd gloss-case-study
git checkout --detach 9889437763f2b8e3a846dad27c8d6ed2ce50d3ee
npm ci --ignore-scripts
npm run test:unit -- src/stores/__tests__/notebookStore.test.ts \
  src/stores/__tests__/conversationFreshness.test.ts \
  src/stores/__tests__/sourceStore.test.ts
python3 -m unittest discover -s scripts/tests -p test_native_click_readiness.py -v
```

Expected and observed: the three frontend files pass all 22 tests; both readiness fixtures pass.  These 22 include existing tests, with eight repair-added counterexamples that fail on baseline.  See the [retained review receipt and four raw logs][review-evidence].

For the negative control, keep product source at the baseline and overlay **only** the same four test files.  This is intentionally a dirty test fixture, not a claim that the baseline originally shipped these regressions:

```bash
git worktree add --detach ../gloss-baseline 95d99220d107635a4a313c575ffd5306c0ffbee8
git archive 9889437763f2b8e3a846dad27c8d6ed2ce50d3ee \
  src/stores/__tests__/notebookStore.test.ts \
  src/stores/__tests__/conversationFreshness.test.ts \
  src/stores/__tests__/sourceStore.test.ts \
  scripts/tests/test_native_click_readiness.py | tar -x -C ../gloss-baseline
ln -s "$(pwd)/node_modules" ../gloss-baseline/node_modules
cd ../gloss-baseline
npm run test:unit -- src/stores/__tests__/notebookStore.test.ts \
  src/stores/__tests__/conversationFreshness.test.ts \
  src/stores/__tests__/sourceStore.test.ts
python3 -m unittest discover -s scripts/tests -p test_native_click_readiness.py -v
```

Expected and observed: frontend exit 1 with eight failures and 14 passes; readiness exit 1 with two failures.  Run both commands even though the first fails.  These expected failures establish that the fixtures discriminate the changed mechanisms; a green run on the old source requires investigation.

Rust-equipped reviewers can run the existing owners with the repository toolchain:

```bash
cargo test --locked --manifest-path validation/native_harness/Cargo.toml portable::boundary_regressions
cargo test --locked --manifest-path validation/native_harness/Cargo.toml extract::fresh_document_regressions
cargo test --locked --manifest-path validation/native_harness/Cargo.toml repeated_shared_strings_cannot_amplify_worksheet_output_past_the_limit
cargo test --locked --manifest-path validation/native_harness/Cargo.toml --test media_identity
```

These Rust commands are proposed replay here, not October 8 execution.  The native harness compiles actual owner modules and explicitly excludes full Tauri IPC, the job runner, and desktop/model execution.

### Full Linux qualification and artifact inspection

Use the existing [CI workflow][workflow] on a genuine disposable GitHub-hosted runner.  Its `workflow_dispatch` can target the existing `codex/gloss-fresh-audit-20260930` branch **after verifying it still resolves to the final head above**.  Record the new run ID, checkout revision/tree/digest, and attempt; a changed ref qualifies a different snapshot.  The workflow supplies Linux libraries, WebKit driver, Xvfb/D-Bus, the pinned tools, isolated Ollama service/model directories and the three-repetition matrix.  Rust build caches are used; fresh runtime/profile repetitions do not imply cache-free compilation.  Do not spoof `GITHUB_ACTIONS` or `RUNNER_ENVIRONMENT`, reuse a user's Ollama cache, lower timeouts, or replace models to obtain this protocol's passing status.

The build command is `npm run verify -- --receipt .codex-run-receipts/build-verification.json`.  The integrated native command is `xvfb-run -a dbus-run-session -- python3 scripts/live_ollama_canary.py --repo . --desktop --output .codex-run-receipts/desktop-ollama-workflow`.  Each package repetition runs `xvfb-run -a dbus-run-session -- python3 validation/gloss_installer_smoke_gate.py --repo . --build --require-integrated --receipt .codex-run-receipts/linux-package/receipt.json`.  These are extracted from CI, not executed in this review.  They build and replay the actual extracted AppRun; they do not establish system installation or upgrade behavior.

For run 36736177399, inspect `build-verification-*`, `desktop-ollama-workflow-*`, `live-ollama-canary-*`, `native-sanitizers-*`, and all three `linux-package-*-repetition-N-attempt-1` artifacts.  Match archive SHA-256 against the Actions artifact metadata, check archive integrity, then read parent/child receipts, source/model/package identities, case results and referenced-file hashes.  Artifact IDs for package repetitions 1/2/3 are `11108396822`, `11108341819`, `11106769811`; integrated desktop is `11107289153`.  On October 8 these metadata entries were unexpired with runtime-artifact expiry October 14.  Availability can change.  If receipts expire or downloads fail, retain that gap and rerun the pinned source; do not promote job summaries into independently inspected child evidence.

## Additional review results and remaining proof debt

The October 8 clean merge checkout passed full `npm test` (200 unit tests and 14 static contracts), `npm run build`, the five documented source-send/event-routing/terminal/provider/receipt static gates, and focused verification/desktop/model/package contract suites.  The frontend build retained its existing chunk-size advisory.  Rust formatting, application tests, Clippy, desktop execution, providers, sanitizers, and AppImage replay were **not rerun locally**: Cargo and the genuine hosted native/model environment were unavailable.  Their historical results remain log evidence, not new runtime execution.

The inspected repositories and connected indexed notes contained the existing audit and accomplishments review, but no equivalent completed PR #10 case study.  That is bounded search coverage, not proof that no private or unindexed draft exists elsewhere.  This document adds a source-linked engineering narrative and a fresh negative-control replay, and leaves the original audit as the owner of its historical findings.

Unproven: operation on an existing installed user environment or live notebook; cached Nomic inference; broad provider/model/media combinations; large corpora and long-running load; non-Linux platforms; signing; public binary release; upgrade/install/rollback qualification; general security, compliance, customer or revenue outcomes.  No performance superiority or broad production maturity follows from these tests.

**Reviewer inspection order:** verify revisions and digest, compare the owner diffs, replay the focused positive and negative controls, inspect the logged canonical receipt, then inspect runtime/package child artifacts or obtain a fresh pinned-source qualification.  Review public wording only within those bounds.

**Recovery and rollback:** this documentation change can be reverted independently.  Product rollback is a coherent revert of PR #10's focused repair commits after reviewing the inherited failure risks.  Preserve canonical notebook/source history; rebuild derived indexes through existing owners rather than forcing readiness.  Do not purge orphan files or silently rewrite extracted content.  This review changes no live notebook, installed application, release, or deployment.  The engineering evidence supports review of a Linux qualification case; merge and release decisions remain separate.

[pr]: https://github.com/RecursiveIntell/Gloss/pull/10
[repair]: https://github.com/RecursiveIntell/Gloss/commit/67e5a3bb2fe28c27f4e4ce40f14eac7469fa9653
[head]: https://github.com/RecursiveIntell/Gloss/commit/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee
[run]: https://github.com/RecursiveIntell/Gloss/actions/runs/36736177399
[audit]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/docs/audits/2026-09-30-merged-source-audit.md
[snapshot]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/scripts/source_snapshot.py
[notebook]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src/stores/notebookStore.ts#L48-L59
[conversation]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src/stores/chatStore.ts#L90-L106
[scope]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src/stores/sourceStore.ts#L16-L21
[portable]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src-tauri/src/db/portable.rs#L419-L443
[extract]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src-tauri/src/ingestion/extract.rs#L297-L426
[workspace]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src-tauri/src/jobs/media_workspace.rs
[notebook-tests]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src/stores/__tests__/notebookStore.test.ts#L30-L56
[conversation-tests]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src/stores/__tests__/conversationFreshness.test.ts
[scope-tests]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src/stores/__tests__/sourceStore.test.ts#L70-L85
[portable-tests]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src-tauri/src/db/portable.rs#L1025-L1071
[extract-tests]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/src-tauri/src/ingestion/extract.rs#L1582-L1609
[media-test]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/validation/native_harness/tests/media_identity.rs
[click-tests]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/scripts/tests/test_native_click_readiness.py
[verify-job]: https://github.com/RecursiveIntell/Gloss/actions/runs/36736177399/job/109958374291
[live-job]: https://github.com/RecursiveIntell/Gloss/actions/runs/36736177399/job/109958374687
[desktop-job]: https://github.com/RecursiveIntell/Gloss/actions/runs/36736177399/job/109958374702
[package-1]: https://github.com/RecursiveIntell/Gloss/actions/runs/36736177399/job/109958374898
[package-2]: https://github.com/RecursiveIntell/Gloss/actions/runs/36736177399/job/109958374936
[package-3]: https://github.com/RecursiveIntell/Gloss/actions/runs/36736177399/job/109958374807
[failed-baseline]: https://github.com/RecursiveIntell/Gloss/actions/runs/36724544998
[failed-followup]: https://github.com/RecursiveIntell/Gloss/actions/runs/36732643114
[workflow]: https://github.com/RecursiveIntell/Gloss/blob/9889437763f2b8e3a846dad27c8d6ed2ce50d3ee/.github/workflows/ci.yml
[review-evidence]: evidence/gloss-2026-10-08/review.json
