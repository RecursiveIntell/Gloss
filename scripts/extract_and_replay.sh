#!/usr/bin/env bash
set -euo pipefail
ZIP_PATH="${1:?usage: extract_and_replay.sh <package.zip> <replay-dir>}"
REPLAY_DIR="${2:?usage: extract_and_replay.sh <package.zip> <replay-dir>}"
RUN_ID="GLOSS_COMPLETION_AND_UX_RELEASE_CANDIDATE_P3_20260513"
rm -rf "$REPLAY_DIR"; mkdir -p "$REPLAY_DIR"; unzip -q "$ZIP_PATH" -d "$REPLAY_DIR"
if [ -d "$REPLAY_DIR/Gloss" ]; then GLOSS="$REPLAY_DIR/Gloss"; elif [ -d "$REPLAY_DIR/Coding/Gloss" ]; then GLOSS="$REPLAY_DIR/Coding/Gloss"; else echo "Could not find extracted Gloss root" >&2; exit 2; fi
OUT="$GLOSS/docs/codex-runs/$RUN_ID"; mkdir -p "$OUT/reports" "$OUT/logs" "$OUT/receipts"
{ echo "zip=$ZIP_PATH"; echo "replay_dir=$REPLAY_DIR"; echo "gloss=$GLOSS"; echo "zip_sha256=$(sha256sum "$ZIP_PATH" | awk '{print $1}')"; echo "absolute path references:"; grep -R "/home/sikmindz/Coding" -n "$GLOSS" --include='Cargo.toml' --include='package.json' || true; } > "$OUT/reports/EXTRACTED_REPLAY.md"
CARGO_TARGET_DIR="$REPLAY_DIR/.cargo-target" bash "$(dirname "$0")/run_command_bar.sh" "$GLOSS"
