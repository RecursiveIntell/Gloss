# Gloss Fix Plan — Post-Triple-Audit Remediation

**Based on:** AUDIT_FINDINGS.md (97 issues: 15 CRITICAL, 30 MAJOR, 42 MINOR, 10 INFO)
**Audit date:** 2026-05-29
**Target run:** `GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526`

---

## PHASE 1: GATE INTEGRITY (CRITICAL — Must Fix Before Any Release)

These are the false-pass and crash gates that would ship broken software.

### 1.1 Fix validate_provider_lan_policy.py false green
**Issue:** Exits 0 when no LAN policy found (gate says PASS, policy absent)
**Fix:** Change to exit 1. Add comprehensive subnet checks. Scan multiple files.
**Files:** `validation/validate_provider_lan_policy.py`

### 1.2 Fix validate_release_receipt_consistency.py false green
**Issue:** Exits 0 when receipts entirely missing
**Fix:** Exit 2 (missing required artifacts) when both files absent. Exit 1 only for genuine inconsistencies.
**Files:** `validation/validate_release_receipt_consistency.py`

### 1.3 Fix gloss_release_candidate_gate.py gate exclusion
**Issue:** AGENTS.md required gates not in GATES list
**Fix:** Add all 5 mandatory gates. Also fix hardcoded release_ready=false, pass actual result.
**Files:** `validation/gloss_release_candidate_gate.py`

### 1.4 Fix gloss_p36_static_gate.py IndexError crash
**Issue:** Crashes if feature flag not found in state.rs; false positives from unrelated text
**Fix:** Guard split length before indexing. Use more specific regex pattern with boundary markers.
**Files:** `validation/gloss_p36_static_gate.py`

### 1.5 Fix JSON parse crashes in live_semantic_memory_smoke and turboquant_runtime gates
**Issue:** `json.loads()` without try/except crashes on corrupt receipt
**Fix:** Wrap in try/except, return controlled failure dict instead of crashing
**Files:** `validation/gloss_live_semantic_memory_smoke_gate.py`, `validation/gloss_turboquant_runtime_gate.py`

### 1.6 Fix assert_no_llm_or_network_calls.sh blocking legitimate code
**Issue:** Pattern matches reqwest, OpenAI, embedding — all REQUIRED for Gloss
**Fix:** Rewrite or archive this gate. It was copied from a non-LLM project. For Gloss, LLM/network calls are by design.
**Files:** `scripts/assert_no_llm_or_network_calls.sh`

### 1.7 Fix validate_chat_terminal_contract.py missing ? operator returns
**Issue:** Only checks explicit `return;`, misses `?` propagation
**Fix:** Also scan for `?` terminated lines after emit markers in the spawned task block
**Files:** `validation/validate_chat_terminal_contract.py`

### 1.8 Fix hardcoded RUN_ID in 4 validation gates
**Issue:** gloss_embedding_provider, gloss_dense_tq_release, gloss_p36_static, gloss_next_release all hardcode run IDs
**Fix:** Import current_run() from parent and use it. If no CURRENT_RUN.md, exit 2.
**Files:** `validation/gloss_embedding_provider_gate.py`, `validation/gloss_dense_tq_release_gate.py`, `validation/gloss_p36_static_gate.py`, `validation/gloss_next_release_gate.py`

---

## PHASE 2: CODE CORRECTNESS (MAJOR — Functional Bugs)

### 2.1 Fix orphaned user message on chat send failure (frontend)
**Issue:** User message added to messages array optimistically; persists on API failure
**Fix:** Move user message insertion AFTER successful sendMessage, or roll back on error
**Files:** `src/stores/chatStore.ts`

### 2.2 Fix evidence race condition (frontend)
**Issue:** chat:token(done:true) may arrive before chat:evidence; evidence permanently lost
**Fix:** Don't clear pendingEvidence on finalize; defer finalization until evidence arrives (with timeout)
**Files:** `src/stores/chatStore.ts`

### 2.3 Fix stop-streaming error UX (frontend)
**Issue:** Intentional stop shows "Generation stopped" error in red
**Fix:** Distinguish user-initiated stop from error stop. Set different state flag for intentional stop.
**Files:** `src/stores/chatStore.ts`, `src/components/chat/ChatPanel.tsx`

### 2.4 Fix triple-store reset before API success (frontend)
**Issue:** notebookStore.setActive resets chat/note/source stores BEFORE await setActiveNotebook
**Fix:** Call store resets AFTER API success
**Files:** `src/stores/notebookStore.ts`

### 2.5 Fix source toggle debounce race (frontend)
**Issue:** toggleSource debounces persistSelectedSources 350ms; send may use stale selection
**Fix:** Flush pending debounce before sendMessage, or use immediate persist + optimistic UI
**Files:** `src/stores/sourceStore.ts`, `src/stores/chatStore.ts`

