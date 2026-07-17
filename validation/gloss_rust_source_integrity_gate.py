#!/usr/bin/env python3
"""Reject serialized Rust source accidentally pasted into Rust files."""

from __future__ import annotations

import re
import sys
from pathlib import Path


ESCAPED_COMMAND = re.compile(r"\\n\\n#\[tauri::command\]")


def suspicious_markers(source: str) -> list[int]:
    """Return 1-based lines containing a marker outside Rust string/comment text."""
    markers: list[int] = []
    state = "code"
    escaped = False
    raw_hashes = 0
    index = 0
    line = 1

    while index < len(source):
        char = source[index]

        if char == "\n":
            line += 1

        if state == "line_comment":
            if char == "\n":
                state = "code"
            index += 1
            continue

        if state == "block_comment":
            if source.startswith("*/", index):
                state = "code"
                index += 2
            else:
                index += 1
            continue

        if state == "string":
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                state = "code"
            index += 1
            continue

        if state == "raw_string":
            terminator = '"' + ("#" * raw_hashes)
            if source.startswith(terminator, index):
                state = "code"
                index += len(terminator)
            else:
                index += 1
            continue

        if source.startswith("//", index):
            state = "line_comment"
            index += 2
            continue
        if source.startswith("/*", index):
            state = "block_comment"
            index += 2
            continue
        if char == '"':
            state = "string"
            index += 1
            continue
        if char == "r":
            match = re.match(r'r(#+)?"', source[index:])
            if match:
                raw_hashes = len(match.group(1) or "")
                state = "raw_string"
                index += len(match.group(0))
                continue

        if ESCAPED_COMMAND.match(source, index):
            markers.append(line)
            index += len("\\n\\n#[tauri::command]")
            continue

        index += 1

    return markers


def main() -> int:
    root = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
    rust_root = root / "src-tauri" / "src"
    failures: list[tuple[Path, int]] = []
    for path in sorted(rust_root.rglob("*.rs")):
        for line in suspicious_markers(path.read_text(encoding="utf-8")):
            failures.append((path, line))

    if failures:
        for path, line in failures:
            print(f"gloss_rust_source_integrity_gate: serialized Rust source blob: {path}:{line}")
        return 1

    print("gloss_rust_source_integrity_gate: PASS")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
