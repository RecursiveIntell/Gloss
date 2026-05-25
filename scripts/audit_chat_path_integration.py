#!/usr/bin/env python3
import json
import pathlib
import sys


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    chat = (root / "src-tauri/src/commands/chat/mod.rs").read_text()
    store = (root / "src/stores/chatStore.ts").read_text()
    types = (root / "src/lib/types.ts").read_text()
    errors = []

    required_chat_markers = [
        "GlossLocalMemoryBackend::new",
        "MemorySearchRequest",
        "allow_fallback: true",
        "raw-content-text-fallback",
        "semantic-memory-empty-context",
        "chat:evidence",
        "AssistantMessageEvidence",
        "backend_requested",
        "backend_used",
        "retrieval_mode",
        "fallback_used",
        "fallback_reason",
        "degradation_markers",
        "receipt_id",
    ]
    for marker in required_chat_markers:
        if marker not in chat:
            errors.append(f"chat.rs missing marker: {marker}")

    local_pos = chat.find("GlossLocalMemoryBackend::new")
    raw_pos = chat.find("raw-content-text-fallback")
    if local_pos < 0 or raw_pos < 0 or local_pos > raw_pos:
        errors.append("ranked gloss-local retrieval must appear before raw content_text fallback")

    if "get_chunks_for_source" in chat:
        errors.append("chat.rs should not use direct source-order get_chunks_for_source fallback")
    if "chunk_id: None" in chat[chat.find("MEMORY_BACKEND_SEMANTIC_MEMORY_PREVIEW"):chat.find("Err(err) if semantic_fallback_allowed")]:
        errors.append("semantic preview context should preserve exact candidate chunk_id")
    if "let mut retrieval_backend_used" not in chat or "backend_used: retrieval_backend_used" not in chat:
        errors.append("chat evidence must track actual backend_used separately from requested backend")
    if "semantic-memory-empty-context-fallback" not in chat:
        errors.append("empty semantic preview results must explicitly fall back or degrade")

    for marker in ["pendingEvidence", "attachAssistantEvidence", "citations: pendingEvidence"]:
        if marker not in store:
            errors.append(f"chatStore.ts missing evidence buffering marker: {marker}")

    for marker in ["ChatEvidenceDisclosure", "retrieval_mode", "ChatEvidencePayload"]:
        if marker not in types:
            errors.append(f"types.ts missing evidence type marker: {marker}")

    result = {"errors": errors, "status": "pass" if not errors else "fail"}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