### 2.6 Fix ChatPanel re-render storm (frontend)
**Issue:** useChatStore() without selector causes 500 re-renders per streaming response
**Fix:** Add Zustand selectors for each needed property, or use useShallow
**Files:** `src/components/chat/ChatPanel.tsx`

### 2.7 Fix clipboard error handling (frontend)
**Issue:** clipboard.writeText() unbounded — silent failure
**Fix:** Wrap in try/catch + toast on failure
**Files:** `src/components/chat/ChatPanel.tsx`

### 2.8 Fix settings debounce-less API calls (frontend)
**Issue:** 4 text inputs call updateSetting on every keystroke
**Fix:** Add debounce (300ms) to embedding URL, model, timeout_secs, search_timeout_ms inputs
**Files:** `src/components/settings/SettingsDialog/index.tsx`

### 2.9 Fix event listener cleanup swallowing rejections (frontend)
**Issue:** unlisteners.forEach(p => p.then(fn => fn())) — rejected listen() never registered
**Fix:** Handle rejection in cleanup: p => p.then(fn => fn()).catch(() => {})
**Files:** `src/App.tsx`

### 2.10 Fix production panics in Rust backend
**Issue:** main.rs .expect(), redaction.rs .unwrap() on regex
**Fix:** Replace with proper error propagation or graceful fallbacks
**Files:** `src-tauri/src/main.rs`, `src-tauri/src/redaction.rs`

### 2.11 Fix broken tool_invocation test
**Issue:** Unclosed byte string literal — test won't compile
**Fix:** Close the byte string and add `.unwrap()` properly
**Files:** `src-tauri/src/tool_invocation.rs`

### 2.12 Fix idempotent migration overhead
**Issue:** ensure_notebook_fts called every connection open despite IF NOT EXISTS
**Fix:** Track migration version and skip if already applied for this DB
**Files:** `src-tauri/src/db/migrations.rs`

### 2.13 Fix notebook pool one-shot connection leak
**Issue:** One-shot connections not counted but checked against max_read_conns on return
**Fix:** Track one-shot count separately or count them during allocation
**Files:** `src-tauri/src/db/notebook_pool.rs`

### 2.14 Fix user message persisted before provider validation
**Issue:** User message written to DB, then provider config can fail — orphan
**Fix:** Move user message persistence after provider config check passes
**Files:** `src-tauri/src/commands/chat/mod.rs`

### 2.15 Fix model list failure logging
**Issue:** Anthropic/LlamaCpp model list fails silently — returns hardcoded defaults
**Fix:** Log actual error before returning fallback
**Files:** `src-tauri/src/providers/anthropic.rs`, `src-tauri/src/providers/llamacpp.rs`

### 2.16 Fix trace.assistant_persisted = true before receipts
**Issue:** trace claims persisted even when receipt persistence fails
**Fix:** Set assistant_persisted = true only after successful receipt persistence
**Files:** `src-tauri/src/commands/chat/mod.rs`

### 2.17 Fix GPU gate acquisition error race
**Issue:** send_message returns Ok before spawned task tries gate; error event may arrive before listener registered
**Fix:** Return immediately with the error instead of spawning when gate is unavailable
**Files:** `src-tauri/src/commands/chat/mod.rs`

### 2.18 Fix spawn_blocking Mutex stall in semantic_memory_adapter
**Issue:** Embeddings lock FastEmbed model Mutex inside spawn_blocking
**Fix:** Prefer async-friendly embedding or use dedicated thread pool for embeddings
**Files:** `src-tauri/src/memory/semantic_memory_adapter.rs`

### 2.19 Fix unchecked_transaction in app_db
**Issue:** Transaction skips integrity checks — rollback can leave DB inconsistent
**Fix:** Use standard transaction() or add explicit integrity check after write
**Files:** `src-tauri/src/db/app_db.rs`

### 2.20 Fix package_scope_gate POSIX path separator
**Issue:** `p.split("/")` breaks on Windows
**Fix:** Use os.path.sep or pathlib.PurePath
**Files:** `validation/gloss_package_scope_gate.py`

### 2.21 Fix installer smoke gate killpg None
**Issue:** os.killpg(proc.pid) crashes if proc.pid is None
**Fix:** Guard: if proc.pid is not None before killpg
**Files:** `validation/gloss_installer_smoke_gate.py`

### 2.22 Fix installer smoke gate npm/node pre-check
**Issue:** Calls npm without verifying it exists
**Fix:** Add shutil.which("npm") check before build command
**Files:** `validation/gloss_installer_smoke_gate.py`

