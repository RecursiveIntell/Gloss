"""Focused contract tests for the source-derived Tauri IPC validator."""

from __future__ import annotations

import importlib.util
import json
import tempfile
import unittest
from pathlib import Path


def load_validator():
    path = Path(__file__).resolve().parents[1] / "verify_tauri_contract.py"
    spec = importlib.util.spec_from_file_location("verify_tauri_contract", path)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load Tauri contract validator")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class TauriContractValidatorTests(unittest.TestCase):
    def make_repo(self) -> Path:
        root = Path(tempfile.mkdtemp())
        (root / "schemas").mkdir()
        (root / "src" / "lib").mkdir(parents=True)
        (root / "src" / "stores").mkdir(parents=True)
        (root / "src-tauri" / "src").mkdir(parents=True)

        (root / "src-tauri" / "src" / "lib.rs").write_text(
            """\
            fn run() {
                tauri::generate_handler![commands::ping]
            }
            """,
            encoding="utf-8",
        )
        (root / "src-tauri" / "src" / "commands.rs").write_text(
            """\
            #[tauri::command]
            pub async fn ping(notebook_id: String) -> Result<String, String> { todo!() }
            """,
            encoding="utf-8",
        )
        (root / "src-tauri" / "src" / "events.rs").write_text(
            "handle.emit(\"chat:done\", payload);\n", encoding="utf-8"
        )
        (root / "src" / "lib" / "tauri.ts").write_text(
            """\
            export async function ping(notebookId: string): Promise<string> {
              return invoke("ping", { notebookId });
            }
            """,
            encoding="utf-8",
        )
        (root / "src" / "lib" / "events.ts").write_text(
            "listen<ChatDonePayload>(\"chat:done\", callback);\n", encoding="utf-8"
        )
        (root / "src" / "stores" / "client.ts").write_text(
            "import { ping } from '../lib/tauri';\nping('nb');\n", encoding="utf-8"
        )
        contract = {
            "version": "tauri-contract-v1",
            "commands": [
                {
                    "name": "ping",
                    "registered": True,
                    "definition": "src-tauri/src/commands.rs",
                    "wrapper": "ping",
                    "caller_count": 1,
                    "operator_only": False,
                    "request": {"casing": "camelCase", "fields": ["notebookId"]},
                    "response_family": "string",
                    "error_family": "String",
                }
            ],
            "events": [
                {
                    "name": "chat:done",
                    "emitters": ["src-tauri/src/events.rs"],
                    "listeners": ["src/lib/events.ts"],
                    "payload_schema_ref": "ChatDonePayload",
                    "sequence_scope": "per-chat-attempt",
                    "terminal_status": "terminal",
                }
            ],
        }
        (root / "schemas" / "tauri-contract-v1.json").write_text(
            json.dumps(contract), encoding="utf-8"
        )
        return root

    def test_accepts_source_derived_contract(self) -> None:
        validator = load_validator()
        self.assertEqual(validator.verify_contract(self.make_repo()), [])

    def test_rejects_request_casing_or_field_drift(self) -> None:
        validator = load_validator()
        root = self.make_repo()
        wrapper = root / "src" / "lib" / "tauri.ts"
        wrapper.write_text(
            wrapper.read_text(encoding="utf-8").replace("notebookId", "notebook_id"),
            encoding="utf-8",
        )
        self.assertTrue(any("request" in failure for failure in validator.verify_contract(root)))

    def test_rejects_uninventoried_frontend_invoke(self) -> None:
        validator = load_validator()
        root = self.make_repo()
        wrapper = root / "src" / "lib" / "tauri.ts"
        wrapper.write_text(
            wrapper.read_text(encoding="utf-8")
            + '\nexport async function extra(): Promise<void> { return invoke("extra"); }\n',
            encoding="utf-8",
        )
        self.assertTrue(any("frontend invokes" in failure for failure in validator.verify_contract(root)))

    def test_rejects_event_listener_drift(self) -> None:
        validator = load_validator()
        root = self.make_repo()
        events = root / "src" / "lib" / "events.ts"
        events.write_text(
            events.read_text(encoding="utf-8").replace("chat:done", "chat:missing"),
            encoding="utf-8",
        )
        self.assertTrue(any("event emit/listen names" in failure for failure in validator.verify_contract(root)))


if __name__ == "__main__":
    unittest.main()
