"""Baseline exit policy tests are not native GUI evidence."""
from __future__ import annotations

import copy
import importlib.util
from pathlib import Path
import unittest

SPEC = importlib.util.spec_from_file_location("live_desktop_smoke", Path(__file__).resolve().parents[1] / "live_desktop_smoke.py")
driver = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(driver)


class BaselineExitPolicyTests(unittest.TestCase):
    def setUp(self):
        self.receipt = {"status": "blocked", "baseline_status": "pass", "live_desktop_exercised": True,
                        "cases": [{"id": case_id, "status": "pass"} for case_id in driver.BASELINE_CASES]}

    def test_successful_baseline_does_not_claim_complete_release(self):
        self.assertEqual(driver.result_exit_code(self.receipt, True), 0)
        self.assertEqual(driver.result_exit_code(self.receipt, False), 2)
        self.assertEqual(self.receipt["status"], "blocked")

    def test_capability_block_cannot_become_ci_success(self):
        self.receipt.update(baseline_status="blocked", live_desktop_exercised=False)
        self.receipt["cases"] = []
        self.assertEqual(driver.result_exit_code(self.receipt, True), 2)

    def test_cleanup_or_source_failure_overrides_passing_cases(self):
        self.receipt["status"] = "fail"
        self.assertEqual(driver.result_exit_code(self.receipt, True), 1)

    def test_missing_failed_or_duplicate_case_is_not_baseline_success(self):
        original = copy.deepcopy(self.receipt)
        self.receipt["cases"].pop()
        self.assertEqual(driver.result_exit_code(self.receipt, True), 2)
        self.receipt = copy.deepcopy(original)
        self.receipt["cases"][0]["status"] = "fail"
        self.assertEqual(driver.result_exit_code(self.receipt, True), 2)
        self.receipt = copy.deepcopy(original)
        self.receipt["cases"].append(copy.deepcopy(self.receipt["cases"][0]))
        self.assertEqual(driver.result_exit_code(self.receipt, True), 2)


if __name__ == "__main__":
    unittest.main()
