# Gloss P36 / Phase 129 — Live Certification and Chat Proof Implementation Plan

> **For Hermes:** Use `subagent-driven-development` skill to implement this plan task-by-task. Do not mark the phase complete from static gates alone; this phase exists to convert Gloss from automated RC green to live-release proven.

**Goal:** Prove and, where necessary, harden the next phase of Gloss: real-provider chat, real retrieval, desktop/AppImage launch, import/studio/export flows, and performance certification.

**Architecture:** Keep current P35 feature implementation intact. Add missing smoke harnesses, deterministic demo fixtures, runtime receipts, and small bug fixes only when a live test exposes a root cause. Separate claims into `candidate`, `certified`, and `not exercised` so receipts cannot overstate readiness.

**Tech Stack:** Tauri v2, React/TypeScript, Rust backend, SQLite/HNSW, Ollama/OpenAI/Anthropic/llama.cpp provider registry, Python validation gates, AppImage packaging.

---

## 0. Evidence-backed current state — 2026-06-07

### Repo

- Path: `/home/sikmindz/Coding/Gloss`
- Branch: `recovery/audit-20260529`
- HEAD at inspection: `d17a5f2 chore: commit final audit delta`
- Target repo status at inspection: dirty; `23` git status entries:
  - 21 modified tracked files
  - 2 untracked docs: `GLOSS_FINALIZATION_PLAN_20260601.md`, `HOSTILE_AUDIT_REVERIFICATION_20260601.md`

### Commands freshly run and passing

```bash
cd /home/sikmindz/Coding/Gloss

python3 validation/gloss_release_candidate_gate.py --repo .
# PASS: { "ok": true, "failed": [] }

python3 validation/validate_source_send_gate.py .
# PASS: sourceListStatus is not used as hard send/disabled gate

python3 validation/validate_frontend_event_routing.py .
# PASS: chat lifecycle events are not pre-filtered by activeNotebookId

python3 validation/validate_chat_terminal_contract.py .
# PASS: no raw terminal-less return detected in spawned chat task

python3 validation/validate_provider_lan_policy.py .
# PASS: explicit LAN provider policy markers present

python3 validation/validate_release_receipt_consistency.py .
# PASS: release receipt consistency gate

npm test
# PASS: 12 frontend contract checks

cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
# PASS: 2 passed, 0 failed

cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
# PASS: 9 passed, 0 failed

python3 validation/gloss_live_receipt_gate.py --repo . --run-id GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526
# PASS: { "ok": true, "failures": [], "warnings": [] }

python3 validation/gloss_generation_receipt_gate.py --repo .
# PASS: { "ok": true, "failures": [], "warnings": [] }

python3 validation/gloss_retrieval_decision_gate.py --repo .
# PASS: { "ok": true, "failures": [], "warnings": [] }
```

### Command mistake observed and corrected

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests providers::tests
# FAIL: cargo only accepts one TESTNAME argument; unexpected argument `providers::tests`.
```

Correction: run chat and provider filters as separate commands.

### Known artifact/path failure

`FINAL_RECEIPT.json` claims an AppImage at:

```text
target/release/bundle/appimage/Gloss_1.0.0_amd64.AppImage
```

Fresh filesystem check found:

```bash
test -f target/release/bundle/appimage/Gloss_1.0.0_amd64.AppImage
# missing

search_files("*.AppImage", target="files", path="/home/sikmindz/Coding/Gloss/target/release/bundle")
# Path not found: /home/sikmindz/Coding/Gloss/target/release/bundle
```

Therefore: **AppImage was previously recorded but is not currently present. Rebuild before claiming package availability.**

### Candidate vs certified line

- **Candidate:** automated gates/unit/contract tests pass.
- **Certified:** a fresh artifact exists, launches in a headed session, real provider chat succeeds, retrieval succeeds, studio widgets render from generated outputs, export/import roundtrip succeeds, and performance receipts are written.
- **Non-claims:** no live desktop smoke, no live performance certification, no currently-present AppImage until rebuilt.

---

## 1. Phase priorities

### P0 — Certification blockers

1. Clean/reconcile repo state enough to isolate this phase.
2. Rebuild AppImage and prove the artifact exists.
3. Run headed desktop smoke.
4. Prove chat live with real provider in both no-retrieval and retrieval modes.
5. Prove persistence/reload of assistant messages.
6. Prove export/import roundtrip.
7. Produce performance certificate.
8. Regenerate final P36 receipt without overstating unsupported claims.

### P1 — Product hardening from smoke findings

1. Fix any live chat issue by root cause only.
2. Add regression gates for any bug found.
3. Tighten chat/store behavior only where live evidence shows a defect.
4. Patch packaging script if rebuild exposes fallback bugs.

### P2 — Polish/future wiring

1. Improve demo fixture UX.
2. Add optional scripted screenshot/video capture.
3. Defer P36+ features not needed for certification: audio overviews, slide decks, infographics, podcasts, query rewriting, interactive timeline/data table.

---

## 2. Implementation tasks

### Task 1: Create a P36 receipt directory

**Objective:** Create a clean receipt home for this phase so outputs do not mix with the stale P35 run.

**Files:**
- Create directory: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/`
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/README.md`

**Step 1: Create README**

```markdown
# Gloss P36 Live Certification Receipts

