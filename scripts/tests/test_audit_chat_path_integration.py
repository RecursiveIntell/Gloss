"""Contract tests for the current chat-path integration audit."""

from __future__ import annotations

import importlib.util
import shutil
import tempfile
import unittest
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
AUDIT_PATH = REPO_ROOT / "scripts" / "audit_chat_path_integration.py"


def load_audit_module():
    spec = importlib.util.spec_from_file_location("audit_chat_path_integration", AUDIT_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load chat-path integration audit")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class ChatPathIntegrationAuditTests(unittest.TestCase):
    def make_repo_copy(self) -> Path:
        root = Path(tempfile.mkdtemp())
        for relative_path in load_audit_module().REQUIRED_FILES.values():
            source = REPO_ROOT / relative_path
            destination = root / relative_path
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source, destination)
        return root

    def test_current_source_satisfies_current_contract(self) -> None:
        self.assertEqual(load_audit_module().audit(REPO_ROOT), [])

    def test_rejects_frontend_terminal_cleanup_on_request_acknowledgement(self) -> None:
        audit = load_audit_module()
        root = self.make_repo_copy()
        store = root / "src/stores/chatStore.ts"
        body = store.read_text(encoding="utf-8")
        body = body.replace(
            "phase: 'cancelling',",
            "isStreaming: false,\n          phase: 'cancelling',",
            1,
        )
        store.write_text(body, encoding="utf-8")

        errors = audit.audit(root)
        self.assertTrue(any("clears terminal state" in error for error in errors), errors)

    def test_rejects_removal_of_replay_persistence(self) -> None:
        audit = load_audit_module()
        root = self.make_repo_copy()
        emitter = root / "src-tauri/src/commands/chat/emit.rs"
        emitter.write_text(
            emitter.read_text(encoding="utf-8").replace("record_chat_stream_event", "record_removed_event", 1),
            encoding="utf-8",
        )

        errors = audit.audit(root)
        self.assertTrue(any("replay-backed terminal contract" in error for error in errors), errors)

    def test_rejects_preparing_cleanup_without_exact_owner_guard(self) -> None:
        audit = load_audit_module()
        root = self.make_repo_copy()
        store = root / "src/stores/chatStore.ts"
        store.write_text(store.read_text(encoding="utf-8").replace(
            "requestedMessageId && get().preparingMessageId === requestedMessageId",
            "requestedMessageId", 1,
        ), encoding="utf-8")
        errors = audit.audit(root)
        self.assertTrue(any("clears terminal state" in error for error in errors), errors)

    def test_rejects_preparing_cleanup_that_can_fall_through_to_ipc(self) -> None:
        audit = load_audit_module()
        root = self.make_repo_copy()
        store = root / "src/stores/chatStore.ts"
        store.write_text(store.read_text(encoding="utf-8").replace(
            "      return;\n    }\n    try {\n      await api.stopChat",
            "    }\n    try {\n      await api.stopChat", 1,
        ), encoding="utf-8")
        errors = audit.audit(root)
        self.assertTrue(any("must return synchronously" in error for error in errors), errors)


if __name__ == "__main__":
    unittest.main()
