#!/usr/bin/env python3
"""Static gate for Gloss chat/runtime repair invariants.

Usage: python3 validation/gloss_runtime_static_gate.py /path/to/Gloss
"""
from __future__ import annotations
import re
import sys
from pathlib import Path

ROOT = Path(sys.argv[1]) if len(sys.argv) > 1 else Path.cwd()
FAIL: list[str] = []
WARN: list[str] = []

def read(rel: str) -> str:
    p = ROOT / rel
    if not p.exists():
        FAIL.append(f"missing required file: {rel}")
        return ""
    return p.read_text(errors="ignore")

def require(cond: bool, msg: str) -> None:
    if not cond:
        FAIL.append(msg)

def warn(cond: bool, msg: str) -> None:
    if not cond:
        WARN.append(msg)

app = read("src/App.tsx")
chat_store = read("src/stores/chatStore.ts")
chat_mod = read("src-tauri/src/commands/chat/mod.rs")
chat_stream = read("src-tauri/src/commands/chat/streaming.rs")
state = read("src-tauri/src/state.rs")

require("rehydrateConversation" in chat_store, "chatStore must define rehydrateConversation for DB-backed reconciliation")
require("loadMessages" in chat_store and "rehydrateConversation" in chat_store, "chatStore must use loadMessages in reconciliation path")
require("getChatEventsSince" in chat_store or "get_chat_events_since" in chat_store or "getChatEventsSince" in app, "frontend must call stream replay API")
require("focus" in app.lower() or "visibilitychange" in app.lower(), "App must rehydrate/replay on window focus or visibility change")
require("chat:done" in app or "onChatDone" in app or "done" in app and "rehydrate" in app.lower(), "terminal event path must trigger reconciliation")
require("ChatStreamEvent" in chat_mod + chat_stream + state, "backend must define/use ChatStreamEvent")
require("get_chat_events_since" in chat_mod + chat_stream + state, "backend must expose get_chat_events_since command")
require("attempt_id" in chat_mod + chat_stream + state, "chat runtime must carry durable attempt_id")
require("chat_attempt" in chat_mod + chat_stream + state.lower(), "chat attempts must be durable or represented in runtime state")
require("localStorage.getItem(ACTIVE_NB_KEY)" not in chat_store, "chatStore must not compare active notebook via stale localStorage ACTIVE_NB_KEY")
require("pendingMessageIds" in chat_store and re.search(r"pendingMessageIds\s*:\s*\{[^}]*assistantMessageId", chat_store, re.S), "assistant message id should be registered before backend response to avoid token race")
require("messages: shouldAppendToVisibleMessages ? [...state.messages, assistantMsg]" not in chat_store, "finalizeMessage must not blindly append assistant from streamingContent only")
require("stop_chat" in chat_mod, "stop_chat command missing")
require("CancellationToken" in chat_mod + chat_stream + state, "chat cancellation must use CancellationToken or equivalent targeted cancellation")

if WARN:
    print("WARNINGS:")
    for w in WARN:
        print(f"  - {w}")
if FAIL:
    print("FAILURES:")
    for f in FAIL:
        print(f"  - {f}")
    sys.exit(1)
print("gloss_runtime_static_gate: PASS")