Run ID: GLOSS_P36_LIVE_CERTIFICATION_20260607
Purpose: certify live desktop/chat/retrieval/package/performance behavior beyond static gates.

Claim levels:
- candidate: static/contract gate proof only
- certified: live run produced receipt in this directory
- not_exercised: honest blocker with reason
```

**Step 2: Verify**

Run:

```bash
test -f docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/README.md
```

Expected: exit `0`.

**Step 3: Commit**

```bash
git add PHASES/PHASE_129_LIVE_CERTIFICATION_AND_CHAT_PROOF_PLAN.md docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/README.md
git commit -m "docs: add P36 live certification plan"
```

If existing dirty files are intentionally uncommitted, do not commit them in this task.

---

### Task 2: Snapshot preflight state

**Objective:** Record exact repo/tool/artifact state before live certification.

**Files:**
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/P00_PREFLIGHT.md`
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/git-status-preflight.txt`

**Step 1: Capture commands**

Run:

```bash
cd /home/sikmindz/Coding/Gloss
{
  echo '# git status --short'
  git status --short
  echo
  echo '# branch'
  git branch --show-current
  echo
  echo '# head'
  git log -1 --oneline --decorate
  echo
  echo '# node/npm/rust/python'
  node --version || true
  npm --version || true
  rustc --version || true
  cargo --version || true
  python3 --version || true
  echo
  echo '# display env'
  env | grep -E '^(DISPLAY|WAYLAND_DISPLAY|XDG_SESSION_TYPE)=' || true
} > docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/git-status-preflight.txt
```

**Step 2: Write preflight summary**

`P00_PREFLIGHT.md` must include:

- repo path
- branch
- HEAD
- dirty file count
- whether DISPLAY/Wayland is available
- whether AppImage exists
- whether provider endpoint is configured

**Step 3: Verify**

Run:

```bash
grep -E 'branch|head|display|AppImage|provider' docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/P00_PREFLIGHT.md
```

Expected: all terms present.

---

### Task 3: Rebuild AppImage from current source

**Objective:** Produce a fresh AppImage artifact from the current tree.

**Files:**
- May modify: `scripts/build-appimage.sh` only if rebuild fails from a script bug
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/appimage-build.log`
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/appimage-artifact.json`

**Step 1: Run build**

```bash
bash scripts/build-appimage.sh 2>&1 | tee docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/appimage-build.log
```

Expected: exit `0`; AppImage produced under `target/release/bundle/appimage/`.

**Step 2: Record artifact metadata**

```bash
python3 - <<'PY'
import hashlib, json, pathlib, time
root = pathlib.Path('target/release/bundle/appimage')
imgs = sorted(root.glob('*.AppImage'))
if not imgs:
    raise SystemExit('no AppImage found')
p = imgs[-1]
h = hashlib.sha256(p.read_bytes()).hexdigest()
print(json.dumps({
    'path': str(p),
    'size_bytes': p.stat().st_size,
    'sha256': h,
    'mtime': p.stat().st_mtime,
    'recorded_utc': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
}, indent=2))
PY > docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/appimage-artifact.json
```

**Step 3: Verify ELF/appimage identity**

```bash
file $(python3 - <<'PY'
import json
print(json.load(open('docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/appimage-artifact.json'))['path'])
PY
)
```

Expected: ELF executable / AppImage-style artifact.

**Step 4: If build fails**

Do not patch blindly. Read the exact failure. If the known fallback bug appears (`tmp_appdir: unbound variable`), patch only the variable lifetime/initialization in `scripts/build-appimage.sh`, rerun, and add a regression comment in the build log.

---

### Task 4: Add a deterministic live-smoke fixture pack

**Objective:** Ensure live tests use small, known content and produce predictable retrieval expectations.

**Files:**
- Create: `fixtures/live-smoke/gloss-smoke-source.md`
- Create: `fixtures/live-smoke/gloss-smoke-source-2.md`
- Create: `fixtures/live-smoke/expected.json`

**Step 1: Create fixture content**

`gloss-smoke-source.md`:

```markdown
# Gloss Smoke Source Alpha

