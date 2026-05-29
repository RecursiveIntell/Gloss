#!/usr/bin/env python3
"""Static audit for Gloss chat no-response defects.

This script is intentionally conservative: it flags known dangerous patterns that
can produce silent chat failure. It does not prove the app works.
"""
from __future__ import annotations
import argparse
from pathlib import Path
import re
import sys

CHECKS = []

def check(name):
    def deco(fn):
        CHECKS.append((name, fn)); return fn
    return deco

def read(repo: Path, rel: str) -> str:
    p = repo / rel
    if not p.exists():
        return ""
    return p.read_text(encoding="utf-8", errors="replace")

@check("frontend_chat_events_forward_to_store")
def _(repo: Path):
    text = read(repo, "src/App.tsx")
    failures = []
    for event in ["onChatToken", "onChatStatus", "onChatError", "onChatEvidence"]:
        idx = text.find(f"const unlisten = {event}")
        if idx == -1:
            failures.append(f"missing {event}")
            continue
        block = text[idx : idx + 900]
        filter_idx = block.find("activeNotebookId")
        store_idx = block.find("useChatStore.getState()")
        if filter_idx != -1 and (store_idx == -1 or filter_idx < store_idx):
            failures.append(f"{event} filters activeNotebookId before chatStore")
    return not failures, "chat lifecycle events forward to chatStore" if not failures else "; ".join(failures)

@check("frontend_has_stream_identity_state")
def _(repo: Path):
    text = read(repo, "src/stores/chatStore.ts")
    ok = (
        "streamingNotebookId" in text
        and "streamingMessageId" in text
        and "streamingNotebookId !== notebookId" in text
        and "streamingMessageId !== messageId" in text
    )
    return ok, "streamingNotebookId/streamingMessageId filters present" if ok else "missing stream identity filtering"

@check("provider_config_uses_provider_rows")
def _(repo: Path):
    settings = read(repo, "src-tauri/src/commands/settings.rs")
    providers = read(repo, "src-tauri/src/providers/mod.rs")
    bad_settings = any(k in settings for k in ["get_setting(\"ollama_url\")", "get_setting(\"openai_base_url\")", "get_setting(\"anthropic_base_url\")", "get_setting(\"llamacpp_url\")"])
    bad_registry = any(k in providers for k in ["get_setting(\"ollama_url\")", "get_setting(\"openai_base_url\")", "get_setting(\"anthropic_base_url\")", "get_setting(\"llamacpp_url\")"])
    return not (bad_settings or bad_registry), f"settings_url_reads settings.rs={bad_settings} providers/mod.rs={bad_registry}"

@check("chat_attempt_trace_exists")
def _(repo: Path):
    hay = "\n".join(read(repo, rel) for rel in ["src-tauri/src/commands/chat/mod.rs", "src-tauri/src/lib.rs", "src/lib/tauri.ts"])
    ok = "ChatAttemptTrace" in hay and "get_last_chat_attempt_trace" in hay
    return ok, "ChatAttemptTraceV1 command present" if ok else "missing ChatAttemptTraceV1/get_last_chat_attempt_trace"

@check("provider_only_smoke_exists")
def _(repo: Path):
    hay = "\n".join(read(repo, rel) for rel in ["src-tauri/src/commands/chat/mod.rs", "src-tauri/src/commands/settings.rs", "src-tauri/src/lib.rs", "src/lib/tauri.ts"])
    ok = "debug_chat_provider_smoke" in hay
    return ok, "debug_chat_provider_smoke present" if ok else "missing debug_chat_provider_smoke"

@check("ollama_stream_error_detection")
def _(repo: Path):
    text = read(repo, "src-tauri/src/providers/ollama.rs")
    ok = re.search(r"\.get\(\s*\"error\"\s*\)", text) is not None
    return ok, "Ollama stream error field detection present" if ok else "missing Ollama stream JSON error detection"

@check("semantic_memory_timeout_fallback")
def _(repo: Path):
    text = read(repo, "src-tauri/src/commands/chat/mod.rs")
    ok = "semantic_memory_search_timeout" in text and "semantic_memory_search_fallback" in text and "memory_backend_fallback" in text
    return ok, "semantic-memory timeout/fallback status present" if ok else "missing semantic-memory bounded fallback indicators"

@check("current_run_not_stale_p30")
def _(repo: Path):
    text = read(repo, "docs/codex-runs/CURRENT_RUN.md")
    stale = "P30" in text and "CHAT_RUNTIME_FIX" not in text
    return not stale, f"CURRENT_RUN.md content: {text.strip()!r}"

@check("missing_auto_phase_runner_reference")
def _(repo: Path):
    text = read(repo, "scripts/run_completion_checks.sh")
    legacy_runner = "." + "codex/tools/auto_phase_runner.py"
    references = "codex/prompts/" in text or legacy_runner in text
    exists = (repo / "codex/prompts").exists() or (repo / legacy_runner).exists()
    return (not references) or exists, f"references auto_phase_runner={references}, exists={exists}"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    failures = 0
    print(f"CHAT_RUNTIME_STATIC_AUDIT repo={repo}")
    for name, fn in CHECKS:
        ok, detail = fn(repo)
        status = "PASS" if ok else "FAIL"
        if not ok:
            failures += 1
        print(f"{status}\t{name}\t{detail}")
    print(f"SUMMARY failures={failures} checks={len(CHECKS)}")
    return 1 if failures else 0

if __name__ == "__main__":
    raise SystemExit(main())
