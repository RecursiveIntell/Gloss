# HOSTILE AUDIT FINDINGS — Gloss Closing Pass
# Date: 2026-05-29
# Run ID: GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526
# Repository: /home/sikmindz/Coding/Gloss
# Commit: dec9ba266a51da025af62665736418ed6ddc3f18
# Status: DIRTY — 4 files modified, uncommitted
#
# This report is the hostile auditor's comprehensive inventory of every
# remaining defect, gap, and risk found during a full-sweep audit. It
# combines automated gate results, manual code review, structural analysis,
# and cross-referencing against AGENTS.md rules.
# =================================================================

=================================================================
SECTION 1: GATE FAILURES (MUST-FIX BLOCKERS)
=================================================================

 1.1  gloss_package_scope_gate.py — FAIL
      Path: validation/gloss_package_scope_gate.py
      Detail: 50 top-level paths outside Gloss/Libraries in the generated
              source package manifest. The package includes the entire
              repo root (06_PHASE_PROMPTS/, checklists/, codex/, docs/,
              evidence/, fixtures/, live_check_logs/, observed_logs/,
              PHASES/, RECEIPT_TEMPLATES/, REPORT_TEMPLATES/, schemas/,
              scripts/, tables/, templates/, validation/, VALIDATION/,
              and root JSON/CSV files) which are NOT source code.
      Fix:   Add a gloss-scoped package profile that extracts ONLY
             src/, src-tauri/src/ (excluding vendor/), package.json,
             Cargo.toml, scripts/, validation/, docs/codex-runs/, and
             any required build configuration. Audit artifacts, phase
             packs, and receipts belong in a separate evidence package.
      Severity: S0 (blocks release)

 1.2  gloss_next_release_gate.py — FAIL
      Path: validation/gloss_next_release_gate.py
      Detail: Fails 9 sub-checks:
             - dense indexed_chunks must be > 0 (is 0)
             - dense live_dense_ingestion_exercised must be true (is false)
             - semantic live_projection_sources must be > 0 (is 0)
             - semantic projection passed must be true (is false)
             - TurboQuant exact_rerank must be true (is false)
             - TurboQuant exact_rerank_count must be > 0 (is 0)
             - TurboQuant/vector artifact manifest digest missing
             - live desktop smoke not exercised
             - live desktop smoke not release_grade
      This gate validates the PREVIOUS run (GLOSS_RELEASE_PROOF_...),
      not the current one. The current run (SUPERPASS_20260526) passes
      the release candidate gate. This is a historical receipt gap
      that persists in the repo.
      Fix:   Archive the stale P33 receipt, or re-run the live semantic/
             TurboQuant/desktop smoke for the current run with real data.
      Severity: S1 (blocks claiming previous run as release-ready)

 1.3  gloss_fresh_unzip_replay_gate.py — FAIL
      Path: validation/gloss_fresh_unzip_replay_gate.py
      Detail: Fails with [Errno 28] No space left on device. The replay
              extracts the full package (including vendor crates) to /tmp
              which fills up. This is an infrastructure / size issue, not
              a code defect per se, but it means we cannot prove the
              package reproduces correctly from scratch.
      Fix:   Either increase /tmp space, use a different temp directory,
              or create a slimmed package profile that excludes vendor/
              crates (see 1.1).
      Severity: S2 (reproducible, but blocks release proof)

 1.4  gloss_installer_smoke_gate.py — FAIL
      Path: validation/gloss_installer_smoke_gate.py
      Detail: Receipt shows rpm artifact missing, deb artifact missing,
              installed launch smoke missing rpm artifact. The installer
              receipt at docs/codex-runs/.../INSTALLER_SMOKE_RECEIPT.json
              was apparently generated on a prior run but the artifacts
              are no longer present (or were never present).
      Fix:   Run the actual installer build (npm run installer-smoke or
             equivalent tauri-bundler invocation) and capture the receipt
             with real artifact paths.
      Severity: S1 (blocks release)

 1.5  audit_tauri_security.py — FAIL
      Path: scripts/audit_tauri_security.py
      Detail: "dialog:allow-save is not justified by current frontend
              usage". The Tauri capability grants dialog:allow-save but
              no frontend code currently triggers a save dialog through
              the Tauri API. This is a capability over-grant.
      Fix:   Either remove dialog:allow-save from capabilities, or add
             the justification/documentation for its presence.
      Severity: S1 (security surface)