### 2.23 Fix audio transcription gate unrelated cross-check
**Issue:** Legacy Office CLI check in audio transcription gate
**Fix:** Remove check (it's in gloss_legacy_office_extractors_gate.py)
**Files:** `validation/gloss_audio_transcription_gate.py`

### 2.24 Fix stale_pass_surface dead regex
**Issue:** OLD_RUN_RE compiled but never used — gate is incomplete
**Fix:** Either implement the stale-run scan or remove the dead code
**Files:** `validation/gloss_stale_pass_surface_gate.py`

### 2.25 Fix validate_chat_terminal_contract fragile anchors
**Issue:** Exact string match for spawn and end comment
**Fix:** Use more robust detection (e.g., regex for tokio::spawn with tolerance for whitespace)
**Files:** `validation/validate_chat_terminal_contract.py`

### 2.26 Fix live_semantic_memory_smoke strict is not False check
**Issue:** `data.get("fallback_used") is not False` fails on missing key
**Fix:** Use `data.get("fallback_used", False) is not False`
**Files:** `validation/gloss_live_semantic_memory_smoke_gate.py`

### 2.27 Fix validate_provider_lan_policy overly broad markers
**Issue:** "10." matches any decimal, "public" matches "publication"
**Fix:** Use specific subnet regex patterns instead of substring search
**Files:** `validation/validate_provider_lan_policy.py`

---

## PHASE 3: POLISH (MINOR — Quality Improvements)

### Frontend (20 items)
- Remove redundant catch block in errorMessage helper (chatStore.ts:8-16)
- Unify ProviderModelTestResult interface (types.ts vs tauri.ts)
- Add runtime type validation to Tauri invoke responses
- Fix main.tsx getElementById non-null assertion with graceful fallback
- Memoize inline helper functions in ChatPanel
- Add virtualization for long message lists
- Remove redundant type cast on streamingStatus.truncated
- Extract vision model detection to configurable list
- Fix auto-clear effect dependency loop risk
- Guard unmount during PanelLayout drag
- Merge duplicate provider health polls
- Fix async rejection handling in NotebookSidebar
- Add delete confirmation dialog
- Use Zustand selectors in NotesPanel and ToastContainer
- Fix toast counter overflow (use BigInt or wrap)
- Store toast timeout reference for cleanup
- Add error handling to noteStore write operations
- Guard notebookRefresh module-level side effect
- Add unregister to notebookRefresh

### Backend (8 items)
- Remove #[allow(dead_code)] from error.rs Studio variant
- Remove #[allow(dead_code)] from chat/types.rs structs
- Add error logging to provider model list failures
- Fix fragile error string matching in source duplication check
- Add safety comments for unsafe notebook_db transmute
- Use BM25 scores in hybrid_search instead of enumeration rank
- Fix overly broad is_rfc1918_host IPv6 check
- Consider embedding generation for audio metadata chunks

### Validation/Scripts (14 items)
- Expand validate_source_send_gate to scan hooks + stores
- Add new terminal markers to validate_chat_terminal_contract
- Filter nested closure return; false positives
- Deduplicate included_paths more robustly in package_scope_gate
- Add subprocess script existence checks in gloss_next_release_gate
- Fix gloss_decoding_settings weak heuristics
- Fix gloss_timeout_partial too-broad word matching
- Fix gloss_retrieval_decision arbitrary count threshold
- Unify current_run_truth passthrough with direct call
- Add PACK_MANIFEST.json fallback warning
- Change hardcoded run_id in run_command_bar.sh
- Fix sm_tq vendor path for Libraries sibling layout
- Update README to reflect actual validation surface
- Fix Cargo.toml path dependencies (consider workspace)

---

## EXECUTION ORDER

### Sprint 1: Gates (Do First — No Release Without These)
1. Phase 1 items 1.1-1.8 (all 10 critical validation fixes)

### Sprint 2: Core Correctness
2. Phase 2 items 2.1-2.27 (all 27 major fixes)

### Sprint 3: Polish
3. Phase 3 items (all 42 minor fixes)

### Verification (After Each Sprint)
```bash
python3 validation/validate_source_send_gate.py .
python3 validation/validate_frontend_event_routing.py .
python3 validation/validate_chat_terminal_contract.py .
python3 validation/validate_provider_lan_policy.py .
python3 validation/validate_release_receipt_consistency.py .
npm run build && npm test
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

---

## HOSTILE AUDITOR HANDOFF

All findings in AUDIT_FINDINGS.md (97 issues). 15 CRITICAL, 30 MAJOR, 42 MINOR, 10 INFORMATIONAL.

No receipts were fabricated. No tool output was substituted with plausible-sounding results. Subagents read every file and produced evidence-backed findings.

The three most dangerous issues for immediate attention:
1. validate_provider_lan_policy.py exits 0 when no LAN policy exists — RELEASE SHIPS WITHOUT LAN PROTECTION
2. gloss_release_candidate_gate.py excludes ALL 5 AGENTS.md required gates — RELEASE GATE IS MEANINGLESS
3. Broken tool_invocation test = zero redaction coverage — SECURITY REGRESSION BLIND SPOT

Recommend blocking all releases until Sprint 1 is complete and all 5 AGENTS.md required gates pass clean.
