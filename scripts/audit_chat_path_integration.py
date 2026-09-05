#!/usr/bin/env python3
"""Static contract audit for the current Gloss chat path.

This is intentionally a source-structure audit, not a historical-name check.
Runtime behavior remains owned by Rust and Vitest tests; this gate protects the
minimum wiring those tests depend on: replay-backed terminal emission, typed
retrieval disclosure, and frontend terminal-state ownership.
"""

from __future__ import annotations

import json
import pathlib
import sys


REQUIRED_FILES = {
    "chat": "src-tauri/src/commands/chat/mod.rs",
    "emit": "src-tauri/src/commands/chat/emit.rs",
    "chat_types": "src-tauri/src/commands/chat/types.rs",
    "store": "src/stores/chatStore.ts",
    "web_types": "src/lib/types.ts",
    "tauri": "src/lib/tauri.ts",
}


def read_sources(root: pathlib.Path) -> tuple[dict[str, str], list[str]]:
    sources: dict[str, str] = {}
    errors: list[str] = []
    for name, relative_path in REQUIRED_FILES.items():
        path = root / relative_path
        if not path.is_file():
            errors.append(f"required chat-path source missing: {relative_path}")
            continue
        sources[name] = path.read_text(encoding="utf-8")
    return sources, errors


def require_contains(errors: list[str], text: str, marker: str, contract: str) -> None:
    if marker not in text:
        errors.append(f"{contract}: missing {marker!r}")


def stop_streaming_body(store: str) -> str:
    start = store.find("  stopStreaming: async")
    end = store.find("  attachAssistantEvidence:", start)
    return store[start:end] if start >= 0 and end > start else ""


def audit(root: pathlib.Path) -> list[str]:
    sources, errors = read_sources(root)
    if errors:
        return errors

    chat = sources["chat"]
    emit = sources["emit"]
    chat_types = sources["chat_types"]
    store = sources["store"]
    web_types = sources["web_types"]
    tauri = sources["tauri"]

    # Backend terminal events must be persisted for replay before webview emit.
    for marker in ("record_chat_stream_event", 'handle.emit("chat:stream_event"', "ChatTerminalGuard", "emit_cancelled"):
        require_contains(errors, emit, marker, "replay-backed terminal contract")
    require_contains(errors, chat, "ChatTerminalEmitter::new", "chat stream terminal ownership")
    require_contains(errors, chat, "StopChatResponseV1", "cancellation request acknowledgement")
    require_contains(errors, chat_types, "ChatCancellationRequestV1", "typed cancellation acknowledgement")
    require_contains(errors, chat_types, "cancellation_requested", "typed cancellation acknowledgement")

    # Retrieval disclosure describes requested and effective behavior, without
    # pinning a deleted backend class or fallback implementation spelling.
    for marker in (
        "ChatEvidenceDisclosure",
        "backend_requested",
        "backend_used",
        "fallback_used",
        "retrieval_capability_decision",
    ):
        require_contains(errors, chat_types, marker, "typed retrieval disclosure")
    for marker in ("retrieval_backend_requested", "retrieval_backend_used", "emit_chat_evidence"):
        require_contains(errors, chat, marker, "retrieval disclosure emission")
    for marker in ("ChatEvidenceDisclosure", "backend_requested", "backend_used", "fallback_used"):
        require_contains(errors, web_types, marker, "frontend retrieval disclosure type")

    # Submitted attempts remain backend-terminal-owned. Preparation has no
    # backend attempt and may be cancelled locally only under its exact owner
    # guard, with a synchronous return before the stop IPC can be reached.
    body = stop_streaming_body(store)
    if not body:
        errors.append("frontend cancellation ownership: stopStreaming body not found")
    else:
        require_contains(errors, body, "await api.stopChat", "frontend cancellation request")
        require_contains(errors, body, "phase: 'cancelling'", "frontend cancellation pending state")
        preparation_guard = "    if (requestedMessageId && get().preparingMessageId === requestedMessageId) {"
        start = body.find(preparation_guard)
        end = body.find("\n    }", start) if start >= 0 else -1
        if start >= 0 and end > start:
            preparation = body[start:end]
            if (preparation.rstrip().endswith("return;")
                    and "await " not in preparation and "api." not in preparation
                    and end < body.find("await api.stopChat")):
                body = body[:start] + body[end + len("\n    }"):]
            else:
                errors.append("frontend cancellation ownership: preparing cancellation must return synchronously before IPC")
        for forbidden in ("isStreaming: false", "streamingMessageId: null", "streamingNotebookId: null"):
            if forbidden in body:
                errors.append(f"frontend cancellation ownership: stopStreaming clears terminal state via {forbidden!r}")
    require_contains(errors, store, "handleChatCancelled", "frontend cancellation terminal handler")
    require_contains(errors, store, "get().handleChatCancelled", "frontend cancellation replay routing")
    # TypeScript permits either an inline import type or a named type import.
    # The contract is the exported typed response, not a particular import spelling.
    require_contains(errors, tauri, "Promise<StopChatResponseV1>", "typed stop_chat IPC response")
    require_contains(errors, tauri, "StopChatResponseV1", "typed stop_chat IPC response")

    return errors


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    errors = audit(root)
    result = {"errors": errors, "status": "pass" if not errors else "fail"}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
