"""Driver acceptance/configuration policy tests are not native GUI evidence."""
from __future__ import annotations

import copy
import http.client
import http.server
import importlib.util
import json
from pathlib import Path
import tempfile
import threading
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


def ollama_config():
    return {"schema": driver.OLLAMA_CONFIG_SCHEMA, "provider": "ollama", "base_url": "http://127.0.0.1:11435",
            "runtime_version": "0.11.10", "chat_model": "qwen3:0.6b", "embedding_model": "all-minilm:22m",
            "chat_model_digest": "sha256:" + "a" * 64, "embedding_model_digest": "b" * 64}


class IntegratedReceiptPolicyTests(unittest.TestCase):
    def setUp(self):
        source = {"worktree_clean": True, "commit": "fixture-source"}
        self.receipt = {"status": "pass", "integrated_status": "pass", "live_desktop_exercised": True,
                        "ollama_config": ollama_config(), "source": source,
                        "build": {"source": source.copy(), "exit_code": 0, "command": ["actual-build"],
                                  "binary": {"path": "gloss", "sha256": "a" * 64},
                                  "log": {"path": "build.log", "sha256": "b" * 64}},
                        "source_scope_widened": False, "hidden_fallback": False, "raw_uuid_flood": False,
                        "cases": [{"id": case, "status": "pass"} for case in driver.REQUIRED_LIVE_CASES]}

    def test_baseline_only_cannot_pass_integrated_gate(self):
        self.receipt["cases"] = self.receipt["cases"][:2]
        self.assertEqual(driver.result_exit_code(self.receipt, require_integrated=True), 2)

    def test_complete_observations_can_pass_while_separate_release_scope_is_blocked(self):
        self.assertEqual(driver.result_exit_code(self.receipt, require_integrated=True), 0)
        self.receipt["status"] = "blocked"
        self.receipt["blockers"] = ["separate packaged release proof is not provided"]
        self.assertEqual(driver.result_exit_code(self.receipt, require_integrated=True), 0)

    def test_unsafe_or_unknown_flags_never_become_integrated_success(self):
        for flag in ("source_scope_widened", "hidden_fallback", "raw_uuid_flood"):
            for value in (True, None, "false"):
                candidate = copy.deepcopy(self.receipt)
                candidate[flag] = value
                self.assertEqual(driver.result_exit_code(candidate, require_integrated=True), 2, (flag, value))

    def test_failed_duplicate_unknown_or_missing_case_is_rejected(self):
        for fault in ("failed", "duplicate", "unknown", "missing"):
            candidate = copy.deepcopy(self.receipt)
            if fault == "failed": candidate["cases"][3]["status"] = "fail"
            elif fault == "duplicate": candidate["cases"].append(candidate["cases"][3].copy())
            elif fault == "unknown": candidate["cases"][3]["id"] = "unrelated"
            else: candidate["cases"].pop()
            self.assertEqual(driver.result_exit_code(candidate, require_integrated=True), 2, fault)

    def test_build_source_mismatch_or_missing_typed_config_is_rejected(self):
        for fault in ("source", "build", "config", "cleanup"):
            candidate = copy.deepcopy(self.receipt)
            if fault == "source": candidate["build"]["source"]["commit"] = "other"
            elif fault == "build": candidate["build"]["exit_code"] = 1
            elif fault == "config": candidate["ollama_config"] = {}
            else: candidate["status"] = "fail"
            self.assertNotEqual(driver.result_exit_code(candidate, require_integrated=True), 0, fault)


