#!/usr/bin/env python3
"""Static gate for cancellable provider contract."""
from __future__ import annotations
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.cwd()
FAIL: list[str] = []

def read(rel: str) -> str:
    p = ROOT / rel
    if not p.exists():
        FAIL.append(f"missing required file: {rel}")
        return ""
    return p.read_text(errors="ignore")

providers_mod = read("src-tauri/src/providers/mod.rs")
studio = read("src-tauri/src/commands/studio.rs")
chat_stream = read("src-tauri/src/commands/chat/streaming.rs")
all_provider_code = "\n".join(p.read_text(errors="ignore") for p in (ROOT/"src-tauri/src/providers").glob("*.rs")) if (ROOT/"src-tauri/src/providers").exists() else ""

if "CancellationToken" not in providers_mod + all_provider_code + chat_stream + studio:
    FAIL.append("provider runtime must use CancellationToken or equivalent cancellation token")

if re.search(r"async\s+fn\s+chat\s*\(\s*&self\s*,\s*request\s*:\s*ChatRequest\s*\)", providers_mod, re.S):
    FAIL.append("LlmProvider::chat still accepts only ChatRequest; add execution context/deadline/cancel argument")

raw_calls = []
for rel in ["src-tauri/src/commands/chat/streaming.rs", "src-tauri/src/commands/studio.rs"]:
    txt = read(rel)
    for m in re.finditer(r"provider\.chat\s*\(\s*request\s*\)", txt):
        raw_calls.append(f"{rel}:{txt[:m.start()].count(chr(10))+1}")
if raw_calls:
    FAIL.append("provider.chat(request) call without execution context remains: " + ", ".join(raw_calls))

if re.search(r"tokio::time::timeout\s*\([^;]+provider\.chat\s*\(", studio, re.S):
    FAIL.append("Studio still wraps provider.chat in timeout without visible cancellation context")

if "Client::builder()" in providers_mod and ".timeout(" not in providers_mod:
    FAIL.append("shared reqwest client builder should include explicit timeout/connect timeout or explain ctx-only timeout")

if "reqwest::blocking::Client" in all_provider_code + providers_mod:
    FAIL.append("reqwest::blocking::Client must not appear in provider runtime code")

if FAIL:
    print("FAILURES:")
    for f in FAIL:
        print(f"  - {f}")
    sys.exit(1)
print("gloss_provider_cancellation_static_gate: PASS")
