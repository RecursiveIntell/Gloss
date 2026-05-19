#!/usr/bin/env bash
set -euo pipefail
KIT_ROOT="$(cd "$(dirname "$0")/.." && pwd)"; GLOSS_ROOT="${1:?usage: install_landing_scaffold.sh <Gloss repo root>}"; RUN_ID="GLOSS_COMPLETION_AND_UX_RELEASE_CANDIDATE_P3_20260513"
mkdir -p "$GLOSS_ROOT/docs/codex-runs"; cp -R "$KIT_ROOT/repo_scaffold/docs/codex-runs/$RUN_ID" "$GLOSS_ROOT/docs/codex-runs/"
mkdir -p "$GLOSS_ROOT/docs/release"; cp "$KIT_ROOT/RELEASE_CRITERIA.md" "$GLOSS_ROOT/docs/release/RELEASE_CRITERIA_$RUN_ID.md"; cp "$KIT_ROOT/PACKAGE_SCOPE_POLICY.md" "$GLOSS_ROOT/docs/release/PACKAGE_SCOPE_POLICY_$RUN_ID.md"
echo "Installed scaffold into $GLOSS_ROOT/docs/codex-runs/$RUN_ID"
