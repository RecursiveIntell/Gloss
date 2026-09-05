"""Fail-closed setup contract; these tests are not downloaded-model proof."""
import importlib.util
import json
from pathlib import Path
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


if __name__ == "__main__":
    unittest.main()
