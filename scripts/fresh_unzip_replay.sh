#!/usr/bin/env bash
set -euo pipefail
ZIP_PATH="${1:-}"
if [[ -z "$ZIP_PATH" || ! -f "$ZIP_PATH" ]]; then
  echo "usage: $0 /path/to/Gloss-context.zip" >&2
  exit 2
fi
TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
unzip -q "$ZIP_PATH" -d "$TMP"
if [[ -d "$TMP/Gloss" ]]; then
  REPO="$TMP/Gloss"
elif [[ -d "$TMP"/Gloss-* ]]; then
  REPO="$(find "$TMP" -maxdepth 2 -type d -name Gloss | head -1)"
else
  REPO="$(find "$TMP" -maxdepth 3 -type f -path '*/src-tauri/Cargo.toml' -print -quit | xargs -r dirname | xargs -r dirname)"
fi
if [[ -z "${REPO:-}" || ! -d "$REPO" ]]; then
  echo "could not locate Gloss repo root inside archive" >&2
  find "$TMP" -maxdepth 3 -type d | sed -n '1,80p' >&2
  exit 1
fi
cd "$REPO"
echo "fresh-unzip repo: $REPO"
python3 scripts/check_gloss_active_validation_scope.py --repo .
python3 scripts/check_feature_flags_static.py --repo .
python3 scripts/check_release_eligibility_current.py --repo . || true
python3 scripts/gloss_button_up_gate.py --repo .