The verification phrase is: cedar lantern protocol.

Gloss should cite this source when asked about the verification phrase.
```

`gloss-smoke-source-2.md`:

```markdown
# Gloss Smoke Source Beta

The second verification phrase is: violet engine receipt.

Gloss should cite this source when asked about the second verification phrase.
```

`expected.json`:

```json
{
  "queries": [
    {
      "mode": "retrieval",
      "prompt": "What is the verification phrase from the Alpha smoke source? Cite the source.",
      "must_include_any": ["cedar lantern protocol", "cedar", "lantern"],
      "must_have_citation": true
    },
    {
      "mode": "no_retrieval",
      "prompt": "Say exactly: no retrieval smoke ok",
      "must_include_any": ["no retrieval smoke ok"],
      "must_have_citation": false
    }
  ]
}
```

**Step 2: Verify fixtures**

```bash
python3 -m json.tool fixtures/live-smoke/expected.json >/dev/null
```

Expected: exit `0`.

---

### Task 5: Add a manual live desktop smoke checklist

**Objective:** Create the human-executable checklist for the headed app session.

**Files:**
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/LIVE_DESKTOP_SMOKE_CHECKLIST.md`

**Checklist must include:**

1. Launch app from dev mode and from AppImage.
2. Create notebook `Gloss P36 Smoke`.
3. Select provider/model.
4. Run no-retrieval chat.
5. Import both fixture markdown sources.
6. Wait for ready/indexed state.
7. Run retrieval chat.
8. Stop a stream mid-generation and verify terminal state clears.
9. Regenerate last answer.
10. Continue from partial answer if partial exists.
11. Switch notebooks during or after streaming; verify no stuck spinner.
12. Reload app; verify assistant persisted.
13. Generate flashcards, quiz, and mind map from fixture source.
14. Export notebook.
15. Import into fresh notebook.
16. Repeat one retrieval query in imported notebook.
17. Record screenshots/logs/receipt paths.

**Verification:** checklist file exists and has all 17 numbered items.

---

### Task 6: Implement a live-smoke receipt schema

**Objective:** Store manual/automated smoke results in machine-readable JSON.

