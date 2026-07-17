# Gloss Hostile Audit Summary — 2026-05-27

## Verdict

**NO-GO for release / public-ready claims.** The latest package is useful as a Codex context artifact, but it is not a closure artifact. The most urgent runtime defect is the chat/Ollama stream termination contract: Gloss currently finalizes only after the response body stream ends, while Ollama's protocol exposes a terminal `done=true` frame. That can leave the UI in a streaming state even after the model finished, especially if the HTTP body stalls or EOF is delayed.

## Evidence basis

Inspected current package:

- `Gloss-generic-rust-next-codex-context-20260526T232615Z.zip`
- `Gloss-generic-rust-next-codex-context-20260526T232615Z.report.md`
- `Gloss-generic-rust-next-codex-context-20260526T232615Z.findings.json`
- `Gloss-generic-rust-next-codex-context-20260526T232615Z.excluded.json`
- `Gloss-generic-rust-next-codex-context-20260526T232615Z.codex-archive.json`

Package facts from the report:

- Root: `/home/sikmindz/Coding/Gloss`
- Archive root: `/home/sikmindz/Coding`
- Mode: `next-codex-context`
- Included files: `29,749`
- Included bytes: `539,775,278`
- Excluded files: `8,443`
- Findings: `72`, with `0` errors and `71` warnings
- Ecosystems detected: Rust, Node, Git
- Rust dry-run status: `available-not-run`
- Node dry-run status: `available-not-run`

## Commands I ran in this sandbox

| Command | Result | Notes |
|---|---:|---|
| `npm run build` | PASS | Frontend/Vite bundle builds. |
| `npm test` | PASS | 12 frontend/static UI contract checks passed. |
| `python3 scripts/chat_runtime_static_audit.py --repo .` | PASS | But output says active notebook chat event filters found: 0; validator is too weak. |
| `python3 validation/gloss_timeout_partial_continuation_gate.py --repo .` | PASS | Static gate passes; live timeout fixture still not proven by receipt. |
| `python3 validation/gloss_package_scope_gate.py --repo .` | FAIL | `no Gloss package manifest found`. |
| `timeout 20s python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT` | TIMEOUT | Aggregate release gate did not emit structured failure. |
| `python3 validation/gloss_legacy_office_extractors_gate.py --repo .` | FAIL | Missing `xls2csv` and `catppt`. |
| `command -v cargo rustc ollama xls2csv catppt antiword` | PARTIAL | `cargo`, `rustc`, `ollama`, `xls2csv`, `catppt` absent; `antiword` present. |

Logs are in `live_check_logs/`.

## Highest-priority root issue

### Chat infinite response root cause

The immediate root issue is **terminal-frame mishandling**:

1. Backend receives provider stream tokens.
2. On `done=true`, it sets `sent_done = true` but does not break.
3. It emits every `chat:token` event with `done: false`.
4. It emits `chat:done` only after the stream loop exits by EOF.
5. If EOF is delayed or never observed, the UI waits until idle timeout or remains effectively stuck.

Correct contract: **`done=true` is the provider terminal frame. EOF is transport cleanup, not semantic completion.**

Secondary chat root issues:

- Partial content is only DB-persisted after success, so timeout/error/cancel can lose or desynchronize partial output.
- Stop/cancel is split between frontend-local state and backend provider lifecycle.
- Background summary jobs share LLM/GPU gates with foreground chat and may make chat appear hung even when Ollama is healthy.
- Runtime receipts lack enough terminal metadata to prove what happened.

## S0/S1 blockers

| Area | Blocker |
|---|---|
| Chat/Ollama | Done frame does not finalize immediately. |
| Chat persistence | Partial output not durably persisted for error/timeout/cancel. |
| Chat cancellation | Stop is not backend-authoritative. |
| Runtime gates | Background jobs can hold gates too long. |
| Validation | Package scope gate fails. |
| Validation | Release candidate gate can hang. |
| Validation | Missing script referenced by `run_all_checks.sh`. |
| Desktop | Desktop smoke is contract-only, not release-grade. |
| Toolchain | Rust/Ollama unavailable in this sandbox; live proof absent. |
| Packaging | Package is broad `/Coding` context package, not clean release/source package. |
| Proof | TurboQuant exact runtime proof missing. |
| Security | Secret scan findings and secret-like filename exclusions unresolved. |

## Deliverables

- `01_ISSUE_LEDGER.csv` — expanded hostile issue ledger.
- `02_CHAT_INFINITE_RESPONSE_ROOT_CAUSE.md` — root-cause analysis and intended stream contract.
- `03_DETAILED_FIX_INSTRUCTIONS.md` — detailed fix instructions by subsystem.
- `04_NEXT_CODEX_MASTER_PROMPT.md` — closing/hardening/final pass prompt.
- `05_PHASE_PLAN.md` and `06_PHASE_PROMPTS/` — phase-by-phase Codex execution prompts.
- `07_VALIDATION_COMMANDS.md` — validation command set.
- `08_ACCEPTANCE_GATES.md` — hard acceptance gates.
- `09_ROLLBACK_PLAN.md` — rollback and quarantine path.
- `10_HOSTILE_AUDITOR_HANDOFF_TEMPLATE.md` — final handoff template.
- `AGENTS.md` — repo-level guidance candidate.
- `scripts/chat_stream_contract_probe.py` — static probe skeleton for the done-frame contract.
