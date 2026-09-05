"""Fail-closed setup contract; these tests are not downloaded-model proof."""
import importlib.util
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
spec = importlib.util.spec_from_file_location("live_ollama_canary", SCRIPTS / "live_ollama_canary.py")
canary = importlib.util.module_from_spec(spec)
spec.loader.exec_module(canary)


class LiveOllamaSetupTests(unittest.TestCase):
    def tags(self):
        return {"models": [{"name": name, "digest": pin["digest_prefix"] + "0" * 52,
                            "size": pin["max_bytes"]}
                           for name, pin in canary.MODELS.items()]}

    def test_absent_model_is_failure(self):
        with self.assertRaisesRegex(ValueError, "unavailable"):
            canary.validate_models({"models": []})

    def test_changed_or_oversized_model_is_failure(self):
        for field, value in [("digest", "0" * 64), ("size", 2_000_000_000), ("size", True)]:
            tags = self.tags()
            tags["models"][0][field] = value
            with self.assertRaises(ValueError):
                canary.validate_models(tags)

    def test_checksum_manifest_does_not_excuse_tampered_archive(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / canary.ASSET
            checksums = Path(directory) / "sha256sum.txt"
            archive.write_bytes(b"not the official runtime")
            checksums.write_text(f"{canary.ASSET_SHA256}  {canary.ASSET}\n")
            with self.assertRaisesRegex(ValueError, "archive SHA256 mismatch"):
                canary.validate_release(archive, checksums)

    def test_unsupported_runner_writes_failed_receipt_without_starting_process(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            output = root / "evidence"
            with patch.dict(canary.os.environ, {"GITHUB_ACTIONS": "false"}), \
                 patch.object(canary, "capture_source_identity", return_value={"source_sha256": "fixture"}), \
                 patch.object(canary.subprocess, "Popen") as start:
                self.assertEqual(canary.execute(root, output), 1)
                start.assert_not_called()
            receipt = json.loads((output / "receipt.json").read_text())
            self.assertEqual(receipt["status"], "fail")
            self.assertFalse(receipt["live_service_exercised"])
            self.assertEqual(receipt["commands"], [])

    def test_desktop_without_display_or_dbus_fails_before_runtime_download(self):
        for missing in ("DISPLAY", "DBUS_SESSION_BUS_ADDRESS"):
            with self.subTest(missing=missing), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                output = root / "evidence"
                env = {"GITHUB_ACTIONS": "true", "RUNNER_ENVIRONMENT": "github-hosted",
                       "DISPLAY": ":99", "DBUS_SESSION_BUS_ADDRESS": "unix:path=/synthetic-bus"}
                del env[missing]
                with patch.dict(canary.os.environ, env, clear=True), \
                     patch.object(canary.platform, "system", return_value="Linux"), \
                     patch.object(canary.platform, "machine", return_value="x86_64"), \
                     patch.object(canary, "capture_source_identity", return_value={"source_sha256": "fixture"}), \
                     patch.object(canary.subprocess, "Popen") as start:
                    self.assertEqual(canary.execute(root, output, desktop=True), 1)
                    start.assert_not_called()
                receipt = json.loads((output / "receipt.json").read_text())
                self.assertEqual(receipt["desktop_status"], "fail")
                self.assertFalse(receipt["live_service_exercised"])
                self.assertEqual(receipt["commands"], [])

    def test_desktop_receipt_from_different_source_is_rejected(self):
        from gloss_desktop_smoke_harness import LIVE_SCHEMA
        configuration = canary.desktop_configuration(canary.validate_models(self.tags()))
        child = {"schema": LIVE_SCHEMA, "source": {"source_sha256": "older"},
                 "ollama_config": configuration, "integrated_status": "pass"}
        with self.assertRaisesRegex(ValueError, "current source"):
            canary.validate_desktop_receipt(child, {"source_sha256": "current"}, configuration)

    def test_desktop_receipt_from_different_runtime_configuration_is_rejected(self):
        from gloss_desktop_smoke_harness import LIVE_SCHEMA
        configuration = canary.desktop_configuration(canary.validate_models(self.tags()))
        source = {"source_sha256": "current"}
        for field, value in (("base_url", "http://127.0.0.1:11434"),
                             ("chat_model_digest", "a" * 64),
                             ("embedding_model", "some-other-model")):
            with self.subTest(field=field):
                child = {"schema": LIVE_SCHEMA, "source": source, "integrated_status": "pass",
                         "ollama_config": {**configuration, field: value}}
                with self.assertRaisesRegex(ValueError, "owned Ollama configuration"):
                    canary.validate_desktop_receipt(child, source, configuration)

    def test_desktop_acceptance_uses_canonical_integrated_gate_without_promoting_release_status(self):
        from gloss_desktop_smoke_harness import LIVE_SCHEMA, REQUIRED_LIVE_CASES
        configuration = canary.desktop_configuration(canary.validate_models(self.tags()))
        source = {"schema": "GlossSourceSnapshotV1", "revision": "a" * 40,
                  "tree_sha": "b" * 40, "source_sha256": "c" * 64, "worktree_clean": True}
        cases = [{"id": name, "status": "pass"} for name in REQUIRED_LIVE_CASES]
        child = {"schema": LIVE_SCHEMA, "source": source, "ollama_config": configuration,
                 "live_desktop_exercised": True, "integrated_status": "pass",
                 "source_scope_widened": False, "hidden_fallback": False, "raw_uuid_flood": False,
                 "build": {"source": source, "exit_code": 0, "command": ["fixture-build"],
                           "binary": {"path": "gloss", "sha256": "d" * 64},
                           "log": {"path": "build.log", "sha256": "e" * 64}},
                 "status": "blocked", "cases": cases}
        canary.validate_desktop_receipt(child, source, configuration)
        self.assertEqual(child["status"], "blocked")
        for changed in ({"cases": cases[:2]}, {"cases": cases + [cases[0]]},
                        {"live_desktop_exercised": False}, {"status": "fail"},
                        {"integrated_status": "blocked"}, {"build": {}},
                        {"hidden_fallback": True}, {"source_scope_widened": None},
                        {"raw_uuid_flood": True}):
            with self.subTest(changed=changed), self.assertRaisesRegex(ValueError, "did not pass"):
                canary.validate_desktop_receipt({**child, **changed}, source, configuration)

    @unittest.skipUnless(sys.platform == "linux", "the hosted canary uses Linux process groups")
    def test_timeout_cleanup_kills_resistant_descendant_and_preserves_separate_group(self):
        # Run the fixture in a child subreaper so its intentionally orphaned
        # descendant can be joined without changing the test runner process.
        controller = r'''
import ctypes, json, os, signal, subprocess, sys, time
sys.path.insert(0, sys.argv[1])
from live_ollama_canary import terminate_group
assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0  # PR_SET_CHILD_SUBREAPER
descendant_code = "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print('ready',flush=True); time.sleep(60)"
parent_code = "import subprocess,sys,time; child=subprocess.Popen([sys.executable,'-c',sys.argv[1]],stdout=subprocess.PIPE,text=True); assert child.stdout.readline().strip()=='ready'; print(child.pid,flush=True); time.sleep(60)"
parent = subprocess.Popen([sys.executable, '-c', parent_code, descendant_code],
                          stdout=subprocess.PIPE, text=True, start_new_session=True)
separate = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'],
                            start_new_session=True)
descendant = None
joined = False
try:
    descendant = int(parent.stdout.readline())
    try:
        parent.wait(timeout=0.05)
        raise AssertionError('fixture command did not time out')
    except subprocess.TimeoutExpired:
        terminate_group(parent)
    assert parent.returncode == -signal.SIGTERM, parent.returncode
    deadline = time.monotonic() + 2
    while time.monotonic() < deadline:
        pid, status = os.waitpid(descendant, os.WNOHANG)
        if pid:
            joined = True
            assert os.WIFSIGNALED(status) and os.WTERMSIG(status) == signal.SIGKILL, status
            break
        time.sleep(0.01)
    assert joined, 'owned descendant survived the command timeout cleanup'
    assert separate.poll() is None, 'separate service group was terminated'
    print(json.dumps({'parent_joined': True, 'descendant_joined': True, 'separate_group_alive': True}))
finally:
    for process in (parent, separate):
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait(timeout=5)
    if descendant is not None and not joined:
        os.waitpid(descendant, 0)
'''
        result = subprocess.run([sys.executable, "-c", controller, str(SCRIPTS)],
                                capture_output=True, text=True, timeout=20, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout), {"parent_joined": True,
                         "descendant_joined": True, "separate_group_alive": True})


if __name__ == "__main__":
    unittest.main()