**Files:**
- Create: `validation/schemas/gloss_p36_live_smoke_receipt.schema.json`
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.template.json`

**Schema fields:**

```json
{
  "run_id": "GLOSS_P36_LIVE_CERTIFICATION_20260607",
  "recorded_utc": "string",
  "app_launch": {
    "dev_mode": "pass|fail|not_exercised",
    "appimage": "pass|fail|not_exercised",
    "appimage_path": "string"
  },
  "provider": {
    "provider_id": "string",
    "model": "string",
    "health": "pass|fail|not_exercised"
  },
  "chat": {
    "no_retrieval": "pass|fail|not_exercised",
    "retrieval": "pass|fail|not_exercised",
    "stop": "pass|fail|not_exercised",
    "regenerate": "pass|fail|not_exercised",
    "continue_partial": "pass|fail|not_exercised",
    "notebook_switch_terminal_clear": "pass|fail|not_exercised",
    "persist_after_reload": "pass|fail|not_exercised"
  },
  "studio": {
    "flashcards": "pass|fail|not_exercised",
    "quiz": "pass|fail|not_exercised",
    "mind_map": "pass|fail|not_exercised"
  },
  "portability": {
    "export": "pass|fail|not_exercised",
    "import": "pass|fail|not_exercised",
    "post_import_retrieval": "pass|fail|not_exercised"
  },
  "evidence": {
    "screenshots": [],
    "logs": [],
    "db_paths": [],
    "receipt_paths": []
  },
  "failures": []
}
```

**Step 2: Verify template JSON**

```bash
python3 -m json.tool docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.template.json >/dev/null
```

Expected: valid JSON.

---

### Task 7: Add a validator for the P36 live-smoke receipt

**Objective:** Prevent final certification when live smoke is missing or failed.

**Files:**
- Create: `validation/gloss_p36_live_smoke_gate.py`
- Test manually with template receipt

**Behavior:**

- Accept `--repo` and `--receipt`.
- Load JSON.
- Fail if required P0 fields are `fail`.
- Fail if required P0 fields are `not_exercised` unless `--allow-not-exercised` is passed.
- Always print JSON: `{ "ok": bool, "failures": [], "warnings": [] }`.

**Required P0 fields without allow:**

- `app_launch.dev_mode`
- `app_launch.appimage`
- `provider.health`
- all `chat.*`
- `portability.export`
- `portability.import`
- `portability.post_import_retrieval`

**Step 1: Write failing template check**

Run against the empty template:

```bash
python3 validation/gloss_p36_live_smoke_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.template.json
```

Expected: fail with missing/not_exercised fields.

**Step 2: Write implementation**

Use plain stdlib Python only: `argparse`, `json`, `pathlib`.

**Step 3: Verify allow mode**

```bash
python3 validation/gloss_p36_live_smoke_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.template.json --allow-not-exercised
```

Expected: `ok: true` or warnings only.

---

### Task 8: Execute headed desktop smoke

**Objective:** Produce the live proof that current automated gates cannot provide.

**Files:**
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.json`
- Create optional screenshots/logs under: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/screenshots/`

**Precondition:** A headed environment exists: `DISPLAY` or `WAYLAND_DISPLAY` set.

**Step 1: Launch dev mode**

```bash
npm run tauri dev
```

Expected: desktop app opens.

**Step 2: Complete checklist**

Follow `LIVE_DESKTOP_SMOKE_CHECKLIST.md` exactly.

**Step 3: Record receipt**

Copy template to `live-smoke.receipt.json`; mark each item pass/fail/not_exercised; attach evidence paths.

**Step 4: Run gate**

```bash
python3 validation/gloss_p36_live_smoke_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.json
```

Expected for certification: `ok: true`.

If no headed environment is available, run with `--allow-not-exercised` only for candidate status, not certification.

---

### Task 9: Add performance certification script

**Objective:** Measure and record latency/memory claims instead of hand-waving them.

**Files:**
- Create: `scripts/gloss_p36_perf_probe.py`
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/perf.receipt.json`

**Measurements:**

- app launch time if scriptable
- provider health latency
- no-retrieval first token latency
- retrieval query total time
- retrieval merge/RRF latency if exposed in backend receipts
- memory RSS baseline and after 30 minutes idle, or shorter `--quick` mode marked as non-certifying

**Script interface:**

```bash
python3 scripts/gloss_p36_perf_probe.py --repo . --run-id GLOSS_P36_LIVE_CERTIFICATION_20260607 --quick
```

**Receipt status rules:**

- `certified: true` only when full timing path runs.
- `certified: false` in `--quick` mode.
- Include exact provider/model and hardware notes.

---

### Task 10: Add performance receipt validator

**Objective:** Block M-3 certification unless performance receipt meets thresholds or honestly marks not certified.

**Files:**
- Create: `validation/gloss_p36_performance_gate.py`

**Default thresholds:**

- retrieval merge latency: `< 200ms` when field is present
- first token latency: `< 2s` for local model certification, or mark as environment-specific if model too slow
- memory growth: no unbounded growth over full idle window

**Commands:**

```bash
python3 validation/gloss_p36_performance_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/perf.receipt.json
```

Expected: `ok: true` only for certified receipt.

---

### Task 11: Fix live chat bugs only with regression tests

**Objective:** If chat fails during live smoke, fix root cause and preserve coverage.

**Files likely involved:**
- `src/stores/chatStore.ts`
- `src/components/chat/ChatPanel.tsx`
- `src-tauri/src/commands/chat/mod.rs`
- `src-tauri/src/commands/chat/streaming.rs`
- `src-tauri/src/commands/chat/emit.rs`
- `validation/validate_chat_terminal_contract.py`
- `validation/validate_frontend_event_routing.py`

**Rules:**

1. Do not patch before reproducing.
2. Write the failure into `live-smoke.receipt.json`.
3. Add or extend a regression gate/test before fixing.
4. Fix one root cause at a time.
5. Rerun the specific test, then the gate suite.

**Minimum verification after any chat fix:**

```bash
python3 validation/validate_source_send_gate.py .
python3 validation/validate_frontend_event_routing.py .
python3 validation/validate_chat_terminal_contract.py .
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
npm test
```

Expected: all pass.

---

### Task 12: Verify P35 intended-feature boundaries

**Objective:** Make sure final P36 docs distinguish shipped features from deferred features.

