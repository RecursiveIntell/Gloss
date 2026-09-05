"""Packaged evidence rejection tests; fixtures are never native/model proof."""
from __future__ import annotations

import copy
import importlib.util
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
import live_desktop_smoke as desktop
import live_ollama_canary as canary

SPEC = importlib.util.spec_from_file_location("packaged_installer_gate", ROOT / "validation/gloss_installer_smoke_gate.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


class PackagedWorkflowEvidenceTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.output = self.root / "workflow"
        self.native = self.output / "desktop"
        self.native.mkdir(parents=True)
        self.source = {"schema": "GlossSourceSnapshotV1", "revision": "a" * 40,
                       "tree_sha": "b" * 40, "source_sha256": "c" * 64, "worktree_clean": True}
        self.application = self.root / "AppRun"
        self.binary = self.root / "gloss"
        self.log = self.root / "package-build.log"
        self.application.write_bytes(b"fixture AppRun; not an executable")
        self.binary.write_bytes(b"fixture packaged binary")
        self.log.write_text("fixture build log; no build occurred\n")
        self.prebuilt = {"schema": "gloss-desktop-prebuilt/v1", "source": self.source.copy(),
                         "application": str(self.application), "application_sha256": canary.sha256(self.application),
                         "binary": str(self.binary), "binary_sha256": canary.sha256(self.binary),
                         "artifact_sha256": "d" * 64, "build_command": ["fixture-build"],
                         "build_log": str(self.log), "build_exit_code": 0}
        self.manifest = self.root / "prebuilt.json"
        self.manifest.write_text(json.dumps(self.prebuilt))
        self.models = {name: {"name": name, "digest": pin["digest_prefix"] + "0" * 52,
                              "size": pin["max_bytes"]} for name, pin in canary.MODELS.items()}
        self.configuration = canary.desktop_configuration(self.models)
        self.config_path = self.output / "desktop-ollama-config.json"
        self.config_path.write_text(json.dumps(self.configuration))
        self.child = {"schema": desktop.LIVE_SCHEMA, "status": "pass", "source": self.source.copy(),
                      "live_desktop_exercised": True, "integrated_status": "pass",
                      "ollama_config": copy.deepcopy(self.configuration), "prebuilt_config": copy.deepcopy(self.prebuilt),
                      "source_scope_widened": False, "hidden_fallback": False, "raw_uuid_flood": False,
                      "cases": [{"id": name, "status": "pass"} for name in desktop.REQUIRED_LIVE_CASES],
                      "build": {"source": self.source.copy(), "exit_code": 0,
                                "command": self.prebuilt["build_command"].copy(),
                                "artifact_sha256": self.prebuilt["artifact_sha256"]}}
        for field, name, original in (("binary", "gloss", self.binary),
                                       ("launcher", "packaged-AppRun", self.application),
                                       ("log", "build.log", self.log)):
            path = self.native / name
            path.write_bytes(original.read_bytes())
            self.child["build"][field] = {"path": name, "sha256": canary.sha256(path)}
        self.hosted = {"schema": canary.CANARY_SCHEMA, "status": "pass", "source": self.source.copy(),
                       "source_after": self.source.copy(), "prebuilt_config": copy.deepcopy(self.prebuilt),
                       "desktop_requested": True, "desktop_status": "pass", "live_service_exercised": True,
                       "models": copy.deepcopy(self.models)}
        self.write_receipts()

    def write_receipts(self):
        self.child_path = self.native / "LIVE_DESKTOP_SMOKE_RECEIPT.json"
        self.child_path.write_text(json.dumps(self.child))
        self.hosted["desktop_receipt"] = {
            "path": "desktop/LIVE_DESKTOP_SMOKE_RECEIPT.json", "sha256": canary.sha256(self.child_path),
            "integrated_status": self.child.get("integrated_status"),
            "full_acceptance_status": self.child.get("status")}
        (self.output / "receipt.json").write_text(json.dumps(self.hosted))

    def require(self):
        return gate.require_packaged_workflow(self.output, self.source, self.prebuilt)

    def test_complete_consistent_fixture_satisfies_evidence_contract_only(self):
        self.assertEqual(self.require(), self.child_path)

    def test_hosted_failure_or_source_build_runtime_mismatch_cannot_pass(self):
        original = copy.deepcopy(self.hosted)
        for change in ({"schema": "unknown"}, {"status": "fail"}, {"status": "blocked"},
                       {"source": {}}, {"source_after": {}}, {"prebuilt_config": {}},
                       {"desktop_requested": False}, {"desktop_status": "blocked"},
                       {"live_service_exercised": False}, {"models": {}}, {"models": {"bad": None}}):
            with self.subTest(change=change):
                self.hosted = {**copy.deepcopy(original), **change}
                self.write_receipts()
                with self.assertRaises(ValueError):
                    self.require()

    def test_wrong_models_or_coordinated_config_rewrite_cannot_change_owned_runtime(self):
        original_models = copy.deepcopy(self.models)
        for field, value in (("digest", "f" * 64), ("size", 8_000_000_000)):
            with self.subTest(field=field):
                self.hosted["models"] = copy.deepcopy(original_models)
                self.hosted["models"]["qwen3:0.6b"][field] = value
                self.write_receipts()
                with self.assertRaises(ValueError):
                    self.require()
        self.hosted["models"] = original_models
        self.child["ollama_config"]["base_url"] = "http://127.0.0.1:11434"
        self.config_path.write_text(json.dumps(self.child["ollama_config"]))
        self.write_receipts()
        with self.assertRaisesRegex(ValueError, "owned pinned models"):
            self.require()

    def test_baseline_missing_duplicate_failed_unknown_and_blocked_cases_do_not_pass(self):
        original = copy.deepcopy(self.child)
        for fault in ("baseline", "missing", "duplicate", "failed", "unknown", "blocked"):
            with self.subTest(fault=fault):
                self.child = copy.deepcopy(original)
                if fault == "baseline": self.child["cases"] = self.child["cases"][:2]
                elif fault == "missing": self.child["cases"].pop()
                elif fault == "duplicate": self.child["cases"].append(self.child["cases"][0].copy())
                elif fault == "unknown": self.child["cases"][3]["id"] = "unrelated"
                else: self.child["cases"][3]["status"] = fault
                self.write_receipts()
                with self.assertRaises(ValueError):
                    self.require()

    def test_packaged_completion_does_not_promote_blocked_or_failed_child(self):
        for status in ("blocked", "fail"):
            self.child["status"] = status
            self.write_receipts()
            with self.assertRaises(ValueError):
                self.require()

    def test_unsafe_or_unknown_observation_flags_do_not_pass(self):
        original = copy.deepcopy(self.child)
        for flag in ("source_scope_widened", "hidden_fallback", "raw_uuid_flood"):
            for value in (True, None, "false"):
                with self.subTest(flag=flag, value=value):
                    self.child = copy.deepcopy(original)
                    self.child[flag] = value
                    self.write_receipts()
                    with self.assertRaises(ValueError):
                        self.require()

    def test_child_build_source_package_launcher_binary_log_or_manifest_mismatch_rejected(self):
        original = copy.deepcopy(self.child)
        for fault in ("source", "prebuilt", "archive", "launcher", "binary", "log", "command", "build-failed"):
            with self.subTest(fault=fault):
                self.child = copy.deepcopy(original)
                if fault == "source": self.child["source"] = {}
                elif fault == "prebuilt": self.child["prebuilt_config"]["application"] = "/other/AppRun"
                elif fault == "archive": self.child["build"]["artifact_sha256"] = "f" * 64
                elif fault == "command": self.child["build"]["command"] = ["other-build"]
                elif fault == "build-failed": self.child["build"]["exit_code"] = 1
                else: self.child["build"][fault]["sha256"] = "f" * 64
                self.write_receipts()
                with self.assertRaises(ValueError):
                    self.require()

    def test_current_child_bytes_must_match_canary_descriptor(self):
        self.child_path.write_text(json.dumps(self.child, indent=2))
        with self.assertRaisesRegex(ValueError, "current native child"):
            self.require()

    def test_evidence_descriptor_cannot_select_another_child_path(self):
        self.hosted["desktop_receipt"]["path"] = "../other-receipt.json"
        (self.output / "receipt.json").write_text(json.dumps(self.hosted))
        with self.assertRaisesRegex(ValueError, "current native child"):
            self.require()

    def test_retained_binary_launcher_and_log_bytes_are_checked(self):
        for name in ("gloss", "packaged-AppRun", "build.log"):
            with self.subTest(name=name):
                path = self.native / name
                original = path.read_bytes()
                path.write_bytes(b"tampered retained evidence")
                with self.assertRaisesRegex(ValueError, "evidence changed"):
                    self.require()
                path.write_bytes(original)

    def test_retained_build_descriptor_cannot_select_another_file(self):
        self.child["build"]["launcher"]["path"] = "../../AppRun"
        self.write_receipts()
        with self.assertRaisesRegex(ValueError, "another file"):
            self.require()

    def test_prebuilt_without_desktop_is_rejected_without_starting_process(self):
        output = self.root / "invalid-mode"
        with patch.object(canary, "capture_source_identity", return_value=self.source), \
             patch.object(canary.subprocess, "Popen") as start:
            self.assertEqual(canary.execute(self.root, output, prebuilt_config=self.manifest), 1)
            start.assert_not_called()
        self.assertIn("requires --desktop", json.loads((output / "receipt.json").read_text())["error"])

    def test_stale_dirty_or_tampered_prebuilt_fails_before_any_download(self):
        env = {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted",
               "DISPLAY": ":99", "DBUS_SESSION_BUS_ADDRESS": "unix:path=/fixture"}
        for fault in ("stale", "dirty", "launcher", "binary"):
            with self.subTest(fault=fault):
                source = copy.deepcopy(self.source)
                if fault == "stale": source["revision"] = "e" * 40
                elif fault == "dirty": source["worktree_clean"] = False
                original = None
                if fault in ("launcher", "binary"):
                    path = self.application if fault == "launcher" else self.binary
                    original = path.read_bytes()
                    path.write_bytes(b"tampered package")
                output = self.root / f"preflight-{fault}"
                with patch.dict(canary.os.environ, env, clear=True), \
                     patch.object(canary.platform, "system", return_value="Linux"), \
                     patch.object(canary.platform, "machine", return_value="x86_64"), \
                     patch.object(canary, "capture_source_identity", return_value=source), \
                     patch.object(canary.subprocess, "Popen") as start:
                    self.assertEqual(canary.execute(self.root, output, desktop=True, prebuilt_config=self.manifest), 1)
                    start.assert_not_called()
                if original is not None:
                    path.write_bytes(original)
                receipt = json.loads((output / "receipt.json").read_text())
                self.assertEqual(receipt["status"], "fail")
                self.assertFalse(receipt["live_service_exercised"])
                self.assertEqual(receipt["commands"], [])


if __name__ == "__main__":
    unittest.main()
