# Live desktop acceptance

`scripts/gloss_desktop_smoke_harness.py` validates current desktop evidence using
`GlossLiveDesktopSmokeReceiptV2`. Historical receipts remain historical records;
boolean-only receipts cannot certify the current checkout.

The receipt must bind a clean current source snapshot (the shared
`scripts/source_snapshot.py` contract), native build command/log and executable
digest, actual run interval, isolated data location, explicit safety observations,
and every required case. Each passing case needs a written observation, runtime
log and screenshot with relative paths and SHA-256 digests. Keep the complete
evidence directory together. A path outside that directory, missing artifact,
changed digest, blocked case or different source fails the gate. Digests establish
integrity and source binding, not runner authenticity: review the trusted run and
its observations. This gate alone is not exhaustive production certification.

## Repeatable native baseline

The standard-library Python driver builds and launches **real Tauri** through
`tauri-driver` and Linux `WebKitWebDriver`. It creates empty XDG data/config/cache
directories, opens the native window, creates a notebook through the UI, restarts
the app to verify persistence, deletes the notebook and verifies deletion after
a second restart. It does not substitute a browser mock or replace Tauri IPC.
It does not use the operator's notebooks or provider credentials.

On a supported Linux host, install the normal Gloss build dependencies plus
`webkit2gtk-driver` and `xvfb`. CI pins `tauri-driver` to
[`2.0.6`](https://tauri.app/release/tauri-driver/v2.0.6/), published May 6, 2026,
and installs it with `cargo install tauri-driver --version 2.0.6 --locked`.
Tauri's [official CI guide](https://v2.tauri.app/develop/tests/webdriver/ci/) describes
the native driver/display requirements. The driver adds no application plugin.

With a clean committed checkout and Rust/npm dependencies available:

```bash
xvfb-run -a python3 scripts/live_desktop_smoke.py --repo .
```

Evidence is written to a new ignored `.codex-run-receipts/desktop-<run-id>/`
directory. A custom `--output` must also be new and should be outside the source
snapshot or ignored. No existing evidence directory is reused. The executable,
build output, driver output, screenshots, request/response observations and
isolated profile are retained for inspection.

**Expected exit status is 2 (blocked)** without a required-scope flag, even when this baseline succeeds: the
remaining live cases include scoped folder import, real-provider chat with and
without retrieval, restart persistence, cancellation/retry, notebook switching,
backend/degradation disclosure, citations, model/prompt UI, Notes persistence,
and source deletion/restart. The driver does not fabricate these observations or
mark unobserved safety flags false. A build/driver failure exits 1. Missing host
capabilities are listed before attempting a build or launching the app.

The `desktop-baseline` GitHub Actions job runs on Ubuntu 24.04 for pull requests,
main/master pushes and manual workflow dispatch. It uses an isolated D-Bus session
and Xvfb, installs the native WebKit driver, then runs:

```bash
xvfb-run -a dbus-run-session -- python3 scripts/live_desktop_smoke.py \
  --repo . --require-baseline --output .codex-run-receipts/desktop-baseline
```

`--require-baseline` returns 0 only after both real baseline cases pass and native
cleanup/source consistency checks succeed. Missing capabilities exit 2 and fail
the job. Build/driver/cleanup failures exit 1. This mode does **not** convert an
arbitrary exit 2 into success. The receipt separately records `baseline_status`
and preserves `status=blocked`, unobserved cases and safety flags for complete
release acceptance. CI retains all resulting evidence for 14 days, including
failed build logs. A green baseline job proves only this native UI baseline,
not real-model behavior, package installation, cross-platform support or the
complete desktop acceptance inventory. A workflow added to source has no proof
value until its job executes on the exact candidate commit.

## Integrated real-Ollama workflow

The same native driver has an explicit `--ollama-config PATH --require-integrated`
mode. The owned-runtime canary supplies the validated endpoint, runtime version,
installed chat/embedding model names and complete model digests, and keeps that
runtime alive while the UI runs:

```bash
xvfb-run -a dbus-run-session -- python3 scripts/live_ollama_canary.py \
  --repo . --desktop --output .codex-run-receipts/ollama-desktop
```

The canary is restricted to its isolated supported GitHub-hosted runner. It does
not reuse an operator's Ollama process or profile. Integrated mode additionally
requires `xdotool` for keyboard interaction with the real native folder chooser.
The child config schema is `gloss-desktop-ollama-config/v1`: `schema`, `provider`
(`ollama`), `base_url` (explicit HTTP loopback IP and port), `runtime_version`,
`chat_model`, `embedding_model`, `chat_model_digest`, `embedding_model_digest`.
Unknown fields, credentials, non-loopback URLs and incomplete digests are rejected.
The complete validated config is echoed in the child receipt and checked against
the parent runtime/model snapshot.

| Required case | Observed UI action and assertion |
| --- | --- |
| `startup_idle` | Empty isolated native window, without queued imports. |
| `notebook_crud_restart` | Create, restart, cancel deletion, confirm deletion, restart. |
| `chat_no_retrieval` | Save Ollama settings with Apply; real response with zero context. |
| `chat_persistence_restart` | Restart and reopen the observed Conversation dropdown option; identical saved answer. |
| `model_dropdown_and_prompt` | Installed model selection persists; captured full prompt identifies provider/model. |
| `chat_cancel_and_retry` | Observe streamed tokens, Stop, inspect cancelled receipt, Edit and rerun successfully. |
| `folder_import_scope` | Native chooser imports only two nested fixture files; outside-directory symlink remains absent. |
| `citation_evidence` | Select Atlas alone; exact fixture fact, preserved scope, valid citation and actual source viewer. |
| `retrieval_backend_and_degradation` | Missing embedding model causes visible failure and disclosed BM25 degradation; restore model, Retry ingestion, Rebuild dense index, observe 100% coverage. |
| `notes_persistence` | Save a long note, restart, expand and compare its full text. |
| `notebook_switch_isolation` | Second notebook has separate chat/sources/Notes; query its distinct source, switch back through confirmed activation. |
| `source_delete_restart` | Delete selected sources, restart, observe zero-context chat and retained Notes. |

All product mutations use real WebDriver element/keyboard actions. DOM reads
inspect rendered controls and evidence; there are no injected dialog results,
mocked IPC, internal store changes or raw IPC calls. Healthy scoped retrieval must
report `native-hybrid`/`hybrid_rrf` or `native-dense`/`dense_only`. The deliberate
failure must report `gloss-local`/`bm25_only` with a visible dense/index/embedding
reason; an undisclosed fallback fails the case.

`--require-integrated` succeeds only with twelve unique passing observations,
the bound clean source/build/config, and all three observed safety flags false.
Missing or unknown flags never become false by default. `hidden_fallback=false`
means the workflow's expected degradation was disclosed; it does not assert
that degradation never occurred. `raw_uuid_flood=false` covers captured normal
answer/source presentation, excluding explicitly opened receipt inspectors.
A receipt may retain a separately blocked full-release scope while the required
integrated scope passes. This remains bounded Linux UI evidence, not an exhaustive
production certificate or proof for other operating systems.

Every passing case retains a DOM snapshot, screenshot and actual action trace.
Failure output includes the case, error/stack location, bounded visible UI and
last request, with the complete evidence retained under the child output directory.
The parent canary preserves a bounded failure-log tail in CI when artifact
downloads are inaccessible. Contract tests validate acceptance rules; they do
not constitute GUI execution.

## Extracted AppImage replay

The package gate supplies `--prebuilt-config PATH --require-baseline` to reuse
the same startup/CRUD/restart observations against the actual extracted AppRun.
`gloss-desktop-prebuilt/v1` binds the current clean source, successful build command
and log, archive digest, launcher path/digest and native binary path/digest.
The driver verifies those files, retains copies as evidence, and launches AppRun
in place with the extracted directory as its working directory. It does not copy
the launcher away from its payload for execution or rebuild a substitute binary.
The package owner separately verifies archive integrity and that payload symlinks
stay inside the extraction. Baseline package replay does not claim the integrated
real-model cases ran against that package.

Replay a complete reviewed receipt against its exact clean checkout:

```bash
python3 scripts/gloss_desktop_smoke_harness.py --repo . \
  --live-receipt .codex-run-receipts/desktop-<run-id>/LIVE_DESKTOP_SMOKE_RECEIPT.json \
  --require-live --receipt .codex-run-receipts/desktop-validation.json
```

Receipt validator regressions are independent of live proof:

```bash
python3 -m unittest discover -s scripts/tests -p 'test_desktop_smoke_receipt.py' -v
python3 -m unittest discover -s scripts/tests -p 'test_live_desktop_baseline.py' -v
```

No schema migration or user-data mutation is needed to roll back these scripts.
Revert them together and retain evidence, but do not promote old boolean-only
receipts to release proof. New driver modes have no execution claim until the
native observations pass on their exact candidate source snapshot.
