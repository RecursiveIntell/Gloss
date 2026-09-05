"""Receipt validation negatives. Synthetic fixtures do not prove live desktop behavior."""
from __future__ import annotations

import base64
import copy
import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("desktop_smoke", Path(__file__).resolve().parents[1] / "gloss_desktop_smoke_harness.py")
smoke = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(smoke)
SOURCE = {"schema": "GlossSourceSnapshotV1", "revision": "a" * 40, "tree_sha": "b" * 40,
          "worktree_clean": True, "source_sha256": "c" * 64}
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/a9sAAAAASUVORK5CYII=")


class DesktopReceiptTests(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        self.path = self.root / "receipt.json"
        self.data = {"schema": smoke.LIVE_SCHEMA, "status": "pass", "run_id": "fixture-only",
                     "runtime": "native_tauri", "live_desktop_exercised": True,
                     "isolated_data_root": str(self.root / "profile"), "source": copy.deepcopy(SOURCE),
                     "started_at": "2026-01-01T10:00:00+00:00", "finished_at": "2026-01-01T10:01:00+00:00",
                     "source_scope_widened": False, "hidden_fallback": False, "raw_uuid_flood": False}
        def artifact(name, kind, content):
            path = self.root / name
            path.write_bytes(content)
            return {"path": name, "kind": kind, "sha256": smoke.file_sha256(path)}
        self.data["build"] = {"source": copy.deepcopy(SOURCE), "exit_code": 0, "command": ["fixture-build"],
                              "log": artifact("build.log", "build_log", b"fixture build succeeded\n"),
                              "binary": artifact("binary", "executable", b"fixture executable")}
        self.data["cases"] = [{"id": case_id, "status": "pass", "observation": "Synthetic validator fixture only",
                               "evidence": [artifact(case_id + ".log", "runtime_log", b"fixture observation\n"),
                                            artifact(case_id + ".png", "screenshot", PNG)]}
                              for case_id in smoke.REQUIRED_LIVE_CASES]

    def validate(self, source=SOURCE):
        self.path.write_text(json.dumps(self.data))
        with patch.object(smoke, "_source_identity", return_value=source):
            return smoke._validate_live_receipt(self.path, self.root)

    def assert_rejected(self, expected):
        passed, failures, _ = self.validate()
        self.assertFalse(passed)
        self.assertTrue(any(expected in failure for failure in failures), failures)

    def test_complete_fixture_validates_contract_only(self):
        passed, failures, _ = self.validate()
        self.assertTrue(passed, failures)

    def test_legacy_boolean_only_receipt_cannot_certify_current_source(self):
        self.data = {"status": "pass", "live_desktop_exercised": True}
        self.assert_rejected("schema")

    def test_missing_or_string_safety_flags_fail_closed(self):
        for flag in smoke.SAFETY_FLAGS:
            with self.subTest(flag=flag):
                self.data[flag] = "false"
                self.assert_rejected(flag)
                self.data[flag] = False
        del self.data["raw_uuid_flood"]
        self.assert_rejected("raw_uuid_flood")

    def test_stale_commit_and_source_digest_are_rejected(self):
        self.data["source"]["revision"] = "d" * 40
        self.assert_rejected("current source")
        self.data["source"] = copy.deepcopy(SOURCE)
        self.data["source"]["source_sha256"] = "e" * 64
        self.assert_rejected("current source")

    def test_dirty_source_cannot_be_release_certified(self):
        dirty = dict(SOURCE, worktree_clean=False)
        self.data["source"] = dirty
        passed, failures, _ = self.validate(dirty)
        self.assertFalse(passed)
        self.assertTrue(any("clean source" in failure for failure in failures))

    def test_missing_duplicate_and_blocked_cases_fail(self):
        removed = self.data["cases"].pop()
        self.assert_rejected("required live desktop case missing")
        self.data["cases"].append(removed)
        self.data["cases"].append(copy.deepcopy(removed))
        self.assert_rejected("duplicate")
        self.data["cases"].pop()
        self.data["cases"][0]["status"] = "blocked"
        self.assert_rejected("did not pass")

    def test_pass_flags_without_artifacts_or_observations_fail(self):
        self.data["cases"][0]["evidence"] = []
        self.data["cases"][0]["observation"] = ""
        self.assert_rejected("evidence missing")

    def test_modified_missing_and_empty_artifacts_fail(self):
        artifact = self.root / self.data["cases"][0]["evidence"][0]["path"]
        artifact.write_text("changed bytes")
        self.assert_rejected("digest mismatch")
        artifact.write_text("")
        self.assert_rejected("empty")
        artifact.unlink()
        self.assert_rejected("regular file")

    def test_evidence_cannot_escape_receipt_directory(self):
        self.data["cases"][0]["evidence"][0]["path"] = "../outside.log"
        self.assert_rejected("escapes")

    def test_failed_or_other_source_build_fails(self):
        self.data["build"]["exit_code"] = False
        self.assert_rejected("build must have succeeded")
        self.data["build"]["exit_code"] = 0
        self.data["build"]["source"] = {}
        self.assert_rejected("same source")

    def test_mock_runtime_and_future_receipt_fail(self):
        self.data["runtime"] = "mock_tauri"
        self.assert_rejected("native_tauri")
        self.data["runtime"] = "native_tauri"
        self.data["finished_at"] = "2999-01-01T00:00:00+00:00"
        self.assert_rejected("completed timezone-aware")

    def test_non_object_json_fails_without_crashing(self):
        self.data = []
        self.assert_rejected("must be an object")

    def test_malformed_evidence_kind_fails_without_crashing(self):
        self.data["cases"][0]["evidence"][0]["kind"] = {}
        self.assert_rejected("needs runtime_log")

    def test_shared_source_helper_imports_without_ambient_pythonpath(self):
        source = smoke._source_identity(Path(__file__).resolve().parents[2])
        self.assertEqual(source["schema"], "GlossSourceSnapshotV1")
        self.assertIn("source_sha256", source)


if __name__ == "__main__":
    unittest.main()
