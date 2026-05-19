#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TMPDIR="$(mktemp -d)"
ARCHIVE="${TMPDIR}/scr-runtime-fresh-unzip.zip"
UNPACK_DIR="${TMPDIR}/unpack"

cleanup() {
  rm -rf "$TMPDIR"
}
trap cleanup EXIT

mkdir -p "$UNPACK_DIR"

python3 "$ROOT/scripts/zip_source_certifier.py" \
  --root "$ROOT" \
  --mode next-codex-context \
  --no-archive-codex-runs \
  --no-strict \
  --output "$ARCHIVE" >/tmp/scr-runtime_fresh_unzip_archive.log

MANIFEST="${ARCHIVE%.zip}.manifest.json"
if [[ ! -f "$MANIFEST" ]]; then
  echo "missing expected manifest: ${MANIFEST}" >&2
  exit 1
fi

python3 "$ROOT/scripts/verify_archive_manifest_parity.py" "$ARCHIVE" "$MANIFEST" | tee /tmp/scr-runtime_fresh_unzip_archive_check.log
python3 "$ROOT/scripts/assert_required_archive_paths.py" "$ARCHIVE" | tee /tmp/scr-runtime_fresh_unzip_required_paths.log

unzip -q "$ARCHIVE" -d "$UNPACK_DIR"

if [[ -f "$UNPACK_DIR/Cargo.toml" && -f "$UNPACK_DIR/README.md" ]]; then
  EXTRACTED="$UNPACK_DIR"
else
  EXTRACTED="$(find "$UNPACK_DIR" -mindepth 1 -maxdepth 1 -type d | sort | head -n 1)"
  if [[ -z "$EXTRACTED" ]]; then
    EXTRACTED="$UNPACK_DIR"
  fi
  if [[ ! -f "$EXTRACTED/Cargo.toml" || ! -f "$EXTRACTED/README.md" ]]; then
    echo "extracted package does not contain expected workspace root" >&2
    exit 1
  fi
fi

(
  cd "$EXTRACTED"
  python3 scripts/validate_schemas.py
  bash scripts/verify_golden_fixtures.sh
  python3 scripts/validate_codex_pack.py
  python3 scripts/assert_codex_active_pack.py
  bash scripts/run_all_checks.sh
)

cat <<'EOF_DOC' > "$ROOT/docs/P31_COMPLETION_FRESH_UNZIP_CERTIFICATION.md"
# Phase 07 — Fresh Unzip Certification

- archive generated at: ${ARCHIVE}
- manifest generated at: ${MANIFEST}
- unpacked path: ${EXTRACTED}
- commands run from fresh unzipped material:
  - python3 scripts/validate_schemas.py
  - bash scripts/verify_golden_fixtures.sh
  - python3 scripts/validate_codex_pack.py
  - python3 scripts/assert_codex_active_pack.py
  - bash scripts/run_all_checks.sh
EOF_DOC

echo "OK: SCR fresh-unzip checks passed"