class RuntimeConfigPolicyTests(unittest.TestCase):
    def test_config_rejects_egress_credentials_unknown_fields_and_short_digests(self):
        self.assertEqual(driver.validate_ollama_config(ollama_config()), ollama_config())
        for url in ("https://example.com", "http://192.168.1.2:11435", "http://user:secret@127.0.0.1:11435", "http://127.0.0.1:11435?token=secret", "http://127.0.0.1.evil.test:11435"):
            config = ollama_config(); config["base_url"] = url
            with self.assertRaises(ValueError): driver.validate_ollama_config(config)
        for change in ({"api_key": "secret"}, {"chat_model_digest": "abc"}, {"provider": "openai"}):
            with self.assertRaises(ValueError): driver.validate_ollama_config({**ollama_config(), **change})

    def test_prebuilt_loader_binds_source_successful_build_and_actual_files(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            application, binary, log = root / "AppRun", root / "gloss", root / "build.log"
            application.write_text("fixture launcher")
            binary.write_bytes(b"fixture binary")
            log.write_text("fixture successful build log")
            source = {"worktree_clean": True, "commit": "source-a"}
            config = {"schema": "gloss-desktop-prebuilt/v1", "source": source.copy(), "application": str(application),
                      "application_sha256": driver.file_sha256(application), "binary": str(binary),
                      "binary_sha256": driver.file_sha256(binary), "artifact_sha256": "c" * 64,
                      "build_command": ["tauri", "build"], "build_log": str(log), "build_exit_code": 0}
            path = root / "config.json"; path.write_text(json.dumps(config))
            self.assertEqual(driver.load_prebuilt_config(path, source), config)
            with self.assertRaises(ValueError): driver.load_prebuilt_config(path, {**source, "commit": "other"})
            with self.assertRaises(ValueError): driver.load_prebuilt_config(path, {**source, "worktree_clean": False})
            binary.write_bytes(b"changed binary")
            with self.assertRaises(ValueError): driver.load_prebuilt_config(path, source)

    def test_rendered_scope_checks_reject_widening_fallback_and_fake_zero_context(self):
        values = {"Scope": "selected (1 selected, 1 excluded, 0 invalid)", "Context": "1 passages, preserved: yes",
                  "Backend requested": "gloss-local", "Backend used": "native-hybrid", "Retrieval": "hybrid_rrf", "Fallback": "no"}
        driver.require_scope_evidence(values, 1, 1, True)
        for change in ({"Scope": "all (2 selected, 0 excluded, 0 invalid)"}, {"Context": "0 passages, preserved: yes"},
                       {"Context": "1 passages, preserved: no"}, {"Fallback": "yes"}, {"Backend used": "unknown"}):
            with self.assertRaises(RuntimeError): driver.require_scope_evidence({**values, **change}, 1, 1, True)

    def test_degraded_mode_requires_disclosed_dense_failure_and_preserved_scope(self):
        values = {"Scope": "selected (1 selected, 2 excluded, 0 invalid)", "Context": "1 passages, preserved: yes",
                  "Backend requested": "gloss-local", "Backend used": "gloss-local", "Retrieval": "bm25_only",
                  "Fallback": "embedding_index_metadata_stale"}
        driver.require_scope_evidence(values, 1, 2, True, degraded=True)
        with self.assertRaises(RuntimeError): driver.require_scope_evidence(values, 1, 2, True)
        for change in ({"Fallback": "no"}, {"Fallback": "yes"}, {"Fallback": "semantic_memory_timeout"},
                       {"Backend used": "native-dense"}, {"Scope": "all (3 selected, 0 excluded, 0 invalid)"}):
            with self.assertRaises(RuntimeError): driver.require_scope_evidence({**values, **change}, 1, 2, True, degraded=True)


class WebDriverTransportEvidenceTests(unittest.TestCase):
    def test_loopback_request_headers_and_http_errors_are_not_replayed(self):
        received = []

        class ResponseHandler(http.server.BaseHTTPRequestHandler):
            protocol_version = "HTTP/1.1"

            def do_POST(self):
                received.append((dict(self.headers), self.rfile.read(int(self.headers["Content-Length"]))))
                status = 200 if len(received) == 1 else 503
                body = b'{"value":{"acknowledged":true}}' if status == 200 else b'fixture unavailable'
                self.send_response(status)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

            def log_message(self, *_args):
                pass

        with http.server.HTTPServer(("127.0.0.1", 0), ResponseHandler) as server:
            server.timeout = 2
            worker = threading.Thread(target=lambda: [server.handle_request() for _ in range(2)])
            worker.start()
            client = driver.WebDriver(server.server_port)
            self.assertEqual(client.request("POST", "/session/fixture/action", {}), {"acknowledged": True})
            with self.assertRaisesRegex(RuntimeError, "HTTP 503: fixture unavailable"):
                client.request("POST", "/session/fixture/action", {})
            worker.join(timeout=3)
            self.assertFalse(worker.is_alive())
        self.assertEqual(len(received), 2)
        self.assertTrue(all(headers.get("Connection", "").lower() != "close" for headers, _ in received))
        self.assertEqual([body for _, body in received], [b"{}", b"{}"])
        self.assertEqual(len(client.trace), 2)
        self.assertIn("HTTP 503", client.trace[1]["error"])
        self.assertNotIn("response", client.trace[1])

    def test_disconnected_mutation_is_recorded_and_never_replayed(self):
        received = []

        class ClosingHandler(http.server.BaseHTTPRequestHandler):
            def do_POST(self):
                received.append((self.path, self.rfile.read(int(self.headers["Content-Length"]))))
                self.close_connection = True

            def log_message(self, *_args):
                pass

        with http.server.HTTPServer(("127.0.0.1", 0), ClosingHandler) as server:
            server.timeout = 2
            worker = threading.Thread(target=server.handle_request, daemon=True)
            worker.start()
            client = driver.WebDriver(server.server_port)
            path = "/session/fixture/element/delete-control/click"
            with self.assertRaises(http.client.RemoteDisconnected):
                client.request("POST", path, {})
            worker.join(timeout=2)
            self.assertFalse(worker.is_alive())
        self.assertEqual(received, [(path, b"{}")])
        self.assertEqual(len(client.trace), 1)
        self.assertEqual(client.trace[0]["path"], path)
        self.assertEqual(client.trace[0]["request"], {})
        self.assertIn("RemoteDisconnected", client.trace[0]["error"])
        self.assertNotIn("response", client.trace[0])


if __name__ == "__main__":
    unittest.main()
