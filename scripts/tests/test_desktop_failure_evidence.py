"""Temporary SQLite diagnostics tests; these are not native desktop observations."""
from __future__ import annotations

from contextlib import redirect_stdout
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import sqlite3
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import parse_qs, urlsplit
import uuid


REPO = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "desktop_failure_driver", REPO / "scripts" / "live_desktop_smoke.py"
)
driver = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(driver)

NOTEBOOK = "11111111-1111-4111-8111-111111111111"
CONVERSATION = "22222222-2222-4222-8222-222222222222"
OLD_CONVERSATION = "33333333-3333-4333-8333-333333333333"
QUERY = "Reply with exactly RETRY_GLOSS. /no_think"
USER_DIGEST = hashlib.sha256(QUERY.encode()).hexdigest()
SYSTEM = "Follow the current user request."
PROMPT_DIGEST = hashlib.sha256(json.dumps({
    "system": SYSTEM, "messages": [{"role": "user", "content": QUERY}],
    "model": "qwen3:0.6b", "num_ctx": 8192, "max_tokens": 1024,
}).encode()).hexdigest()

MESSAGES_SCHEMA = """
CREATE TABLE messages (
    id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL, role TEXT NOT NULL,
    content TEXT NOT NULL, citations TEXT, model_used TEXT,
    tokens_prompt INTEGER, tokens_response INTEGER,
    created_at TEXT DEFAULT (datetime('now'))
);
CREATE INDEX idx_messages_conversation ON messages(conversation_id, created_at);
"""
PROMPTS_SCHEMA = """
CREATE TABLE prompt_receipts (
    receipt_id TEXT PRIMARY KEY, notebook_id TEXT NOT NULL,
    conversation_id TEXT NOT NULL, message_id TEXT NOT NULL,
    prompt_digest TEXT NOT NULL, context_payload_digest TEXT NOT NULL,
    raw_receipt_json TEXT NOT NULL,
    recorded_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX idx_prompt_receipts_message ON prompt_receipts(message_id, recorded_at DESC);
"""


def prompt_receipt(message_id: str) -> dict:
    return {
        "schema": "PromptReceiptV1", "receipt_id": str(uuid.uuid4()),
        "notebook_id": NOTEBOOK, "conversation_id": CONVERSATION,
        "message_id": message_id, "prompt_digest": PROMPT_DIGEST,
        "context_payload_digest": hashlib.sha256(b"").hexdigest(),
        "capture_state": "captured_system_prompt",
        "redaction_state": "system_prompt_stored_other_content_digest_only",
        "system_prompt_digest": hashlib.sha256(SYSTEM.encode()).hexdigest(),
        "system_prompt_text": SYSTEM, "user_turn_digest": USER_DIGEST,
        "source_passage_count": 0, "recorded_at": "2026-09-05T00:00:05Z",
        "unrecognized_private_field": "DO_NOT_EXPORT_PRIVATE_RECEIPT_FIELD",
    }


def citations_payload(message_id: str) -> dict:
    return {"citations": [], "evidence": {
        "prompt_receipt": prompt_receipt(message_id),
        "decoding_settings_receipt": {
            "schema": "DecodingSettingsReceiptV1", "provider": "ollama",
            "model": "qwen3:0.6b", "requested": {"temperature": "0"},
            "effective": {"temperature": 0.0, "top_p": None, "top_k": None,
                          "min_p": None, "repeat_penalty": None, "max_tokens": 1024},
        },
        "prompt_budget_receipt": {
            "model_context_window": 8192, "system_prompt_chars": 300,
            "message_count": 3, "source_passage_count": 0,
            "prompt_digest": PROMPT_DIGEST, "context_budgeted": False,
            "estimated_prompt_tokens": 160,
        },
        "unrelated_secret": "DO_NOT_EXPORT_OTHER_EVIDENCE",
    }}


