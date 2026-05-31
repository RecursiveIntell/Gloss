#!/usr/bin/env bash
# build-appimage.sh — Build Gloss AppImage for Linux
#
# Two strategies:
#   1. Tauri native (preferred): cargo tauri build --bundles appimage
#   2. Manual appimagetool (fallback): builds AppDir then packages with appimagetool
#
# Usage:
#   ./scripts/build-appimage.sh           # auto-detect strategy
#   ./scripts/build-appimage.sh --force   # force manual appimagetool (skip Tauri bundler)
#   ./scripts/build-appimage.sh --check   # verify prerequisites only
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BUNDLE_DIR="$REPO_ROOT/target/release/bundle/appimage"
BINARY="$REPO_ROOT/target/release/gloss"
OUTPUT="$BUNDLE_DIR/Gloss_1.0.0_amd64.AppImage"
ICON="$REPO_ROOT/src-tauri/icons/icon.png"
TAURI_CACHE="$HOME/.cache/tauri"
LINUXDEPLOY="$TAURI_CACHE/linuxdeploy-x86_64.AppImage"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

log()  { echo -e "${GREEN}[build-appimage]${NC} $*"; }
warn() { echo -e "${YELLOW}[build-appimage]${NC} $*"; }
err()  { echo -e "${RED}[build-appimage]${NC} $*"; }

check_prereqs() {
    local ok=true

    if ! command -v cargo &>/dev/null; then
        err "cargo not found — install Rust"
        ok=false
    fi

    if ! command -v mksquashfs &>/dev/null; then
        err "mksquashfs not found — install squashfs-tools"
        ok=false
    fi

    # Check for cached linuxdeploy (Tauri downloads this on first build)
    if [[ ! -f "$LINUXDEPLOY" ]]; then
        warn "linuxdeploy not cached ($LINUXDEPLOY)"
        warn "Will be downloaded during Tauri build"
    fi

    # Check for appimagetool (extracted from linuxdeploy-plugin-appimage)
    local appimagetool
    appimagetool=$(find /tmp -name appimagetool -path "*/usr/bin/appimagetool" 2>/dev/null | head -1 || true)
    if [[ -z "$appimagetool" || ! -x "$appimagetool" ]]; then
        warn "No cached appimagetool found — manual fallback may fail"
        warn "Run a Tauri build once to populate the cache"
    fi

    $ok
}

build_binary() {
    log "Building Rust binary (release)..."
    cargo build --manifest-path "$REPO_ROOT/src-tauri/Cargo.toml" \
        --features semantic-memory-turbo-quant \
        --release 2>&1 | tail -5

    if [[ ! -f "$BINARY" ]]; then
        err "Binary not found at $BINARY — build failed"
        return 1
    fi
    log "Binary ready: $(du -h "$BINARY" | cut -f1)"
}

build_frontend() {
    log "Building frontend..."
    (cd "$REPO_ROOT" && npm run build 2>&1 | tail -3)
}

try_tauri_bundler() {
    log "Attempting Tauri bundler (AppImage)..."
    if cd "$REPO_ROOT" && cargo tauri build --bundles appimage 2>&1; then
        if [[ -f "$OUTPUT" ]]; then
            log "Tauri bundler succeeded: $(du -h "$OUTPUT" | cut -f1)"
            return 0
        fi
    fi
    warn "Tauri bundler failed — falling back to manual appimagetool"
    return 1
}

manual_appimage() {
    local tmp_appdir
    tmp_appdir=$(mktemp -d /tmp/gloss-appdir.XXXXXX)
    trap 'rm -rf "$tmp_appdir"' EXIT

    log "Creating AppDir at $tmp_appdir..."

    # Binary
    mkdir -p "$tmp_appdir/usr/bin"
    cp "$BINARY" "$tmp_appdir/usr/bin/gloss"
    chmod +x "$tmp_appdir/usr/bin/gloss"

    # Icon
    if [[ -f "$ICON" ]]; then
        cp "$ICON" "$tmp_appdir/Gloss.png"
        mkdir -p "$tmp_appdir/usr/share/icons/hicolor/256x256/apps"
        cp "$ICON" "$tmp_appdir/usr/share/icons/hicolor/256x256/apps/Gloss.png"
    fi

    # Desktop entry
    cat > "$tmp_appdir/Gloss.desktop" << 'DESKTOP'
[Desktop Entry]
Name=Gloss
Exec=gloss
Icon=Gloss
Type=Application
Categories=Office;
Comment=A local-first desktop notebook/RAG app
DESKTOP

    # AppRun launcher
    cat > "$tmp_appdir/AppRun" << 'APPRUN'
#!/bin/bash
HERE="$(dirname "$(readlink -f "${0}")")"
exec "${HERE}/usr/bin/gloss" "$@"
APPRUN
    chmod +x "$tmp_appdir/AppRun"

    # Find appimagetool
    local appimagetool
    appimagetool=$(find /tmp -name appimagetool -path "*/usr/bin/appimagetool" 2>/dev/null | head -1 || true)
    if [[ -z "$appimagetool" || ! -x "$appimagetool" ]]; then
        # Try extracting linuxdeploy plugin
        if [[ -f "$LINUXDEPLOY" ]]; then
            log "Extracting linuxdeploy-plugin-appimage to get appimagetool..."
            local extract_dir
            extract_dir=$(mktemp -d /tmp/linuxdeploy-extract.XXXXXX)
            "$LINUXDEPLOY" --appimage-extract-and-run \
                --appdir "$tmp_appdir" \
                --plugin appimage 2>&1 | tail -3 || true
            rm -rf "$extract_dir"
        fi
        appimagetool=$(find /tmp -name appimagetool -path "*/usr/bin/appimagetool" 2>/dev/null | head -1 || true)
    fi

    if [[ -z "$appimagetool" || ! -x "$appimagetool" ]]; then
        err "appimagetool not found — cannot create AppImage"
        return 1
    fi

    log "Packaging with appimagetool..." 
    mkdir -p "$BUNDLE_DIR"
    NO_STRIP=1 "$appimagetool" "$tmp_appdir" "$OUTPUT" 2>&1 | tail -3

    if [[ -f "$OUTPUT" ]]; then
        log "AppImage created: $(du -h "$OUTPUT" | cut -f1)"
        return 0
    else
        err "AppImage not created"
        return 1
    fi
}

# --- Main ---
cd "$REPO_ROOT"

case "${1:-}" in
    --check)
        check_prereqs
        log "Prerequisites check complete"
        exit 0
        ;;
    --force)
        build_frontend
        build_binary
        manual_appimage
        ;;
    *)
        check_prereqs || exit 1
        build_frontend
        build_binary
        if ! try_tauri_bundler; then
            manual_appimage
        fi
        ;;
esac

log "Done — AppImage at: $OUTPUT"
