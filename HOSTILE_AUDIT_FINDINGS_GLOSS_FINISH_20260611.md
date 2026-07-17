# HOSTILE AUDIT FINDINGS — GLOSS FINISH 2026-06-11

Repository: `/home/sikmindz/Coding/Gloss`
Branch: `perf-slowdown-fix-20260610`
Commit before: `26208eb` (Batches G/H/I close-out)
Commit after: `f4d1b74` (this pass)
Run: `GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526`
Dirty state before: 18 uncommitted files from prior sessions
Dirty state after: clean (all work committed)

## Scope completed

The user directive was "finish everything up. I need the program fixed and stable." This pass:

1. Verified the current state of all mandatory gates and tests with the
   18 prior-session uncommitted changes in place.
2. Identified 3 real gate-script bugs that were failing on stale literal
   tokens (the source of truth had drifted; the gates had not).
3. Identified 3 real dead-code / compatibility-shim items that violated
   doctrine ("no compatibility shims" / "do not claim release from
   dependencies").
4. Reverted 1 harmful uncommitted change that would have made the
   package-scope gate flag legitimate toolchain dirs as violations.
5. Committed all surviving work as a single finishing commit.
6. Wrote this handoff.

### Gate-script bugs fixed (source of truth = code, not the literal gate)

| # | File:line | Problem | Fix |
|---|---|---|---|
| G1 | `validation/gloss_security_egress_gate.py:53` | Gate expected `validate_provider_base_url(provider_type, candidate_url, allow_lan)` as a literal substring. Real call at `src-tauri/src/commands/settings.rs:278` is `validate_provider_base_url(provider_type, &candidate_url, allow_lan)` (correct: function takes `&str`). | Gate now accepts both forms. Release-candidate gate is now green. |
| G2 | `scripts/chat_runtime_static_audit.py:30-31` | Gate expected `const unlisten = onChatToken(...)` literal. Real shape in `src/App.tsx:157` is `unlisteners.push(onChatToken((payload) => { ... }))`. Behavior was correct, literal was stale. | Replaced with regex `onChatX\s*\(\s*\(payload` that matches the call-site (not the import) and the same activeNotebookId filter check. Static audit now 9/9 pass. |
| G3 | `scripts/validate_codex_pack.py:31` | Gate looked for `PACK_MANIFEST.json` at repo root. Active pack manifest has been at `docs/PACK_MANIFEST.json` since the SUPERPASS run. | Lookup falls back from `docs/` to root. Codex pack validation now passes. |

### Dead-code / shim items removed

| # | File:line | Item | Disposition |
|---|---|---|---|
| D1 | `src-tauri/src/ingestion/embed.rs:192` | `pub fn new_ollama_default(url, model)` — "backwards-compatible constructor" with 60s timeout. Zero call sites. The doc comment claimed it was for batch imports, but no batch path uses it. | DELETED. Doctrine: no compatibility shims. |
| D2 | `src-tauri/src/retrieval/hybrid_search.rs:80` | `pub fn local_retrieval_outcome(...)` — free function, used only by tests in the same file. Production callers go through `AppState::local_retrieval_outcome` (method) or `local_retrieval_outcome_with_query` (free function). | Gated as `#[cfg(test)]`. No production caller impact; tests still pass. |
| D3 | `src-tauri/src/state.rs:94` | `QueryEmbedCache::stats()` — only used by the LRU unit tests. | Gated as `#[cfg(test)]`. No production caller impact; tests still pass. |

### Harmful uncommitted change reverted

| # | File | Change | Disposition |
|---|---|---|---|
| R1 | `validation/gloss_package_scope_gate.py:14` | Working-tree diff REMOVED `.claude .hermes .vscode target node_modules .codex-runs` from the `ALLOWED_TOP_SINGLE_REPO` allowlist. Would have made the gate flag those dirs as package-scope violations, breaking the release-candidate gate. | REVERTED to HEAD via `git checkout HEAD -- ...`. No functional change. The gate was still passing at HEAD; the working-tree change was an experimental edit. |

### Carried-over uncommitted work (now committed)

These were the 18 uncommitted files in `git status` before this session.
All reviewed, verified to build + test + gate, and committed as a single
finishing commit. The prior session's `HOSTILE_AUDIT_FINDINGS_GLOSS_SLOWDOWN_FIX_20260610.md`
is the receipt for Batches A–I; this pass added the test fixups and the
dead-code / gate fixes.

| File | Nature of change | Status |
|---|---|---|
| `src-tauri/src/commands/chat/types.rs` | `provider_done_terminal_decision().emit_done_on_current_token` flipped to `false` so chat:done is not emitted before persistence; new test pins the contract. | SHIPPED |
| `src-tauri/src/db/app_db.rs` | `update_provider` preserves an existing base_url when the caller passes `None` (was clobbering with `None`); 2 new tests pin both the preserve-existing and insert-with-None paths. | SHIPPED |
| `src-tauri/src/db/notebook_pool.rs` | One-shot read connections (past `max_read_conns`) are now dropped on return instead of being cached into the pool. Pool stays bounded under burst. | SHIPPED |
| `src-tauri/src/commands/studio.rs` | `generate_studio_output` refactored into 3 phases: (1) read lock for deterministic artifact + full text collection, (2) NO lock held during LLM refinement / structured widget generation, (3) short write lock for `insert_studio_output`. Also added `is_widget_kind`, `generate_structured_widget_content`, citation injection, and re-validation. | SHIPPED |
| `src-tauri/src/studio/mod.rs` | Import surface tweak to match the commands::studio split. | SHIPPED |
| `src/components/settings/SettingsDialog/index.tsx` | New `useDebouncedSetting` hook for text/number inputs. Without it, typing "http://localhost:11434" issued 21 IPC calls. Also wires optimistic echo / rollback semantics. | SHIPPED |
| `src/components/studio/MindMapGraph.tsx` | `parseMindMap` now also handles the deterministic template branch shape `{center, branches: [{label, children}]}` in addition to the explicit `{nodes, edges}` shape. Exported as `parseMindMap` for testability. | SHIPPED |
| `src/stores/chatStore.ts` | Added `pendingMessageIds` set to guard against the token race when the backend re-assigns a different `messageId` than the frontend asked for. Notebook-switch and send-error paths now roll back the optimistically added user message. | SHIPPED |
| `src/stores/noteStore.ts` | `createNote` / `updateNote` / `deleteNote` now surface a real error toast on failure instead of `console.warn` and silent state. | SHIPPED |
| `src/stores/settingsStore.ts` | Provider URL setters revalidate through `NetworkScopePolicy` on every change so a tightened policy can refuse an old custom URL. | SHIPPED |
| `src/stores/sourceStore.ts` | `getSourceScope` preserves `all`-scope when the source list is partial-but-has-sources (so chat still works on a partial load). | SHIPPED |
| `src/stores/__tests__/sourceStore.test.ts` | Replaced a tautological "set status to ready" test with a real one asserting the partial-but-has-sources scope behavior. | SHIPPED |

## Verification (post-commit)

All checks green, all warnings cleared:

```
=== 1. npm run build ===                              ✓ built in 2.73s
=== 2. npm test ===                                   12/12 contract tests pass
=== 3. cargo fmt --all -- --check ===                 clean
=== 4. cargo test commands::chat::tests ===           2 passed
=== 5. cargo test providers::tests ===                10 passed
=== 6. validate_source_send_gate ===                  PASS
=== 7. validate_frontend_event_routing ===            PASS
=== 8. validate_chat_terminal_contract ===            PASS
=== 9. validate_provider_lan_policy ===               PASS
=== 10. validate_release_receipt_consistency ===      PASS
=== 11. gloss_release_candidate_gate ===              ok=true failed=[]
=== 12. chat_runtime_static_audit ===                 9/9 checks pass
=== 13. validate_codex_pack ===                       OK
=== 14. assert_codex_active_pack ===                  OK
=== 15. cargo test --lib --no-fail-fast ===           170 passed, 0 failed, 1 ignored
=== 16. cargo check --all-targets ===                 0 warnings
```

## Failing / skipped checks

None. The 5 AGENTS.md mandatory gates all pass. The release-candidate
gate passes. The static audit passes. The full lib test suite (170 tests)
passes. The frontend build succeeds.

The only previously-known blocker that remains unchanged is
`live_desktop_exercised=false` in `LIVE_DESKTOP_SMOKE_RECEIPT.json` —
that's documented in `FINAL_AUDITOR_HANDOFF.md` as a known unresolved
risk ("live release-grade desktop GUI driver") and is NOT in the scope
of "fixed and stable"; it requires a real Tauri GUI driver, which the
local environment does not have.

## Source-of-truth decisions

- **Gate scripts are textual guards, not compilers.** When the
  authoritative source code uses a different literal than the gate
  expects, fix the gate's literal to match the real code shape, not the
  other way around. All three gate fixes in this pass follow this
  principle.
- **Compatibility shims are doctrine violations.** `new_ollama_default`
  was deleted because it had zero callers and a misleading docstring.
  Production code uses `new_ollama(url, model, timeout_secs)` with an
  explicit budget.
- **Test-only APIs belong behind `#[cfg(test)]`.** Both
  `QueryEmbedCache::stats` and the free function
  `local_retrieval_outcome` are used only in tests, so they should not
  be present in production builds. Gating them removes the dead-code
  warnings and makes the production API surface smaller.
- **No fabrication under "I cannot" doctrine.** The reverted
  `gloss_package_scope_gate.py` change was an accidental or experimental
  edit that would have broken a release gate. Reverting to HEAD was the
  only honest move.

## Canonical owners used

- `src-tauri/src/commands/chat/mod.rs` — chat lifecycle (unchanged
  shape; `commands/chat/types.rs` carries the terminal decision).
- `src-tauri/src/db/notebook_db` and `notebook_pool` — pool semantics
  unchanged; one-shot conn drop is internal.
- `src-tauri/src/memory/semantic_memory_adapter` — unchanged.
- `src-tauri/src/ingestion/embed.rs` — `new_ollama_default` removed.
- `src-tauri/src/providers/*` — unchanged.
- `src/stores/*` — three stores hardened (chat, note, settings,
  source).
- `src/components/studio/MindMapGraph.tsx` — only branch-shape
  fallback added; explicit `{nodes, edges}` path unchanged.
- `src/components/settings/SettingsDialog/index.tsx` — debounce hook
  added; existing flow unchanged.

## Deleted / quarantined duplicate implementations

- `new_ollama_default` deleted (compatibility shim, no callers).

No compatibility shims added. No shadow truth stores. No duplicate
master docs.

## Receipt / evidence additions

- This file: `HOSTILE_AUDIT_FINDINGS_GLOSS_FINISH_20260611.md`
- Commit: `f4d1b74` on `perf-slowdown-fix-20260610`
- `docs/codex-runs/GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526/RELEASE_CANDIDATE_GATE_RESULTS.json` was rewritten with `ok: true, failed: []`.

## Boundary / schema changes

- `gloss_security_egress_gate.py`: now matches both `&candidate_url` and
  `candidate_url` as the second arg of `validate_provider_base_url`. No
  schema change.
- `chat_runtime_static_audit.py`: regex matches the call-site of
  `onChatToken` / `onChatStatus` / `onChatError` / `onChatEvidence`
  inside `src/App.tsx`. The semantic check (no `activeNotebookId` filter
  before `chatStore`) is unchanged.
- `validate_codex_pack.py`: `PACK_MANIFEST.json` lookup falls back from
  `docs/PACK_MANIFEST.json` to repo root. No schema change.

## Tests / fixtures added

This pass did not add new tests; it verified that the tests added in
prior sessions all still pass after the close-out work:

- `commands::chat::tests::provider_done_frame_does_not_emit_done_before_persistence` (2)
- `db::app_db::tests::test_update_provider_preserves_existing_base_url_when_omitted` (1)
- `db::app_db::tests::test_update_provider_inserts_new_row_when_missing_and_base_url_none` (1)
- `db::notebook_pool::tests::read_pool_does_not_grow_past_max_read_conns_under_burst` (1)
- `src/stores/__tests__/sourceStore.test.ts` partial-but-has-sources test (1)

## Known unresolved risks (carried forward, not in scope of "finish")

These are the same risks documented in the prior handoffs; this pass
did not address them because they are not "fixed and stable" blockers:

- **Live release-grade desktop GUI driver** —
  `LIVE_DESKTOP_SMOKE_RECEIPT.json` records `live_desktop_exercised=false`.
  Requires a real Tauri GUI driver with import/query/delete/restart
  evidence. Not available locally; documented as honest blocker.
- **AppImage packaging** — deb/rpm are receipt-proven; AppImage is
  not.
- **Installed package post-launch workflow smoke** — deb/rpm package
  metadata and isolated launch are proven; full workflow smoke is not.
- **Live performance certification / full benchmark** — partial
  telemetry only.
- **Legacy Office layout/rendering fidelity beyond CLI text** —
  degraded text-only extraction is proven; layout fidelity is not.
- **Audio diarization / speaker labels / automatic model download** —
  not implemented.
- **YouTube private/cookie-only captions / generated transcription** —
  not implemented.
- **URL crawling / authenticated fetch / readability fidelity** —
  one-shot text only.
- **Historical failed-import batch replay/grouping** — not implemented.

## Release blockers (carried forward)

The same blockers as `FINAL_AUDITOR_HANDOFF.md`: live release-grade
desktop GUI smoke, installed package post-launch workflow smoke,
AppImage packaging, full performance certification. This pass did not
move any of those — they require infrastructure not present in the
local repo.

## Rollback plan

```bash
git reset --hard 26208eb
```

Reverts to the prior Batches G/H/I close-out. The 18 uncommitted files
revert with it.

## Public claim decision

This pass does not change the public-claim surface from
`FINAL_AUDITOR_HANDOFF.md`. The same narrow claims remain supportable:

- Deterministic source-cited Studio artifacts
- Provider base-URL validation / error redaction
- First-use FastEmbed download consent blocking
- Local encrypted secret store with owner-only permission repair
- ffmpeg/ffprobe ToolInvocationReceipt routing
- Path redaction for selected runtime surfaces
- Strict import capability / degraded-state disclosure
- Degraded local PDF/DOCX/XLSX/PPTX/EPUB text extraction
- Degraded one-shot URL text fetch with per-import consent
- Degraded YouTube public-caption-track transcript import
- Degraded audio metadata + cached-Whisper audio transcription
- DB doctor check/repair with Settings diagnostics UI
- Failed-import quarantine-by-deselection
- Compressed notebook archive portability with sidebar UI
- Scripted desktop runtime-contract smoke with capability blocker
- deb/rpm package metadata + payload + isolated launch smoke

Do NOT claim: live desktop GUI release proof, installed package
post-launch workflow smoke, AppImage support, full release readiness,
NotebookLM parity, broad-spec completion, TurboQuant runtime
contribution.

## Next pass

If the user wants the next concrete unblock, the highest-leverage item
is the **live Tauri GUI smoke driver** — that would flip
`live_desktop_exercised` to true and unlock the live release-grade
desktop claim. That requires a real Tauri headless driver (webdriver /
tauri-driver), which is not part of this repo and would need to be
provisioned.

If "fixed and stable" is the bar and live-release-proof is out of
scope, then the program IS fixed and stable as of `f4d1b74`:
- All 5 AGENTS.md mandatory gates pass
- All 170 lib tests pass
- All 12 frontend contract tests pass
- All 9 static-audit checks pass
- Release-candidate gate passes
- 0 cargo warnings
- Clean worktree

Done. Pass.
