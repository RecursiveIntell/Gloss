# Fresh hostile review of the merged Gloss repair

Baseline: [`95d99220d107635a4a313c575ffd5306c0ffbee8`](https://github.com/RecursiveIntell/Gloss/commit/95d99220d107635a4a313c575ffd5306c0ffbee8), the merge of [PR #9](https://github.com/RecursiveIntell/Gloss/pull/9). Its tree is `d587ddfbc96a9df0fa6c69fc1fb277494e9a000c`. The new pass inspected that source and executed new counterexamples; it does not relabel the first audit's findings as new evidence.

This report describes twelve distinct audit scopes, not twelve independent auditors. A separate reviewer independently reproduced the stale-list and export-ownership failures and reviews the bounded repair delta. No full security, release, large-corpus, or installed-user-notebook certification is implied.

## Scope and evidence map

| Audit angle | Current owners and inspected boundary | Result and executable gate |
| --- | --- | --- |
| 1. Frontend request ownership | `notebookStore.ts`, `chatStore.ts`, `noteStore.ts`: overlapping list reads, newer mutations, notebook identity reuse, old failures | R2-01: six deferred-response counterexamples failed before repair; notebook/conversation owner tests cover reordered success, old failure, A→B→A and background refresh |
| 2. Retrieval scope and citation identity | `sourceStore.ts`, `retrieval/source_scope.rs`, `retrieval/citations.rs`, semantic adapter mapping and freshness checks | R2-06: both source and group toggles widened a stale explicit set to all; two actual-store deletion/refresh probes failed before repair. Backend invalid-ID and citation/backpointer gates remain required |
| 3. Canonical persistence and export privacy | `db/portable.rs`, source row/file deletion, `db/notebook_lifecycle.rs`: database snapshot versus filesystem leftovers | R2-02: independent actual-owner export admitted an unreferenced secret into a valid manifest; two new privacy gates plus existing WAL, tampering, rollback and roundtrip tests |
| 4. Replay, terminal persistence and cancellation | `chatStore.ts`, chat emit/terminal owners, notebook admission, provider cancellation and NDJSON/SSE decoders | No additional confirmed defect in the inspected sequence/terminal paths. Native owner and frontend replay tests cover duplicates, real repeated tokens, ownership, EOF, cancellation and errors; final native workflow must still pass |
| 5. Embedding identity and recovery | `embedding_cache.rs`, `embedding_contract.rs`, settings invalidation, semantic adapter metadata and strict TurboQuant admission | No new confirmed failure in the inspected identity gates. Cache-layout, stale-metadata, settings and native dense owner tests apply. Actual cached Nomic inference remains outside local proof |
| 6. Provider authority and configuration | `providers/mod.rs`, `providers/background.rs`, settings update/model registry, provider adapters | No new confirmed egress/configuration defect in inspected paths. Current dispatch rechecks provider/model grants; redirect, proxy, stale background grant and response-contract owner tests remain gates |
| 7. IPC, imports and local privacy | Tauri CSP/capability files, URL consent/DNS pinning, legacy Office child process and capture files | R2-07: named ambient-permission captures could expose plaintext and survive early error paths. Actual subprocess fixtures now inspect inherited capture FD permissions/link count and success/failure/overflow cleanup |
| 8. Document semantic integrity | `ingestion/extract.rs`, import capability declarations, real ZIP fixture boundaries | R2-03/04/05: PPTX presentation order, rich XLSX string indices, XML entities/CDATA/formatting boundaries all failed executed baseline owner probes; follow-up tests use manifest relationships and explicit cell identities |
| 9. UI and accessibility | `useModalFocus.ts`, `PanelLayout.tsx`, source viewer, notes and chat panels: focus ownership, rendering, async drafts, toolbar reachability | R2-09: post-merge native evidence exposed a missing modal-to-control admission observation. Readiness now observes closed viewer, stable enabled/non-inert hit-owned Settings and performs one native click; the strict native gate still must pass |
| 10. Resource bounds and subprocess lifecycle | XML/archive limits, repeated shared-string expansion, provider response bounds, audio/video external-tool deadlines | R2-08: a short worksheet could repeatedly expand a large shared string before the final output guard. New expansion-time guard has a failing-then-passing regression. R2-10: imported source identities could choose video scratch/cleanup paths. Private per-job RAII workspaces isolate video and audio outputs. Legacy output monitoring checks both streams during execution; it is not a hard OS quota |
| 11. Build and dependency closure | Root workspace, application manifest, native harness, lockfiles, frontend build, canonical verifier | Extractor is now compiled from its actual application file by the native owner harness. Every registry package's name/version/source/checksum is matched to root `Cargo.lock`; no dependency versions were intentionally upgraded |
| 12. Packaging and public evidence | `ci.yml`, `verify_release.py`, locked AppImage builder, README proof boundaries | Required final-candidate gates remain full Tauri profiles/tests/Clippy, native real-model workflow, and three planned independent fresh-profile package repetitions. No assertion, repetition, timeout or success requirement is relaxed |

## Confirmed repair roots

### R2-01 — Superseded list reads replace newer state

Severity: P2; executed. A slow pre-delete notebook or conversation read could resolve after the fresh list and reintroduce the deleted row. Notebook identity checks alone also allowed A→B→A stale conversation results and obsolete error toasts. The canonical stores now fence list reads by latest request; conversation reads additionally bind activation identity and reset generation. Inactive refresh requests cannot supersede the active read. Backend persistence is unchanged. Reverting the bounded store delta is the rollback; no user rows were altered by these tests.

### R2-02 — Export includes source assets without database owners

Severity: P1 privacy; independently executed. Export recursively copied the entire `sources/` directory after snapshotting SQLite. A leftover file from a failed unlink, or a copied but not registered import, was accepted into a self-consistent package. Export now enumerates unique `file_path` identities from the snapshot and copies only those assets. Package validation rejects an extra source asset even if its hash and manifest digest are internally valid. Canonical source files, nested paths, hashes, WAL content and restore invalidation remain checked. Original orphan files are retained locally; no purge is performed. Three older tampering/roundtrip fixtures were corrected to register their intended source file, preserving their original assertions.

### R2-03 — Office filename order substitutes for document order

Severity: P2; PPTX counterexample executed. ZIP filename sorting is not the presentation's relationship order and does not establish workbook sheet identity. PPTX and XLSX now follow the owning presentation/workbook manifest through internal relationships. Missing, external, duplicate or unsafe referenced parts fail visibly; unrelated ZIP parts are not promoted. Worksheet names and row/cell addresses are retained. Existing miniature positive ZIP fixtures now contain their required manifests. This changes future extraction; existing source text is not silently rewritten.

### R2-04 — Rich shared-string runs shift spreadsheet indices

Severity: P2; executed. Each text run was incorrectly counted as a shared-string item. A two-run first item made cell index 1 return the second run instead of the next item. Shared strings are now grouped by `si`, including empty items, with rich runs concatenated. Invalid indices fail rather than disappearing. Row and cell identities avoid flattening unrelated cells into an unlabelled value stream. Existing notebooks require an explicit retry/reimport to replace previously extracted content.

### R2-05 — XML event boundaries corrupt source text

Severity: P2; executed. General references and CDATA were dropped, and formatting runs acquired invented spaces. The reader now decodes those text events, preserves inline adjacency, and separates block boundaries. Unknown references fail; script/style content remains excluded from EPUB text. Output limits apply during accumulation. The red DOCX probe expected `microscope & <secret>` and observed corrupted text before repair.

### R2-06 — Equal selection size silently widens retrieval

Severity: P1 scope integrity; executed. While a selection save was pending, deleting a selected source retained its explicit ID for diagnostics. Selecting one surviving source then made the stale set's size equal the current source count and incorrectly selected `all`, admitting an unselected source. Both individual and group toggles now require exact membership as well as equal size before `all` is allowed. Invalid IDs remain explicit for backend reporting. No provider authority or retrieval fallback policy is expanded.

### R2-07 — Legacy extraction writes shared-temp plaintext

Severity: P1 local privacy; baseline and repaired capture behavior executed. Legacy Office stdout/stderr used named files with ambient permissions and manual cleanup after fallible reads. Captures are now anonymous handles, explicitly mode 0600 on Unix, with automatic handle cleanup on every return. A child guard terminates/reaps the extractor on error; deadline and observed-size checks cover both streams. The same isolated fixture applied to the baseline extractor witnessed mode 644/link count 1. Repaired disposable Linux subprocess fixtures witness inherited FD mode 600/link count 0 and cleanup for success, tool failure and excessive output. Size polling can overshoot between observations and is not an OS disk quota. Other-platform capture semantics require platform testing.

### R2-08 — Shared-string expansion bypasses the early output budget

Severity: P2 resource integrity; executed. Repeating one large shared-string reference could allocate expanded values before the final rendering limit. A three-cell probe expanded a 500,000-byte shared string past the one-million-byte budget. The worksheet owner now rejects expansion while accumulating cells. The guard preserves the existing output bound and does not truncate content or report partial text as successful extraction.

### R2-09 — Native test clicks before observing modal cleanup

Severity: verification blocker. The [post-merge run](https://github.com/RecursiveIntell/Gloss/actions/runs/36724544998) passed seven jobs, including all three package repetitions, but failed the integrated desktop case waiting for Settings. The preserved trace shows the Settings click about 25 ms after the source viewer close click, without observing closed/non-inert/hit-ready state. The failure screenshot showed the viewer closed and Settings absent. This supports an admission-race hypothesis; the exact inert state at the failed click was not recorded, so it is not presented as a conclusively reproduced application defect.

The driver now observes viewer absence and stable, enabled, non-inert, hit-owned Settings before one native click. Two tests execute the actual readiness JavaScript against a bounded DOM fixture: persistent inert state must produce no click, and transient inert state must be released before admission. Both failed before repair. The native 30-second deadline and mandatory cases remain unchanged; only final-candidate native/package execution can establish the runtime result.

### R2-10 — Imported source IDs choose video scratch and cleanup targets

Severity: P1 security; independently source-confirmed, package/owner boundaries executed. Imported source IDs are opaque database strings. The video job joined that value beneath `_tmp_frames_` and subsequently wrote frames and recursively removed the resulting directory. An absolute or traversing imported ID could therefore select an unrelated target on explicit retry. Audio used generated IDs but left transcript scratch behind on some failure paths.

Both jobs now own fresh private `TempDir` values from one media-workspace module, with fixed purpose prefixes and no source-ID argument. RAII handles early-error cleanup. Tests exercise distinct workspace identities, mode 0700, partial-output cleanup, and an actual package roundtrip containing an absolute source ID while preserving an unrelated sentinel through scratch cleanup. That fixture does not execute ffmpeg, Whisper or the Tauri job runner; the call-site connection is source-reviewed and must compile in full application CI. No imported IDs are rewritten and no UUID-only compatibility rule is introduced.

## Validation and remaining boundary

Baseline source checks: 192 frontend tests plus contracts/build and 232 native unit tests plus five integration tests passed. New tests established red outcomes before the corresponding list, export, document and retrieval-scope repairs. Failed attempts are retained; corrected test fixtures are described above.

At this report's local checkpoint: 200 frontend tests plus contracts/build, 247 native unit tests plus six integration tests, and the new extraction/privacy fixtures pass. Counts overlap and must not be summed as unique tests. The explicitly ignored extractor child is executed by its parent isolation test; the existing real-model environment probe remains separate.

The first follow-up candidate passed all three package repetitions and the integrated desktop job, but its canonical verifier stopped at the dependent TurboQuant harness lockfile. Adding extraction owners expanded the native harness dependency graph; the downstream lockfile also needed refresh. The failed [candidate run](https://github.com/RecursiveIntell/Gloss/actions/runs/36732643114) is preserved. Both harness lockfiles now follow the root registry identities; `--locked` remains required and final-source CI must rerun.

An exploratory strict all-targets Clippy run on the native harness reports the same five lints on the untouched baseline and repair: one public-harness `from_str` trait suggestion and four existing test-only read/clone suggestions. This optional harness result is retained as failed; it is not substituted for or used to relax the canonical strict application Clippy gate.

Full Tauri compilation, native GUI/model workflow, sanitizer results and the three AppImage repetitions must be read from the follow-up PR's exact-candidate CI and attached receipts. Local pure-owner tests do not certify those stages. The README retains this distinction and the existing installed-host, real Nomic, large-corpus, media, non-Linux, signing and public-release limits.

Rollback is a normal revert of the focused repair commit(s). All executed fault fixtures use disposable stores and inputs. No live notebook, deployment, signing configuration or published release is changed by this pass.