class DesktopFailureEvidenceTests(unittest.TestCase):
    def setUp(self):
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.temporary = Path(temporary.name)
        self.root = self.temporary / "evidence"

    def database(self, root: Path | None = None) -> Path:
        path = (root or self.root) / "profile/data/gloss/notebooks" / NOTEBOOK / "notebook.db"
        path.parent.mkdir(parents=True)
        with sqlite3.connect(path) as connection:
            connection.executescript(MESSAGES_SCHEMA)
        return path

    def insert(self, connection, role, content, *, second=1, conversation=CONVERSATION,
               citations=None, message_id=None):
        message_id = message_id or str(uuid.uuid4())
        connection.execute(
            "INSERT INTO messages (id,conversation_id,role,content,citations,model_used,created_at) "
            "VALUES (?,?,?,?,?,?,?)",
            (message_id, conversation, role, content, citations,
             "qwen3:0.6b" if role == "assistant" else None,
             f"2026-09-05 00:00:{second:02d}"),
        )
        return message_id

    def populated(self, root: Path | None = None):
        path = self.database(root)
        assistant_id = str(uuid.uuid4())
        payload = citations_payload(assistant_id)
        with sqlite3.connect(path) as connection:
            self.insert(connection, "user", "OLD_CONVERSATION_DO_NOT_EXPORT", second=0,
                        conversation=OLD_CONVERSATION)
            self.insert(connection, "user", "Reply with exactly HELLO_GLOSS.", second=1)
            self.insert(connection, "assistant", "HELLO_GLOSS", second=2)
            self.insert(connection, "user", "Write 100 ocean facts.", second=3)
            self.insert(connection, "user", QUERY, second=4)
            self.insert(connection, "assistant", "HELLO_GLOSS", second=5,
                        citations=json.dumps(payload), message_id=assistant_id)
        return path, assistant_id, payload

    def collect(self, root: Path | None = None):
        result = driver.collect_desktop_failure_evidence(root or self.root)
        self.assertIsInstance(result, dict)
        self.assertIn(result.get("status"), {"ok", "absent", "rejected", "error", "partial"})
        self.assertIsInstance(result.get("truncated"), bool)
        self.assertLessEqual(len(json.dumps(result).encode("utf-8")), 8192)
        return result

    def test_latest_conversation_preserves_rows_and_citations_request_receipts(self):
        _, assistant_id, _ = self.populated()
        result = self.collect()
        self.assertEqual(result["status"], "ok")
        self.assertFalse(result["truncated"])
        self.assertEqual(len(result["notebooks"]), 1)
        notebook = result["notebooks"][0]
        self.assertEqual(notebook["notebook_id"], NOTEBOOK)
        self.assertEqual(notebook["conversation_id"], CONVERSATION)
        self.assertEqual(notebook["message_count"], 5)
        self.assertFalse(notebook["messages_truncated"])
        self.assertEqual(len(notebook["messages"]), 5)
        for row in notebook["messages"]:
            self.assertIsInstance(row["rowid"], int)
            self.assertEqual(row["conversation_id"], CONVERSATION)
            self.assertIn(row["role"], {"user", "assistant"})
            self.assertTrue(row["id"])
            self.assertTrue(row["created_at"])
            self.assertFalse(row["content_truncated"])
        self.assertIn(QUERY, [row["content"] for row in notebook["messages"]])
        latest = notebook["latest_assistant"]
        self.assertEqual(latest["message_id"], assistant_id)
        self.assertEqual(latest["model"], "qwen3:0.6b")
        self.assertEqual(latest["prompt_receipt"]["user_turn_digest"], USER_DIGEST)
        self.assertEqual(latest["prompt_receipt"]["prompt_digest"], PROMPT_DIGEST)
        self.assertEqual(latest["decoding_settings_receipt"]["effective"]["temperature"], 0.0)
        self.assertEqual(latest["decoding_settings_receipt"]["effective"]["max_tokens"], 1024)
        self.assertEqual(latest["prompt_budget_receipt"]["message_count"], 3)
        for forbidden in ("OLD_CONVERSATION_DO_NOT_EXPORT", "DO_NOT_EXPORT_PRIVATE_RECEIPT_FIELD",
                          "DO_NOT_EXPORT_OTHER_EVIDENCE"):
            self.assertNotIn(forbidden, json.dumps(result))

    def test_prompt_table_is_canonical_and_missing_row_allows_citations_fallback(self):
        for has_row in (False, True):
            with self.subTest(has_row=has_row):
                root = self.temporary / f"table-{has_row}"
                path, assistant_id, _ = self.populated(root)
                canonical = prompt_receipt(assistant_id)
                canonical["prompt_digest"] = hashlib.sha256(b"canonical stored request").hexdigest()
                with sqlite3.connect(path) as connection:
                    connection.executescript(PROMPTS_SCHEMA)
                    if has_row:
                        connection.execute(
                            "INSERT INTO prompt_receipts (receipt_id,notebook_id,conversation_id,"
                            "message_id,prompt_digest,context_payload_digest,raw_receipt_json) "
                            "VALUES (?,?,?,?,?,?,?)",
                            (canonical["receipt_id"], NOTEBOOK, CONVERSATION, assistant_id,
                             canonical["prompt_digest"], canonical["context_payload_digest"],
                             json.dumps(canonical)),
                        )
                result = self.collect(root)
                saved = result["notebooks"][0]["latest_assistant"]["prompt_receipt"]
                self.assertEqual(saved["prompt_digest"],
                                 canonical["prompt_digest"] if has_row else PROMPT_DIGEST)
                self.assertEqual(saved["user_turn_digest"], USER_DIGEST)

    def test_database_is_opened_read_only_and_bytes_and_mtime_are_unchanged(self):
        path, _, _ = self.populated()
        before = (path.read_bytes(), path.stat().st_mtime_ns)
        real_connect = sqlite3.connect
        opened = []

        def observe_connect(database, *args, **kwargs):
            opened.append(str(database))
            self.assertTrue(kwargs.get("uri"), "SQLite URI handling must be explicitly enabled")
            self.assertEqual(parse_qs(urlsplit(str(database)).query).get("mode"), ["ro"])
            connection = real_connect(database, *args, **kwargs)
            try:
                with self.assertRaises(sqlite3.OperationalError):
                    connection.execute("CREATE TABLE mutation_probe (value TEXT)")
            except BaseException:
                connection.close()
                raise
            connection.set_authorizer(lambda action, table, *_rest:
                sqlite3.SQLITE_DENY if action == sqlite3.SQLITE_READ and table not in {
                    "messages", "prompt_receipts", "sqlite_master", "sqlite_schema"
                } else sqlite3.SQLITE_OK)
            return connection

        with patch.object(sqlite3, "connect", side_effect=observe_connect):
            result = self.collect()
        self.assertEqual(result["status"], "ok")
        self.assertTrue(opened)
        self.assertEqual((path.read_bytes(), path.stat().st_mtime_ns), before)
        self.assertEqual(sorted(item.name for item in path.parent.iterdir()), ["notebook.db"])

    def test_absent_and_noncanonical_locations_do_not_create_or_open_databases(self):
        absent = self.temporary / "does-not-exist"
        with patch.object(sqlite3, "connect") as connect:
            result = self.collect(absent)
            self.assertEqual(result["status"], "absent")
            connect.assert_not_called()
        self.assertFalse(absent.exists())
        path, _, _ = self.populated()
        invalid = path.parent.with_name("not-a-canonical-notebook")
        path.parent.rename(invalid)
        app_database = self.root / "profile/data/gloss/app.db"
        app_database.write_bytes(b"DO_NOT_OPEN_APP_DATABASE")
        before = sorted(str(item.relative_to(self.root)) for item in self.root.rglob("*"))
        with patch.object(sqlite3, "connect") as connect:
            result = self.collect()
            connect.assert_not_called()
        self.assertFalse(result["notebooks"])
        self.assertNotIn("DO_NOT_OPEN_APP_DATABASE", json.dumps(result))
        self.assertEqual(sorted(str(item.relative_to(self.root)) for item in self.root.rglob("*")), before)

    def test_live_uncheckpointed_wal_rows_are_visible_without_database_or_wal_writes(self):
        path = self.database()
        writer = sqlite3.connect(path)
        self.addCleanup(writer.close)
        self.assertEqual(writer.execute("PRAGMA journal_mode=WAL").fetchone()[0], "wal")
        writer.execute("PRAGMA wal_autocheckpoint=0")
        self.insert(writer, "user", QUERY, second=1)
        assistant_id = str(uuid.uuid4())
        self.insert(writer, "assistant", "RETRY_GLOSS", second=2, message_id=assistant_id,
                    citations=json.dumps(citations_payload(assistant_id)))
        writer.commit()
        wal = Path(str(path) + "-wal")
        self.assertTrue(wal.is_file())
        self.assertGreater(wal.stat().st_size, 0)
        disk_only = sqlite3.connect(path.as_uri() + "?mode=ro&immutable=1", uri=True)
        try:
            self.assertEqual(disk_only.execute("SELECT count(*) FROM messages").fetchone()[0], 0,
                             "The fixture must keep its new rows in WAL rather than the main file")
        finally:
            disk_only.close()
        before = {item: (item.read_bytes(), item.stat().st_mtime_ns) for item in (path, wal)}
        result = self.collect()
        self.assertEqual(result["status"], "ok")
        notebook = result["notebooks"][0]
        self.assertEqual(notebook["message_count"], 2)
        self.assertIn("RETRY_GLOSS", [row["content"] for row in notebook["messages"]])
        self.assertEqual(notebook["latest_assistant"]["message_id"], assistant_id)
        self.assertEqual(notebook["latest_assistant"]["prompt_receipt"]["user_turn_digest"], USER_DIGEST)
        self.assertEqual({item: (item.read_bytes(), item.stat().st_mtime_ns) for item in (path, wal)}, before)

    def test_corrupt_sqlite_becomes_diagnostic_error_without_changing_file(self):
        path = self.database()
        path.write_bytes(b"This is a corrupt SQLite fixture.")
        before = (path.read_bytes(), path.stat().st_mtime_ns)
        result = self.collect()
        self.assertIn(result["status"], {"error", "partial", "rejected"})
        self.assertIn("error", json.dumps(result).lower())
        self.assertEqual((path.read_bytes(), path.stat().st_mtime_ns), before)

    def test_malformed_receipts_retain_messages_and_report_failure_without_silent_replacement(self):
        for malformed in ("citations", "prompt_table"):
            with self.subTest(malformed=malformed):
                root = self.temporary / malformed
                path, assistant_id, _ = self.populated(root)
                with sqlite3.connect(path) as connection:
                    if malformed == "citations":
                        connection.execute("UPDATE messages SET citations=? WHERE id=?",
                                           ("{broken JSON", assistant_id))
                    else:
                        connection.executescript(PROMPTS_SCHEMA)
                        connection.execute(
                            "INSERT INTO prompt_receipts (receipt_id,notebook_id,conversation_id,"
                            "message_id,prompt_digest,context_payload_digest,raw_receipt_json) "
                            "VALUES (?,?,?,?,?,?,?)",
                            (str(uuid.uuid4()), NOTEBOOK, CONVERSATION, assistant_id,
                             PROMPT_DIGEST, "a" * 64, "{broken JSON"),
                        )
                result = self.collect(root)
                self.assertIn("error", json.dumps(result).lower())
                notebook = result["notebooks"][0]
                self.assertEqual(len(notebook["messages"]), 5)
                self.assertIn(QUERY, [row["content"] for row in notebook["messages"]])
                latest = notebook.get("latest_assistant") or {}
                saved = latest.get("prompt_receipt") or {}
                self.assertNotEqual(saved.get("prompt_digest"), PROMPT_DIGEST)

    def test_symlink_directories_database_and_wal_are_rejected_before_sqlite_open(self):
        for component in ("profile", "data", "gloss", "notebooks", "notebook", "database", "wal", "shm"):
            with self.subTest(component=component):
                root = self.temporary / f"symlink-{component}"
                path, _, _ = self.populated(root)
                targets = {
                    "profile": root / "profile", "data": root / "profile/data",
                    "gloss": root / "profile/data/gloss",
                    "notebooks": path.parent.parent, "notebook": path.parent,
                    "database": path, "wal": Path(str(path) + "-wal"),
                    "shm": Path(str(path) + "-shm"),
                }
                target = targets[component]
                outside = self.temporary / f"outside-{component}"
                if component in {"wal", "shm"}:
                    outside.write_bytes(b"OUTSIDE_PROFILE_FILE_MUST_NOT_BE_READ")
                    target.symlink_to(outside)
                else:
                    target.rename(outside)
                    target.symlink_to(outside, target_is_directory=outside.is_dir())
                with patch.object(sqlite3, "connect") as connect:
                    result = self.collect(root)
                    connect.assert_not_called()
                self.assertIn(result["status"], {"rejected", "error", "partial"})
                self.assertNotIn("OUTSIDE_PROFILE_FILE_MUST_NOT_BE_READ", json.dumps(result))

    def test_large_unicode_messages_and_receipts_have_explicit_bounded_truncation(self):
        path = self.database()
        assistant_id = str(uuid.uuid4())
        payload = citations_payload(assistant_id)
        payload["evidence"]["prompt_receipt"]["system_prompt_text"] = "🧪" * 20000
        with sqlite3.connect(path) as connection:
            for index in range(40):
                self.insert(connection, "user", "🧪" * 20000, second=index)
            self.insert(connection, "assistant", "🧪" * 20000, second=41,
                        message_id=assistant_id, citations=json.dumps(payload))
        result = self.collect()
        self.assertTrue(result["truncated"])
        notebook = result["notebooks"][0]
        self.assertEqual(notebook["message_count"], 41)
        self.assertTrue(notebook["messages_truncated"])
        self.assertLessEqual(len(notebook["messages"]), 16)
        self.assertTrue(any(row["content_truncated"] for row in notebook["messages"]))

    def test_main_preserves_original_error_when_failure_collector_also_raises(self):
        output = self.temporary / "main-failure"
        original_error = "original owned runtime configuration failure"
        diagnostic_error = "secondary failure-evidence collection failure"
        source = {"worktree_clean": True, "commit": "fixture-source"}
        arguments = ["live_desktop_smoke.py", "--repo", str(REPO), "--output", str(output),
                     "--ollama-config", str(self.temporary / "unused-config.json")]
        with patch.object(driver.sys, "argv", arguments), \
             patch.object(driver, "capture_source_identity", return_value=source), \
             patch.object(driver, "load_ollama_config", side_effect=ValueError(original_error)), \
             patch.object(driver, "collect_desktop_failure_evidence", side_effect=RuntimeError(diagnostic_error)) as collector, \
             patch.object(driver.subprocess, "Popen") as process, \
             redirect_stdout(io.StringIO()):
            exit_code = driver.main()
        self.assertEqual(exit_code, 1)
        process.assert_not_called()
        collector.assert_called_once_with(output)
        failure = json.loads((output / "failure.json").read_text())
        self.assertEqual(failure["error"], original_error)
        diagnostic = failure["owned_profile_evidence"]
        self.assertEqual(diagnostic["status"], "error")
        self.assertEqual(diagnostic["capture_error"], "RuntimeError")
        receipt = json.loads((output / "LIVE_DESKTOP_SMOKE_RECEIPT.json").read_text())
        self.assertEqual(receipt["status"], "fail")
        self.assertIn(original_error, receipt["blockers"])


if __name__ == "__main__":
    unittest.main()
