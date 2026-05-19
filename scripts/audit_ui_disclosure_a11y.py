#!/usr/bin/env python3
import json
import pathlib
import sys


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    chat_panel = (root / "src/components/chat/ChatPanel.tsx").read_text()
    store = (root / "src/stores/chatStore.ts").read_text()
    types = (root / "src/lib/types.ts").read_text()
    errors = []

    required_ui = [
        "Answer evidence",
        "Backend requested",
        "Backend used",
        "Retrieval",
        "Fallback",
        "Scope",
        "Context",
        "Citations",
        "Omitted",
        "Index",
        "Links",
        "Invalid scope IDs",
        "Requested source IDs",
        "Selected source IDs",
        "Excluded source IDs",
        "source_scope_preserved",
        "excluded_source_count",
        "Degraded:",
        "Receipt:",
        "aria-expanded",
        "aria-controls",
        "role=\"region\"",
        "aria-label=\"Answer evidence\"",
    ]
    for marker in required_ui:
        if marker not in chat_panel:
            errors.append(f"ChatPanel.tsx missing UI/a11y marker: {marker}")

    required_types = [
        "backend_requested",
        "backend_used",
        "retrieval_mode",
        "fallback_used",
        "fallback_reason",
        "degradation_markers",
        "invalid_source_ids",
        "selected_source_ids",
        "excluded_source_ids",
        "excluded_source_count",
        "context_passage_count",
        "source_scope_preserved",
        "citation_valid_count",
        "citation_invalid_count",
        "receipt_id",
    ]
    for marker in required_types:
        if marker not in types:
            errors.append(f"types.ts missing evidence type field: {marker}")

    for marker in ["pendingEvidence", "attachAssistantEvidence", "resetForNotebookSwitch", "citations: pendingEvidence"]:
        if marker not in store:
            errors.append(f"chatStore.ts missing state marker: {marker}")
    if "filter(([id]) => id !== streamingMessageId)" not in store:
        errors.append("chatStore.ts must clear pendingEvidence for stopped streaming message")

    result = {"errors": errors, "status": "pass" if not errors else "fail"}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