=================================================================
SECTION 2: DIRTY WORKING TREE (UNCOMMITTED CHANGES)
=================================================================

 2.1  Uncommitted modifications (4 files):
      - src-tauri/src/commands/sources/mod.rs (+2: added gemma3/gemma4
        to has_vision_capability)
      - src-tauri/src/db/migrations.rs (+1/-1: default model qwen3:8b
        changed to qwen3.5:4b)
      - src/components/settings/SettingsDialog/index.tsx (+2: added
        gemma3/gemma4 to isVisionCapableModel)
      - src/stores/settingsStore.ts (+2/-2: default model changed to
        qwen3.5:4b in TypeScript)

      These are benign model configuration updates but they are UNTRACKED
      BY VERSION CONTROL. This means:
      1. The audit was conducted against a different code state than HEAD
      2. Any build/test/gate results reflect the modified state
      3. There is no guarantee the committed code matches what was tested

      Fix: Commit or stash these changes. Do not run release validation
           on a dirty tree.

      Severity: S2 (process hygiene)

=================================================================
SECTION 3: ISSUE LEDGER — OPEN/UNRESOLVED ITEMS
=================================================================

      The root ISSUE_LEDGER.csv lists 81 issues. 24 remain open or
      partially open. Full inventory below:

 3.1  OPEN (no resolution):

      GLOSS-NP34-S1-014 (S1): Summary scheduler state
        - SummaryQueueStateV1 not exposed; UI can't tell if summaries
          are queued, blocked, manual, or failed.
        - Acceptance gate: cargo test + npm test summaryState

      GLOSS-NP34-S1-016 (S1): Import capability matrix
        - No pre-scan type report before folder import; user doesn't know
          what will be imported.
        - Acceptance gate: cargo test + npm test importCapabilityMatrix

      GLOSS-NP34-S2-022 (S2): GLib file metadata warnings
        - GLib-GIO-CRITICAL at runtime; g_file_info_get_size called
          without requesting standard::size.
        - Acceptance gate: manual Linux file-picker smoke has no GLib
          critical warnings

      GLOSS-NP34-S2-024 (S2): User message persists before
                             retrieval/provider success
        - send_message persists user message before provider/retrieval
          errors; failed setup leaves orphan user turns without
          assistant/error receipt.
        - Acceptance gate: cargo test chat_turn_records_* 

      GLOSS-NP34-S2-025 (S2): Assistant trace/latest mutable projection
        - Chat attempt traces include mutable latest.json; no guarantee
          latest is not treated as source truth.
        - Acceptance gate: cargo test chat_attempt_latest_is_projection_only

      GLOSS-NP34-S2-027 (S2): FTS query limits and code search
        - FTS sanitizer not proven for code symbols, underscores, hyphens,
          Rust paths, cargo terms.
        - Acceptance gate: cargo test fts_code_terms_paths_symbols

      GLOSS-NP34-S2-028 (S2): No import batch cancellation UI
        - Background folder import has no cancel/stop affordance in UI.
        - Acceptance gate: cargo test + npm test importCancel

      GLOSS-NP34-S2-031 (S2): No no-source strict answer guard
        - When no notebook sources selected, system prompt asks model to
          tell user no sources selected but backend still calls provider;
          compliance depends on model.
        - Acceptance gate: cargo test no_source_mode_blocks_grounded_claims

      GLOSS-NP34-S2-032 (S2): Prompt injection tests missing runtime smoke
        - Hostile source fixture and smoke not implemented.
        - Acceptance gate: python3 scripts/gloss_hostile_source_smoke.py

      GLOSS-NP34-S2-033 (S2): Source manifests may bias unsupported claims
        - Manifest summaries can become pseudo-evidence when retrieval fails.
        - Acceptance gate: cargo test manifest_only_context_discloses_limitation

      GLOSS-NP34-S2-034 (S2): Summary model provider restrictions too narrow
        - Background summary requires Ollama provider even though app supports
          OpenAI/Anthropic chat providers.
        - Acceptance gate: npm test backgroundModelPolicy

      GLOSS-NP34-S2-040 (S2): Media ingestion partial/stub
        - Vision/video queue paths exist but file extraction for audio/video/
          PDF/docx not implemented; BINARY_EXTENSIONS excludes many doc types.
        - Acceptance gate: python3 scripts/gloss_import_capability_gate.py

      GLOSS-NP34-S2-041 (S2): Secret warning not classified
        - Package warning about secret-content-named-secret-assignment at
          ollama.rs line 313 is not classified.
        - Acceptance gate: python3 scripts/classify_package_warnings.py

      GLOSS-NP34-S3-050 (S3): Frontend test coverage thin
        - Only 12 contract tests; no component/store tests for ingestion,
          evidence drawer, status bar, settings, cancellation.
        - Acceptance gate: npm test includes sourceStore, notebookStore,
          evidenceDrawer, statusBar tests

      GLOSS-NP34-MISSING-080 (S2): Current docs claim boundary
        - README still points to stale P33 and broad spec.
        - Acceptance gate: docs claim gate

 3.2  OPEN_NEXT_PASS:

      GLOSS-NP34-S2-044 (S2): No bitemporal/supersession for source updates
        - Sources are mutable rows; source lifecycle lacks append-plus-
          supersession and audit history.
        - Acceptance gate: cargo test source_delete_writes_lifecycle_receipt

      GLOSS-NP34-S2-047 (S2): Source summaries can become stale
        - Summary fields exist but no digest/backpointer tying summary to
          source content digest/model/prompt.
        - Acceptance gate: cargo test summary_invalidates_on_source_digest_change

      GLOSS-NP34-S3-052 (S3): Import speed / sequential ingestion
        - One source at a time with 50ms pause; no parallel CPU chunking.
        - Acceptance gate: manual import smoke under target time

      GLOSS-NP34-S3-053 (S3): Settings dialog size / complexity
        - SettingsDialog/index.tsx is 1027 lines; monolithic.
        - Acceptance gate: component-level tests import each settings subpanel

      GLOSS-NP34-S3-054 (S3): ChatPanel size / complexity
        - ChatPanel.tsx is 638 lines (now 775 after edits).
        - Acceptance gate: tests for EvidenceDrawer

      GLOSS-NP34-S3-055 (S3): Sources command module size
        - sources/mod.rs is 4904 lines, chat/mod.rs is 2529 lines.
        - Acceptance gate: no command module over agreed threshold

      GLOSS-NP34-MISSING-071 (S2): Search across notebook UI proof
        - Search bar exists but cross-notebook/source search quality
          not proven.
        - Acceptance gate: npm test notebookSearch + cargo test search_commands

      GLOSS-NP34-MISSING-072 (S2): Local model capability registry
        - Model capabilities not stored with exact roles (chat/vision/embed).
        - Acceptance gate: cargo test model_capability_registry_roles

      GLOSS-NP34-MISSING-073 (S2): Provider health/status receipts
        - No per-provider health receipts with latency/endpoint/model list.
        - Acceptance gate: cargo test provider_health_receipts

      GLOSS-NP34-MISSING-076 (S2): Accessibility audit
        - Keyboard navigation, focus traps, ARIA labels not proven.
        - Acceptance gate: npm test a11yStatic + manual keyboard smoke

 3.3  PREVIOUSLY VERIFIED BUT OPEN (PARTIAL):

      GLOSS-NP34-S2-036 (S2): Offline vendor package too large for Codex
        - Package includes 27,997 src-tauri files, 531MB included.
        - Acceptance gate: python3 z.py --profile gloss-codex-source

      GLOSS-NP34-S2-037 (S2): Cargo/Rust checks not reproducible here
        - Package relies on included receipts; must include host Rust
          command logs and fresh-unzip replay logs.
        - Acceptance gate: cargo fmt/test/clippy clean

      GLOSS-NP34-S2-038 (S2): Missing feature matrix / deferred truth
        - SPEC lists Studio, reports, flashcards, quizzes; current matrix
          must stay current.
        - Acceptance gate: python3 scripts/gloss_feature_matrix_gate.py

