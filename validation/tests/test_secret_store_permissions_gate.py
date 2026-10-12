"""Exercise the actual permission gate; receipt truth is not native permission proof."""
import contextlib
import importlib.util
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "secret_store_permissions_gate", ROOT / "validation/gloss_secret_store_permissions_gate.py"
)
GATE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(GATE)


class SecretStorePermissionsGate(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.repo = Path(self.temp.name)
        store = self.repo / "src-tauri/src/provider_config_store.rs"
        store.parent.mkdir(parents=True)
        shutil.copyfile(ROOT / "src-tauri/src/provider_config_store.rs", store)
        run_root = self.repo / "docs/codex-runs"
        run_root.mkdir(parents=True)
        (run_root / "CURRENT_RUN.md").write_text("Current run: `PERMISSION_GATE_TEST`\n")
        self.receipt = run_root / "PERMISSION_GATE_TEST/SECRET_STORE_PERMISSION_RECEIPT.json"
        self.receipt.parent.mkdir()

    def invoke_gate(self):
        output = io.StringIO()
        with mock.patch("sys.argv", [str(SPEC.origin), "--repo", str(self.repo)]):
            with contextlib.redirect_stdout(output):
                status = GATE.main()
        result = json.loads(output.getvalue())
        self.assertIs(result["ok"], status == 0)
        return status, result["failures"]

    def write_receipt(self, permission_value=True):
        self.receipt.write_text(json.dumps({
            "schema": "SecretStorePermissionReceiptV1",
            "unix_owner_only_permissions_active": permission_value,
        }))

    def assert_failed(self, expected_failure):
        status, failures = self.invoke_gate()
        self.assertEqual(status, 1)
        self.assertIn(expected_failure, failures)

    def test_literal_true_passes_actual_gate_with_current_store_source(self):
        self.write_receipt(True)
        self.assertEqual(self.invoke_gate(), (0, []))

    def test_false_and_non_boolean_permission_values_fail(self):
        values = [False, "false", "true", "", 0, 1, -1, 1.0, None, [], [True], {}]
        for value in values:
            with self.subTest(value=value, value_type=type(value).__name__):
                self.write_receipt(value)
                self.assert_failed(
                    "secret-store receipt does not mark Unix owner-only permissions active"
                )

    def test_missing_permission_value_fails(self):
        self.receipt.write_text(json.dumps({"schema": "SecretStorePermissionReceiptV1"}))
        self.assert_failed(
            "secret-store receipt does not mark Unix owner-only permissions active"
        )

    def test_non_object_json_fails_without_exception(self):
        for value in [None, True, False, 0, 1, "true", [], [{"schema": "SecretStorePermissionReceiptV1"}]]:
            with self.subTest(value=value):
                self.receipt.write_text(json.dumps(value))
                self.assert_failed("secret-store permission receipt must be a JSON object")

    def test_malformed_json_fails_without_exception(self):
        for value in ["", "{", '{"schema":', "null trailing", '{"schema": "SecretStorePermissionReceiptV1",}']:
            with self.subTest(value=value):
                self.receipt.write_text(value)
                status, failures = self.invoke_gate()
                self.assertEqual(status, 1)
                self.assertTrue(any(
                    failure.startswith("invalid secret-store permission receipt JSON:")
                    for failure in failures
                ))

    def test_unreadable_receipt_text_fails_without_exception(self):
        self.receipt.write_bytes(b"\xff")
        status, failures = self.invoke_gate()
        self.assertEqual(status, 1)
        self.assertTrue(any(
            failure.startswith("invalid secret-store permission receipt JSON:")
            for failure in failures
        ))

    def test_missing_receipt_fails(self):
        self.assert_failed(
            "missing secret-store permission receipt: "
            "docs/codex-runs/PERMISSION_GATE_TEST/SECRET_STORE_PERMISSION_RECEIPT.json"
        )

    def test_wrong_schema_fails_even_with_literal_true(self):
        self.receipt.write_text(json.dumps({
            "schema": "OtherReceiptV1",
            "unix_owner_only_permissions_active": True,
        }))
        self.assert_failed("secret-store permission receipt schema mismatch")

    def test_source_permission_checks_still_fail_even_with_literal_true(self):
        self.write_receipt(True)
        store = self.repo / "src-tauri/src/provider_config_store.rs"
        store.write_text(store.read_text().replace("0o600", "0o644"))
        self.assert_failed("missing owner-only file permissions")


if __name__ == "__main__":
    unittest.main()