**Files:**
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/FEATURE_SCOPE_MATRIX.md`

**Include sections:**

- Certified live in P36
- Implemented but static/contract only
- Candidate but not certified due environment
- Deferred to P36+ / P37

**Deferred list must include:**

- audio overviews/TTS
- slide deck generation + viewer
- infographic generation + PNG export
- podcast outputs
- multi-angle query rewriting
- timeline interactive view
- data table interactive view

---

### Task 13: Regenerate final P36 receipt

**Objective:** Produce one final receipt that makes the release decision obvious.

**Files:**
- Create: `docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/FINAL_P36_RECEIPT.json`

**Required fields:**

- run id
- git commit
- dirty status summary
- appimage artifact metadata
- static gate summary
- live smoke summary
- performance summary
- feature scope summary
- release decision: `certified`, `candidate_only`, or `blocked`
- blockers with exact reasons

**Validation:**

```bash
python3 -m json.tool docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/FINAL_P36_RECEIPT.json >/dev/null
python3 validation/gloss_p36_live_smoke_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.json
python3 validation/gloss_p36_performance_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/perf.receipt.json
```

---

### Task 14: Final full validation sweep

**Objective:** Ensure P36 changes did not regress P35 gates.

**Commands:**

```bash
cd /home/sikmindz/Coding/Gloss

python3 validation/gloss_release_candidate_gate.py --repo .
python3 validation/validate_source_send_gate.py .
python3 validation/validate_frontend_event_routing.py .
python3 validation/validate_chat_terminal_contract.py .
python3 validation/validate_provider_lan_policy.py .
python3 validation/validate_release_receipt_consistency.py .
python3 validation/gloss_p36_live_smoke_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/live-smoke.receipt.json
python3 validation/gloss_p36_performance_gate.py --repo . --receipt docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607/perf.receipt.json

npm test
npm run build

cargo fmt --manifest-path src-tauri/Cargo.toml --all -- --check
cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
```

**Expected:** all pass. If full `cargo test --features semantic-memory-turbo-quant` is run, expect all tests to pass with the known ignored HNSW/usearch teardown test.

---

### Task 15: Commit and handoff

**Objective:** End with a reviewable commit and clean release decision.

**Files:**
- Update: `HOSTILE_AUDITOR_HANDOFF.md` or create: `HOSTILE_AUDITOR_HANDOFF_P36.md`
- Include P36 receipt references

**Step 1: Review diff**

```bash
git diff --stat
git diff --name-status
```

**Step 2: Commit P36 phase**

```bash
git add PHASES/PHASE_129_LIVE_CERTIFICATION_AND_CHAT_PROOF_PLAN.md \
  docs/codex-runs/GLOSS_P36_LIVE_CERTIFICATION_20260607 \
  validation/gloss_p36_live_smoke_gate.py \
  validation/gloss_p36_performance_gate.py \
  scripts/gloss_p36_perf_probe.py \
  fixtures/live-smoke

git commit -m "test: add P36 live certification gates and receipts"
```

If source fixes were required, commit them separately before the final receipt commit.

**Step 3: Final handoff must say exactly one of:**

- `CERTIFIED`: live desktop, chat, retrieval, package, and performance all passed.
- `CANDIDATE_ONLY`: static gates pass, but one or more live certifications not exercised.
- `BLOCKED`: a live P0 flow failed and remains unfixed.

---

## 3. Acceptance criteria

P36 is complete only when:

1. Fresh AppImage exists and metadata receipt includes size + SHA256.
2. Headed smoke receipt exists.
3. No-retrieval chat passes live.
4. Retrieval chat passes live using smoke fixture and citation evidence.
5. Stop/regenerate/continue/notebook-switch terminal behavior is live-smoked.
6. Assistant message persists after reload.
7. Flashcards, quiz, and mind map render from generated studio outputs or are honestly marked not certified.
8. Export/import roundtrip passes live.
9. Performance receipt exists and is either certified or honestly non-certified with blocker reason.
10. P35 release candidate gates still pass.
11. Final P36 receipt is valid JSON and does not overclaim.
12. Git history separates docs/gates from source bug fixes.

---

## 4. Do-not-do list

- Do not claim all intended features work from static gates alone.
- Do not patch chat without a reproduced failure and regression test/gate.
- Do not reuse stale AppImage receipt if the artifact is missing.
- Do not mark performance certified from a quick probe.
- Do not include deferred P36+/P37 features in release scope.
- Do not commit unrelated dirty files unless explicitly reviewed.
- Do not hide headed-environment absence; mark it `not_exercised` and downgrade final decision to `CANDIDATE_ONLY`.
