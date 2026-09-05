"""Contract tests for the canonical release verifier."""

from __future__ import annotations

import importlib.util
import sys
import unittest
from unittest.mock import patch
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
MODULE_PATH = REPO_ROOT / "scripts" / "verify_release.py"


def load_verifier_module():
    # Match the script's normal direct-execution import path.
    if str(MODULE_PATH.parent) not in sys.path:
        sys.path.insert(0, str(MODULE_PATH.parent))
    spec = importlib.util.spec_from_file_location("verify_release", MODULE_PATH)
    if spec is None or spec.loader is None:
        raise RuntimeError("could not load canonical verifier")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class VerifyReleaseContractTests(unittest.TestCase):
    def test_changed_source_cannot_receive_a_passing_receipt(self):
        verifier = load_verifier_module()
        with patch.object(verifier, "capture_source_identity", side_effect=[
            {"revision": "same", "source_sha256": "before"},
            {"revision": "same", "source_sha256": "after"},
        ]):
            receipt, code = verifier.verify_snapshot(
                REPO_ROOT, [verifier.Gate("fixture", ("fixture",))],
                runner=lambda *_: 0,
            )
        self.assertEqual(code, 1)
        self.assertEqual(receipt["status"], "failed")
        self.assertFalse(receipt["source_unchanged"])

    def test_failed_gate_explicitly_lists_unrun_gates(self):
        verifier = load_verifier_module()
        with patch.object(verifier, "capture_source_identity", return_value={"revision": "same"}):
            receipt, code = verifier.verify_snapshot(
                REPO_ROOT, [verifier.Gate("failed", ("first",)), verifier.Gate("unrun", ("second",))],
                runner=lambda *_: 7,
            )
        self.assertEqual(code, 7)
        self.assertEqual(receipt["unrun_gates"], ["unrun"])
        self.assertEqual(receipt["scope"], "build_and_contracts_only")

    def test_gate_inventory_is_complete_and_fail_fast_stops_after_first_failure(self):
        verifier = load_verifier_module()
        gates = verifier.build_gates(skip_desktop_compile=False)

        self.assertEqual(
            [gate.name for gate in gates],
            [
                "cargo_fmt",
                "tauri_contract",
                "rust_static_repair_gates",
                "python_script_contracts",
                "python_validation_contracts",
                "native_owner_contracts",
                "turbo_quant_runtime_contracts",
                "cargo_check_default",
                "cargo_check_semantic_memory_backend",
                "cargo_check_semantic_memory_turbo_quant",
                "cargo_test_semantic_memory_turbo_quant",
                "npm_unit_tests",
                "npm_static_contract_tests",
                "npm_build",
                "cargo_clippy",
                "cargo_deny",
                "npm_production_audit",
                "desktop_compile",
            ],
        )

        commands_run: list[tuple[str, ...]] = []

        def failing_runner(command: tuple[str, ...], _root: Path) -> int:
            commands_run.append(command)
            return 23

        receipt, exit_code = verifier.run_gates(
            gates,
            root=REPO_ROOT,
            runner=failing_runner,
        )

        self.assertEqual(exit_code, 23)
        self.assertEqual(receipt["status"], "failed")
        self.assertEqual(len(commands_run), 1)
        self.assertEqual(len(receipt["gates"]), 1)
        self.assertEqual(receipt["gates"][0]["name"], "cargo_fmt")
        self.assertEqual(receipt["gates"][0]["exit_code"], 23)


if __name__ == "__main__":
    unittest.main()