=================================================================
SECTION 4: CODE QUALITY — STRUCTURAL DEFECTS
=================================================================

 4.1  Excessive .unwrap() usage in production code
      Count: 394 occurrences across src-tauri/src/ (excluding vendor)
      Top offenders:
        - db/notebook_db/mod.rs: 64 unwraps
        - db/portable.rs: 45 unwraps
        - commands/sources/mod.rs: 41 unwraps
        - db/doctor.rs: 34 unwraps
        - db/app_db.rs: 28 unwraps
        - state.rs: 27 unwraps
        - db/migrations.rs: 24 unwraps
        - ingestion/extract.rs: 18 unwraps
        - features.rs: 18 unwraps
        - provider_config_store.rs: 18 unwraps
        - ingestion/hybrid_search.rs: 16 unwraps
        - commands/notebooks.rs: 11 unwraps

      Many of these are on Mutex.lock() or .get_setting() which should
      never fail in production paths, but a poisoned mutex would crash
      the entire Tauri process without any recovery or diagnostic.

      Risk: Poisoned mutex in any command kills the app. No graceful
            degradation path for lock failures.
      Fix: Add lock recovery or structured error propagation. At minimum,
           document which unwraps are truly infallible and why.

 4.2  Production .expect() calls: 7
      - lib.rs: 4 (likely in setup)
      - memory/backend.rs: 2
      - providers/ollama.rs: 1

      These will panic and kill the process on failure.

 4.3  TypeScript 'any' usage: 1
      - src/stores/chatStore.ts: type on parseAssistantPayload parameter
        (acceptable — it's a parsing boundary)

 4.4  Monolithic modules (exceeding maintainability thresholds):
      - src-tauri/src/commands/sources/mod.rs: 4,904 lines
      - src-tauri/src/commands/chat/mod.rs: 2,529 lines
      - src/components/chat/ChatPanel.tsx: 775 lines
      - src/components/settings/SettingsDialog/index.tsx: ~1,027 lines

      AGENTS.md does not set a hard limit but the ISSUE_LEDGER flags
      these at S3. Large modules resist auditing and increase defect
      density.

 4.5  Duplicated vision-detection logic:
      Both src-tauri/src/commands/sources/mod.rs (Rust) and
      src/components/settings/SettingsDialog/index.tsx (TypeScript)
      maintain independent lists of vision-capable model name prefixes.
      The frontend list and backend list can drift.

      Fix: Single source of truth — either derive from backend via
           Tauri command, or store model capabilities in the DB.

 4.6  localStorage as notebook switch guard:
      14 calls to localStorage.getItem(ACTIVE_NB_KEY) spread across
      chatStore.ts, sourceStore.ts, and notebookStore.ts. This is a
      synchronous cross-store coordination mechanism using the DOM
      storage API. If localStorage is cleared or corrupted mid-session,
      store state and backend state will diverge.

      Fix: Use a shared Zustand store or context variable for the active
           notebook ID, synchronized from backend on startup.

=================================================================
SECTION 5: ARCHITECTURAL CONCERNS
=================================================================

 5.1  Provider hard-dependency on Ollama
      Background summaries (ingestion/summarize.rs) hardcode Ollama as
      the provider. The app supports OpenAI and Anthropic for chat but
      summaries cannot use them. This is flagged in ISSUE_LEDGER S2-034.

 5.2  No provider-level retry or circuit-breaking
      All provider errors bubble immediately to the chat stream. There is
      no retry for transient network errors, rate limits, or temporary
      unavailability. Ollama returning 503 or timeout kills the entire
      chat turn.

 5.3  Single-flight gate is coarse
      llm_gate (Semaphore in state.rs) is a binary permit — only one LLM
      call at a time. This prevents concurrent chat sessions but also
      prevents concurrent summary generation. There is no priority or
      queue for different request types (chat vs background summary).

 5.4  No cancellation token for in-flight provider calls
      stop_chat increments active_epoch which is checked between stream
      poll intervals (250ms). There is no abort handle for the HTTP
      request itself. On slow connections, cancellation can take up to
      250ms + network timeout to propagate.

 5.5  Chat message persistence BEFORE provider success
      send_message persists the user message (line 511 of chat/mod.rs)
      before provider configuration is resolved, before retrieval, and
      before the provider call. If any of these fail, the conversation
      has an orphaned user message with no assistant response. This is
      a data integrity issue.

 5.6  No durability for streaming/cancellation/timeout states
      When the frontend stops a stream (stopStreaming), the partial
      content is shown transiently in the UI but is NOT persisted to the
      database. Reloading the page loses the partial content. The backend
      has no partial persistence path.

=================================================================
SECTION 6: AGENTS.MD RULE COMPLIANCE AUDIT
=================================================================

 6.1  "Do not hide provider errors behind a spinner"
      Status: COMPLIANT
      ChatPanel correctly shows streamingError in a red alert box below
      the chat area. The spinner only shows when isStreaming AND content
      is empty (loading state), not on error.

 6.2  "Do not block no-retrieval chat because source list loading/
      partial/error"
      Status: COMPLIANT
      sourceStore's buildSourceScope returns { kind: 'none' } when
      sourceListStatus is loading/partial/error/idle. Chat still sends
      with kind: 'none' scope. Verified by validate_source_send_gate.py.

 6.3  "Do not emit chat:done before assistant persistence succeeds
      unless explicit partial/cancel artifact is emitted"
      Status: COMPLIANT (fixed in SUPERpass)
      ChatTerminalGuard ensures exactly-one terminal event. The done
      emission in chat/mod.rs is now the LAST operation after message
      persistence, evidence recording, and receipts.

 6.4  "Do not silently allow LAN or cloud endpoints; provider authority
      must be explicit"
      Status: COMPLIANT
      validate_provider_base_url enforces loopback-only for Ollama/
      LlamaCpp (with LAN opt-in), and exact host match for OpenAI/
      Anthropic. Verified by validate_provider_lan_policy.py.

 6.5  "Do not claim semantic-memory/TurboQuant/dense indexing proof
      from dependencies alone"
      Status: COMPLIANT
      features.rs gate chain is explicit: turbo_quant requires
      semantic_memory_preview which requires experimental master.
      All three are default-disabled. Build features are separate
      from runtime settings.

 6.6  "Do not add compatibility shims or shadow truth stores"
      Status: COMPLIANT
      No evidence of duplicate backend implementations or shadow stores.
      The ChatTerminalGuard replaced duplicate emit paths.

 6.7  "Do not update release docs by hand if gate JSON says failed"
      Status: CANNOT FULLY VALIDATE
      The gate results are mixed — some pass, some fail. We cannot verify
      whether docs were hand-edited to claim passing status without
      checking the full history of every doc edit. The gloss_p36_static_gate
      warning suggests Run ID visibility is acceptable "only if
      docs/codex-runs/CURRENT_RUN.md owns it" — this is borderline.

=================================================================
SECTION 7: SECURITY SURFACE AUDIT
=================================================================

 7.1  Tauri capability over-grant: dialog:allow-save
      (See 1.5) The save dialog capability is granted but unused.

 7.2  Secret management
      - SecretStore (provider_config_store.rs) uses AES-GCM encryption
        with a platform keyring. This is correct for local-first.
      - API keys are redacted from error messages (sanitize_provider_error_body).
      - ollama.rs line 313: content of test fixture json!({"error": "model
        not found"}) is NOT an actual secret. The secret warning from the
        Codex scanner is a false positive. This is ISSUE_LEDGER S2-041.

 7.3  URL sanitization
      validate_provider_base_url rejects credentials in URL, query strings,
      fragments, and non-http/https schemes. RFC 1918 detection handles
      both IPv4 and IPv6 private ranges. This is solid.

 7.4  Source scope injection
      source_scope.rs test explicit_scope_treats_sql_fts_payload_as_data
      confirms SQL injection resistance in explicit scope resolution.

 7.5  No input length limits on chat messages
      send_message accepts unbounded query length. While this goes to an
      LLM and is not a traditional injection vector, it can consume
      excessive memory/tokens without validation.

=================================================================
SECTION 8: TEST COVERAGE ASSESSMENT
=================================================================

 8.1  Rust tests: 146 passed, 0 failed (with semantic-memory-turbo-quant)
      Coverage areas:
      - Provider LAN policy: 9 tests (thorough)
      - Ollama chat + streaming: 4 tests (body correctness, error, normal)
      - Feature flags: 5+ tests (gate chains)
      - Source scope: 7 tests (dedup, invalid, SQL injection)
      - Chat done-frame: 1 test (regression)
      Total: ~146 tests across workspace

 8.2  Frontend contract tests: 12 passed
      - Chat message evidence envelope
      - Note citations
      - No citation union types
      - Unknown evidence defaults
      - System prompt excludes quoted passages
      - User turn wraps source data
      - Notebook portability UI (import + export)
      - DB doctor UI
      - Failed import quarantine UI
      - YouTube transcript UI
      - Studio UI
      These are STRUCTURAL contract tests, not behavioral. They verify
      type shapes and import/export patterns, not user interactions.

 8.3  MISSING test categories:
      - No React component unit tests (render/mount/click)
      - No store action tests (state transitions)
      - No integration tests for chat flow (backend -> frontend events)
      - No retrieval decision tests (BM25 vs semantic)
      - No citation filtering tests
      - No performance/load tests
      - No accessibility tests beyond static strings

=================================================================
SECTION 9: DEPENDENCY / BUILD CONCERNS
=================================================================

 9.1  External path dependencies
      Cargo.toml depends on:
      - llm-pipeline = { path = "../../Libraries/llm-pipeline" }
      - tauri-queue = { path = "../../Libraries/tauri-queue" }
      - semantic-memory = { path = "../../Libraries/semantic-memory",
        optional = true }
      These are sibling-directory dependencies — not published crates.
      Anyone cloning Gloss must also clone Libraries/ at the correct
      relative path.

 9.2  fastembed downloads models at runtime
      ONNX embedding models are downloaded on first use. This requires
      network access and the download consent flag (fastembed_download_consent).
      On air-gapped machines, embeddings will silently fail.

 9.3  usearch (C++ native library)
      The usearch crate compiles C++ code. This requires a C++ compiler.
      The Cargo.toml does not document this build requirement.

 9.4  Tauri plugin dependencies
      dialog, fs, shell, clipboard-manager, opener — each adds Tauri
      permission surface. The dialog:allow-save over-grant (see 1.5)
      is the only identified issue.

=================================================================
SECTION 10: DOCUMENTATION / TRUTH CONSISTENCY
=================================================================

10.1  README.md points to stale P33 scripts
      ISSUE_LEDGER NP34-S0-001 was marked closed but gloss_p36_static_gate
      produces a warning that Run ID is not visible in AGENTS.md/README.md.
      The README validation block still references historical commands.

10.2  HOSTILE_AUDITOR_HANDOFF.md lists 7 release blockers
      - Package scope gate failure (see 1.1)
      - Fresh-unzip replay failure (see 1.3)
      - Missing live Ollama Gloss-path smoke script
      - Missing live desktop smoke driver
      - Incomplete durable partial persistence
      - Incomplete backend cancellation
      - Missing gloss_public_claim_gate.py
      All 7 remain unresolved as of this audit.

10.3  SPEC-gloss.md vs implementation gap
      SPEC at 73,941 bytes describes a much broader product than the
      current implementation. The CURRENT_FEATURE_MATRIX.md is the
      intended bridge but may need updates.

10.4  Multiple control pack directories
      codex-control-pack and codex_control_pack both exist as top-level
      directories. ISSUE_LEDGER S2-035 was marked closed but both dirs
      are still present at the top level.

=================================================================
SECTION 11: KNOWN BUT NOT YET GATED DEFECTS
=================================================================

11.1  Frontend stopStreaming discards partial content
      When user clicks Stop, the streamingContent is cleared and an error
      message replaces it. The partial output is DISPLAYED transiently
      but not persisted to DB. On refresh, it's gone.

11.2  ChatPanel uses crypto.randomUUID() without fallback
      In environments where crypto.randomUUID is unavailable (older
      browsers, non-HTTPS), message IDs would fail to generate. Tauri's
      webview should support it, but no polyfill exists.

11.3  No token-count warning for oversized requests
      The prompt budget receipt exists and is emitted, but there is no
      user-facing warning BEFORE sending. Users can submit prompts that
      will be silently truncated.

11.4  Model name display uses raw IDs
      ChatPanel displays "qwen3.5:4b" as the active model. There is no
      display_name lookup — what the user sees is the raw backend ID.

11.5  Evidence drawer UX can be overwhelming
      Even after the UUID flood fix (S0-007), the evidence drawer is dense.
      Collapsible sections and copy-JSON buttons help, but there is no
      human-readable summary mode for non-technical users.

=================================================================
SECTION 12: RECOMMENDED TRIAGE ORDER
=================================================================

Immediate (blocks any release claim):
  1. Fix package scope gate (1.1) — slimmed source package
  2. Fix audit_tauri_security.py (1.5) — remove unused capability
  3. Commit dirty working tree (2.1)

High priority (S0/S1 open issues):
  4. Fix installer smoke (1.4)
  5. Fix fresh-unzip replay (1.3)
  6. Resolve open S1 issues: S1-014 (summary state), S1-016 (import matrix)

Medium (S2 open issues, structural):
  7. Address code quality: reduce .unwrap() count, especially in
     db/notebook_db, db/portable, commands/sources
  8. Implement partial persistence for stopped/cancelled streams
  9. Add backend cancellation token
 10. Add provider retry/backoff (S2-048)

Long tail (S2/S3, deferred):
 11. Split monoliths (S3-053, S3-054, S3-055)
 12. Add component tests (S3-050)
 13. Add model capability registry (MISSING-072)
 14. Add provider health receipts (MISSING-073)
 15. Accessibility audit (MISSING-076)
 16. Import performance (S3-052)

=================================================================
SECTION 13: HOSTILE AUDITOR HANDOFF
=================================================================

This audit was conducted on 2026-05-29 against a DIRTY working tree at
commit dec9ba266a51da025af62665736418ed6ddc3f18.

Tools used:
  - All 38 gloss_*.py validation gates
  - All 15 scripts/audit_*.py, check_*.py, assert_*.py scripts
  - Manual code review of all major source files (18 files, ~10,778 lines)
  - Pattern search for code quality markers (.unwrap, .expect, any, TODO)
  - Cross-reference against AGENTS.md rules
  - ISSUE_LEDGER.csv completeness check

Key findings: 5 failing gates, 24 open issues, 394 production unwraps,
7 release blockers from prior handoff still unresolved.

Next auditor should:
  1. Start with the RECOMMENDED TRIAGE ORDER (Section 12)
  2. Verify any fixes by re-running the full gate suite:
     python3 validation/gloss_package_scope_gate.py --repo .
     python3 validation/gloss_next_release_gate.py --repo .
     python3 validation/gloss_fresh_unzip_replay_gate.py --repo .
     python3 validation/gloss_installer_smoke_gate.py --repo .
     python3 scripts/audit_tauri_security.py .
  3. Do not close any issue without acceptance gate evidence
  4. Commit all changes before claiming any pass

Receipts or it did not happen.
