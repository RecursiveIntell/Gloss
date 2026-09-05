"""Real process cancellation fixtures, never AppImage or model evidence."""
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("nested_cleanup_installer", ROOT / "validation/gloss_installer_smoke_gate.py")
gate = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(gate)


@unittest.skipUnless(sys.platform == "linux", "owned canary sessions use Linux process groups")
class CanaryNestedCleanupTests(unittest.TestCase):
    def test_execute_cancellation_writes_failed_receipt_and_joins_its_active_child(self):
        controller = r'''
import json, pathlib, signal, subprocess, sys
from unittest.mock import patch
sys.path.insert(0, sys.argv[1])
import live_ollama_canary as canary
root = pathlib.Path(sys.argv[2])
source = {'worktree_clean': True, 'source_sha256': 'fixture-only'}
real_start = subprocess.Popen
created = []
def start(command, **kwargs):
    assert command[0] == 'curl', command
    # No download or model runtime is started: cancel this controlled command
    # while the real execute(), signal, wait, cleanup and receipt code runs.
    code = "import signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); time.sleep(60)"
    process = real_start([sys.executable, '-c', code], **kwargs)
    created.append(process)
    return process
env = {'GITHUB_ACTIONS': 'true', 'RUNNER_ENVIRONMENT': 'github-hosted',
       'DISPLAY': ':99', 'DBUS_SESSION_BUS_ADDRESS': 'unix:path=/fixture'}
with patch.dict(canary.os.environ, env), \
     patch.object(canary.platform, 'system', return_value='Linux'), \
     patch.object(canary.platform, 'machine', return_value='x86_64'), \
     patch.object(canary.platform, 'platform', return_value='fixture Linux metadata'), \
     patch.object(canary, 'capture_source_identity', return_value=source), \
     patch.object(canary.Path, 'home', return_value=root), \
     patch.object(canary.socket, 'socket'), \
     patch.object(canary.subprocess, 'Popen', side_effect=start):
    code = canary.execute(root, root / 'canary', desktop=True)
assert len(created) == 1
(root / 'execution.json').write_text(json.dumps({
    'exit_code': code, 'child_pid': created[0].pid,
    'child_returncode': created[0].returncode,
    'extraction_exists_during_cleanup': root.is_dir()}))
raise SystemExit(code)
'''
        with tempfile.TemporaryDirectory() as extraction:
            root = Path(extraction)
            result = gate.run_command([sys.executable, "-c", controller, str(ROOT / "scripts"), str(root)],
                                      root, root / "outer.log", 1, cooperative_cleanup=True)
            self.assertTrue(result["timed_out"])
            self.assertEqual(result["status"], "fail")
            execution = json.loads((root / "execution.json").read_text())
            self.assertEqual(execution["exit_code"], 1)
            self.assertEqual(execution["child_returncode"], -signal.SIGKILL)
            self.assertTrue(execution["extraction_exists_during_cleanup"])
            with self.assertRaises(ProcessLookupError):
                os.kill(execution["child_pid"], 0)
            receipt = json.loads((root / "canary/receipt.json").read_text())
            self.assertEqual(receipt["status"], "fail")
            self.assertEqual(receipt["desktop_status"], "fail")
            self.assertEqual(receipt["cancel_signal"], signal.SIGTERM)
            self.assertFalse(receipt["live_service_exercised"])
            self.assertEqual(receipt["commands"][0]["exit_code"], 128 + signal.SIGTERM)
            self.assertIn("cancelled", receipt["commands"][0]["error"])

    def test_outer_timeout_closes_actual_canary_owned_sessions_and_preserves_control(self):
        controller = r'''
import json, pathlib, signal, subprocess, sys
sys.path.insert(0, sys.argv[1])
from live_ollama_canary import CanaryCancelled, OwnedProcesses
root = pathlib.Path(sys.argv[2])
owned = OwnedProcesses()
owned.install()
service = command = None
cancelled = False
try:
    # Exercise escalation in the actual owner; this service ignores TERM.
    code = "import os,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); print(os.getpid(),flush=True); time.sleep(60)"
    service = owned.start([sys.executable, '-c', code], stdout=subprocess.PIPE, text=True)
    assert int(service.stdout.readline()) == service.pid
    command = owned.start([sys.executable, '-c', 'import time; time.sleep(60)'])
    (root / 'owned.json').write_text(json.dumps({'service': service.pid, 'command': command.pid}))
    try:
        owned.wait(command, 60)
    except CanaryCancelled:
        cancelled = True
    finally:
        owned.stop(command)
        owned.stop(service)
finally:
    owned.close()
    (root / 'cleanup.json').write_text(json.dumps({
        'cancelled': cancelled, 'service_returncode': service.returncode,
        'command_returncode': command.returncode, 'registry_empty': not owned.processes,
        'extraction_exists_during_cleanup': root.is_dir()}))
raise SystemExit(1 if cancelled else 0)
'''
        control = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(60)"], start_new_session=True)
        try:
            with tempfile.TemporaryDirectory() as extraction:
                root = Path(extraction)
                started = time.monotonic()
                result = gate.run_command([sys.executable, "-c", controller, str(ROOT / "scripts"), str(root)],
                                          root, root / "outer.log", 1, cooperative_cleanup=True)
                elapsed = time.monotonic() - started
                self.assertEqual(result["status"], "fail")
                self.assertTrue(result["timed_out"])
                self.assertIsNone(result["exit_code"])
                self.assertLess(elapsed, 20, "cooperative cancellation exceeded its nested cleanup budget")
                cleanup = json.loads((root / "cleanup.json").read_text())
                self.assertTrue(cleanup["cancelled"])
                self.assertTrue(cleanup["registry_empty"])
                self.assertTrue(cleanup["extraction_exists_during_cleanup"])
                self.assertEqual(cleanup["service_returncode"], -signal.SIGKILL)
                self.assertEqual(cleanup["command_returncode"], -signal.SIGTERM)
                for pid in json.loads((root / "owned.json").read_text()).values():
                    with self.assertRaises(ProcessLookupError):
                        os.kill(pid, 0)
                self.assertIsNone(control.poll(), "unrelated control session was terminated")
            self.assertFalse(root.exists())
        finally:
            os.killpg(control.pid, signal.SIGKILL)
            control.wait(timeout=5)

    def test_final_registry_cleanup_runs_on_exception_and_restores_signal_handlers(self):
        controller = r'''
import json, os, signal, subprocess, sys
sys.path.insert(0, sys.argv[1])
from live_ollama_canary import OwnedProcesses
previous = {sig: signal.getsignal(sig) for sig in (signal.SIGTERM, signal.SIGINT)}
owned = OwnedProcesses()
owned.install()
process = owned.start([sys.executable, '-c', 'import time; time.sleep(60)'])
try:
    raise RuntimeError('fixture failure after owned process creation')
except RuntimeError:
    pass
finally:
    owned.close()
assert all(signal.getsignal(sig) == handler for sig, handler in previous.items())
assert not owned.processes
assert process.returncode is not None
try:
    os.kill(process.pid, 0)
    raise AssertionError('owned process remained after exceptional cleanup')
except ProcessLookupError:
    pass
print(json.dumps({'owned_process_joined': True, 'handlers_restored': True}))
'''
        result = subprocess.run([sys.executable, "-c", controller, str(ROOT / "scripts")],
                                capture_output=True, text=True, timeout=15, check=False)
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertEqual(json.loads(result.stdout), {"owned_process_joined": True, "handlers_restored": True})


if __name__ == "__main__":
    unittest.main()
