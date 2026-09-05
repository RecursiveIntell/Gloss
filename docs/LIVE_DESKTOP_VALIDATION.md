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

**Expected exit status is 2 (blocked)** even when this baseline succeeds: the
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
dbus-run-session -- xvfb-run -a python3 scripts/live_desktop_smoke.py \
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

Replay a complete reviewed receipt against its exact clean checkout:

```bash
python3 scripts/gloss_desktop_smoke_harness.py --repo . \
  --live-receipt .codex-run-receipts/desktop-<run-id>/LIVE_DESKTOP_SMOKE_RECEIPT.json \
  --require-live --receipt .codex-run-receipts/desktop-validation.json
```

Receipt validator regressions are independent of live proof:

```bash
python3 -m unittest discover -s scripts/tests -p 'test_desktop_smoke_receipt.py' -v
```

No schema migration or user-data mutation is needed to roll back these scripts.
Revert them together and retain evidence, but do not promote old boolean-only
receipts to release proof. The baseline currently has no local live execution
claim until it has run on a host with the native dependencies and display.
