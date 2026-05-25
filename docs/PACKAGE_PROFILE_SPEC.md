# Package profile spec

Current generic next-codex package is too large/noisy because vendored crates dominate, while release evidence excludes needed logs/screenshots.

## Required profiles

### codex-source-small

Purpose: implementation pass context.

Include:

```text
src/
src-tauri/src/
src-tauri/Cargo.toml
src-tauri/tauri.conf.json
Cargo.toml
Cargo.lock
package.json
package-lock.json
scripts/
fixtures/
docs/CURRENT_FEATURE_MATRIX.md
README.md
AGENTS.md
ISSUE_LEDGER.csv
```

Exclude vendored crates except root manifests/summaries.

### offline-build-full

Purpose: build from package without network.

Include vendored crates and lockfiles.

### release-evidence

Purpose: public/internal release proof packet.

Include:

```text
docs/codex-runs/{RUN_ID}/COMMANDS_RUN.log
docs/codex-runs/{RUN_ID}/VALIDATION_RESULTS.md
docs/codex-runs/{RUN_ID}/DENSE_INDEXING_RECEIPT.json
docs/codex-runs/{RUN_ID}/TURBOQUANT_BUILD_RECEIPT.json
docs/codex-runs/{RUN_ID}/LIVE_DESKTOP_SMOKE_RECEIPT.json
docs/codex-runs/{RUN_ID}/PACKAGE_WARNING_REVIEW.md
docs/codex-runs/{RUN_ID}/FINAL_RECEIPT.json
docs/codex-runs/{RUN_ID}/screenshots/final_desktop_smoke.png
```

Release-evidence must sanitize secrets and can redact private paths, but it must not omit evidence required by release gates.
