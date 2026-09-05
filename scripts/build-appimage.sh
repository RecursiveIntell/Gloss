#!/usr/bin/env bash
# Canonical locked AppImage build; never substitute ambient manual tools.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$REPO_ROOT"
for tool in cargo npm mksquashfs; do
  if ! command -v "$tool" >/dev/null 2>&1; then
    echo "Required AppImage build tool is missing: $tool" >&2
    exit 1
  fi
done
if [[ ! -x node_modules/.bin/tauri ]]; then
  echo "Install locked frontend dependencies with npm ci before packaging." >&2
  exit 1
fi
if [[ "${1:-}" == "--check" ]]; then
  exit 0
fi
if [[ $# -ne 0 ]]; then
  echo "Usage: scripts/build-appimage.sh [--check]" >&2
  exit 2
fi
# Official AppImage extraction mode runs bundler tools without a FUSE mount.
# https://docs.appimage.org/user-guide/troubleshooting/fuse.html
export APPIMAGE_EXTRACT_AND_RUN=1
exec npm exec -- tauri build --ci --no-sign --bundles appimage \
  --features semantic-memory-turbo-quant -- --locked
