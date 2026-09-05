"""Negative contract tests for the actual package owner; no GUI success mocked."""
import copy
from contextlib import redirect_stdout
import importlib.util
import io
import os
from pathlib import Path
import sys
import signal
import tempfile
import time
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("installer_gate", ROOT / "validation/gloss_installer_smoke_gate.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


def executable(path, content=b"\x7fELFfixture"):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(content)
    path.chmod(0o755)
    return path


class InstallerSmokeContract(unittest.TestCase):
    def test_large_non_elf_does_not_pass(self):
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "fake.AppImage"
            with path.open("wb") as stream:
                stream.truncate(11_000_000)
            path.chmod(0o755)
            with self.assertRaisesRegex(ValueError, "Not an ELF"):
                gate.require_elf(path)

    def test_old_or_ambiguous_artifacts_are_rejected(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            old = executable(root / "old.AppImage")
            os.utime(old, ns=(1, 1))
            started = time.time_ns()
            with self.assertRaisesRegex(ValueError, "found 0"):
                gate.select_fresh_artifact(root, started)
            fresh = executable(root / "new.AppImage")
            self.assertEqual(gate.select_fresh_artifact(root, started), fresh)
            executable(root / "ambiguous.AppImage")
            with self.assertRaisesRegex(ValueError, "found 2"):
                gate.select_fresh_artifact(root, started)

    def payload(self, root):
        executable(root / "AppRun", b"#!/bin/sh\nexec ./usr/bin/gloss\n")
        executable(root / "usr/bin/gloss")
        (root / "Gloss.desktop").write_text("[Desktop Entry]\nName=Gloss\nType=Application\nExec=gloss\nIcon=gloss\n")
        (root / "gloss.png").write_bytes(b"fixture-icon")

    def test_payload_requires_its_launcher_binary_and_desktop(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.payload(root)
            proof = gate.validate_payload(root, "Gloss")
            self.assertEqual(proof["binary_sha256"], gate.file_sha256(root / "usr/bin/gloss"))
            (root / "AppRun").unlink()
            with self.assertRaisesRegex(ValueError, "AppRun"):
                gate.validate_payload(root, "Gloss")

    def test_payload_rejects_escaping_links_and_wrong_launch_target(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            self.payload(root)
            (root / "escape").symlink_to("/etc/passwd")
            with self.assertRaisesRegex(ValueError, "escapes"):
                gate.validate_payload(root, "Gloss")
            (root / "escape").unlink()
            desktop = root / "Gloss.desktop"
            desktop.write_text(desktop.read_text().replace("Exec=gloss", "Exec=other"))
            with self.assertRaisesRegex(ValueError, "does not launch"):
                gate.validate_payload(root, "Gloss")

    def test_package_requires_observed_baseline_and_same_source_artifact(self):
        source = {"worktree_clean": True, "source_sha256": "source"}
        receipt = {"status": "blocked", "live_desktop_exercised": True,
                   "baseline_status": "pass", "source": source,
                   "prebuilt_config": {"artifact_sha256": "archive"},
                   "cases": [{"id": name, "status": "pass"}
                             for name in ("startup_idle", "notebook_crud_restart")]}
        gate.require_packaged_baseline(receipt, source, "archive")
        for mutated in [dict(receipt, live_desktop_exercised=False),
                        dict(receipt, source={}), dict(receipt, prebuilt_config={})]:
            with self.assertRaises(ValueError):
                gate.require_packaged_baseline(mutated, source, "archive")
        missing = copy.deepcopy(receipt)
        missing["cases"][1]["status"] = "blocked"
        with self.assertRaises(ValueError):
            gate.require_packaged_baseline(missing, source, "archive")

    def test_timeout_is_a_failed_command(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            result = gate.run_command([sys.executable, "-c", "import time; time.sleep(30)"], root, root / "timeout.log", 1)
            self.assertTrue(result["timed_out"])
            self.assertEqual(result["status"], "fail")
            self.assertIsNone(result["exit_code"])

    @unittest.skipUnless(hasattr(os, "fork"), "Unix process-group regression")
    def test_timeout_kills_term_ignoring_descendant_after_parent_exits(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            script = root / "owned-child.py"
            script.write_text("""import os, signal, time
from pathlib import Path
pid = os.fork()
if pid == 0:
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
    signal.signal(signal.SIGUSR1, lambda *_: Path('child-ack').write_text('alive'))
    Path('child-pid').write_text(str(os.getpid()))
    while True:
        signal.pause()
while True:
    time.sleep(60)
""")
            child = None
            try:
                result = gate.run_command([sys.executable, str(script)], root, root / "child.log", 2)
                child = int((root / "child-pid").read_text())
                self.assertTrue(result["timed_out"])
                self.assertEqual(result["status"], "fail")
                try:
                    os.kill(child, signal.SIGUSR1)
                except ProcessLookupError:
                    pass
                deadline = time.monotonic() + 0.3
                while time.monotonic() < deadline and not (root / "child-ack").exists():
                    time.sleep(0.01)
                self.assertFalse((root / "child-ack").exists(), "timed-out descendant remained runnable")
            finally:
                if child is not None:
                    try:
                        os.kill(child, signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    def test_failed_command_emits_bounded_tail_and_retains_complete_log(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            log = root / "failed.log"
            console = io.StringIO()
            with redirect_stdout(console):
                result = gate.run_command([sys.executable, "-c", "import sys; print('omitted-prefix-' + 'x' * 40000 + '-last-marker'); sys.exit(7)"], root, log, 5)
            self.assertEqual(result["exit_code"], 7)
            rendered = console.getvalue()
            self.assertIn("last-marker", rendered)
            self.assertNotIn("omitted-prefix", rendered)
            self.assertLessEqual(len(rendered.encode()), 32 * 1024 + 128)
            self.assertIn("omitted-prefix", log.read_text())

    def test_canonical_builder_requires_locked_tauri_and_has_no_manual_fallback(self):
        script = (ROOT / "scripts/build-appimage.sh").read_text()
        self.assertIn("npm exec -- tauri build --ci --no-sign --bundles appimage", script)
        self.assertIn("--features semantic-memory-turbo-quant -- --locked", script)
        self.assertNotIn("find /tmp", script)
        self.assertNotIn("manual_appimage", script)

    def test_historical_and_existing_receipts_are_not_written(self):
        with tempfile.TemporaryDirectory() as tmp:
            repo = Path(tmp)
            historical = repo / "docs/codex-runs/old/INSTALLER_SMOKE_RECEIPT.json"
            with patch.object(sys, "argv", ["gate", "--repo", str(repo), "--receipt", str(historical)]):
                self.assertEqual(gate.main(), 2)
            self.assertFalse(historical.exists())
            receipt = repo / "existing.json"
            receipt.write_text("retained evidence")
            with patch.object(sys, "argv", ["gate", "--repo", str(repo), "--receipt", str(receipt)]):
                self.assertEqual(gate.main(), 2)
            self.assertEqual(receipt.read_text(), "retained evidence")


if __name__ == "__main__":
    unittest.main()
