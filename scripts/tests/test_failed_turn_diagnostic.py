"""Failure evidence contracts with synthetic data; not a real-model claim."""
import copy
from contextlib import redirect_stdout
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import sqlite3
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

SCRIPTS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(SCRIPTS))
import diagnose_failed_ollama_turn as diagnostic
import live_ollama_canary as canary


def fixture():
    # Independent literal represents serde_json::Value output: sorted keys,
    # no images for None, explicit null optional settings and f64 temperature.
    serialized = ('{"decoding_settings":{"max_tokens":2048,"min_p":null,"repeat_penalty":null,'
                  '"temperature":0.0,"top_k":null,"top_p":null},"max_tokens":2048,'
                  '"messages":[{"content":"HELLO","role":"user"},'
                  '{"content":"HELLO.\\n","role":"assistant"},'
                  '{"content":"RETRY","role":"user"}],"model":"qwen3:0.6b",'
                  '"num_ctx":8192,"system":"Saved system\\n"}')
    material = json.loads(serialized)
    hashed = hashlib.sha256(serialized.encode()).hexdigest()
    rows = []
    for index, (role, content) in enumerate([("user", "HELLO"), ("assistant", "HELLO.\n"),
                                           ("user", "Cancelled question"), ("user", "RETRY"),
                                           ("assistant", "HELLO.\n")]):
        rows.append({"rowid": index + 1, "id": str(index), "conversation_id": "conversation",
                     "role": role, "created_at": "equal timestamps are not reordered",
                     "content": content, "content_truncated": False,
                     "content_sha256": hashlib.sha256(content.encode()).hexdigest()})
    prompt = {"schema": "PromptReceiptV1", "notebook_id": "notebook", "conversation_id": "conversation",
              "message_id": "4", "source_passage_count": 0, "system_prompt_text": "Saved system\n",
              "system_prompt_digest": diagnostic.digest("Saved system\n"),
              "user_turn_digest": diagnostic.digest("RETRY"), "prompt_digest": hashed}
    assistant = {"notebook_id": "notebook", "conversation_id": "conversation", "message_id": "4",
                 "model": "qwen3:0.6b", "prompt_receipt": prompt,
                 "decoding_settings_receipt": {"provider": "ollama", "model": "qwen3:0.6b",
                                               "effective": material["decoding_settings"]},
                 "prompt_budget_receipt": {"model_context_window": 8192, "message_count": 3,
                                           "source_passage_count": 0, "prompt_digest": hashed,
                                           "system_prompt_chars": len(serialized.encode())}}
    notebook = {"notebook_id": "notebook", "conversation_id": "conversation", "message_count": 5,
                "messages_truncated": False, "messages": rows, "latest_assistant": assistant}
    return {"case": "chat_cancel_and_retry", "owned_profile_evidence": {
        "schema": "GlossDesktopFailureProfileV1", "status": "ok", "truncated": False, "notebooks": [notebook]}}


def notebook(failure):
    return failure["owned_profile_evidence"]["notebooks"][0]


class ReconstructionTests(unittest.TestCase):
    def test_saved_digest_proves_nonadjacent_rerun_prefix_and_exact_whitespace(self):
        failure = fixture()
        body = diagnostic.reconstruct(failure, "qwen3:0.6b")["body"]
        self.assertEqual([row["content"] for row in body["messages"]],
                         ["Saved system\n", "HELLO", "HELLO.\n", "RETRY"])
        self.assertEqual(body["options"], {"temperature": 0.0, "num_predict": 2048, "num_ctx": 8192})
        self.assertEqual(body["think"], False)
        self.assertEqual(body["stream"], True)
        self.assertEqual(failure, fixture(), "reconstruction mutated captured evidence")

    def test_incomplete_mismatched_or_ambiguous_material_is_rejected(self):
        changes = [
            lambda f: f["owned_profile_evidence"].update(truncated=True),
            lambda f: f["owned_profile_evidence"].update(status="partial"),
            lambda f: notebook(f).update(capture_error="ValueError"),
            lambda f: notebook(f).update(messages_truncated=True),
            lambda f: notebook(f)["messages"][0].update(content_truncated=True),
            lambda f: notebook(f)["messages"][1].update(content="HELLO."),
            lambda f: notebook(f)["latest_assistant"].update(model="different"),
            lambda f: notebook(f)["latest_assistant"]["prompt_receipt"].update(message_id="1"),
            lambda f: notebook(f)["latest_assistant"]["prompt_receipt"].update(system_prompt_text="guessed"),
            lambda f: notebook(f)["latest_assistant"]["prompt_receipt"].update(prompt_digest="0" * 64),
            lambda f: notebook(f)["latest_assistant"]["prompt_receipt"].update(source_passage_count=1),
            lambda f: notebook(f)["latest_assistant"]["prompt_budget_receipt"].update(model_context_window=4096),
            lambda f: notebook(f)["latest_assistant"]["prompt_budget_receipt"].update(message_count=4),
            lambda f: notebook(f)["latest_assistant"]["decoding_settings_receipt"]["effective"].pop("top_p"),
            lambda f: notebook(f)["messages"][2].update(content="RETRY", content_sha256=diagnostic.digest("RETRY")),
            lambda f: notebook(f)["messages"].__setitem__(slice(0, 2), list(reversed(notebook(f)["messages"][:2]))),
        ]
        for index, change in enumerate(changes):
            with self.subTest(index=index):
                failure = fixture()
                change(failure)
                with self.assertRaises((ValueError, KeyError)):
                    diagnostic.reconstruct(failure, "qwen3:0.6b")

    def test_actual_sqlite_collector_output_reconstructs_same_verified_request(self):
        from live_desktop_smoke import collect_desktop_failure_evidence
        failure = fixture()
        saved = notebook(failure)
        notebook_id = "11111111-1111-4111-8111-111111111111"
        saved["latest_assistant"]["notebook_id"] = notebook_id
        saved["latest_assistant"]["prompt_receipt"]["notebook_id"] = notebook_id
        evidence = {key: saved["latest_assistant"][key] for key in
                    ("prompt_receipt", "prompt_budget_receipt", "decoding_settings_receipt")}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            database = root / "profile/data/gloss/notebooks" / notebook_id / "notebook.db"
            database.parent.mkdir(parents=True)
            with sqlite3.connect(database) as connection:
                connection.execute("CREATE TABLE messages(id TEXT PRIMARY KEY, conversation_id TEXT, "
                                   "role TEXT, content TEXT, created_at TEXT, citations TEXT, model_used TEXT)")
                for row in saved["messages"]:
                    connection.execute("INSERT INTO messages VALUES(?,?,?,?,?,?,?)",
                        (row["id"], row["conversation_id"], row["role"], row["content"],
                         "2026-09-05T00:00:00Z", json.dumps({"evidence": evidence}) if row["id"] == "4" else None,
                         "qwen3:0.6b" if row["role"] == "assistant" else None))
            captured = collect_desktop_failure_evidence(root)
        self.assertEqual(captured["status"], "ok")
        actual = diagnostic.reconstruct({"case": "chat_cancel_and_retry", "owned_profile_evidence": captured}, "qwen3:0.6b")
        self.assertEqual(actual["body"]["messages"][-1]["content"], "RETRY")
        self.assertEqual(actual["request_material_sha256"], saved["latest_assistant"]["prompt_receipt"]["prompt_digest"])

    def test_all_saved_knobs_are_preserved_with_rust_f32_value_widening(self):
        failure = fixture()
        assistant = notebook(failure)["latest_assistant"]
        original = diagnostic.reconstruct(failure, "qwen3:0.6b")
        assistant["decoding_settings_receipt"]["effective"].update(
            temperature=0.7, top_p=0.9, top_k=21, min_p=0.05, repeat_penalty=1.1)
        material = original["request_material"]
        material["decoding_settings"].update(temperature=0.699999988079071, top_p=0.8999999761581421,
            top_k=21, min_p=0.05000000074505806, repeat_penalty=1.100000023841858)
        encoded = diagnostic.canonical(material)
        assistant["prompt_receipt"]["prompt_digest"] = diagnostic.digest(encoded)
        assistant["prompt_budget_receipt"].update(prompt_digest=diagnostic.digest(encoded),
                                                  system_prompt_chars=len(encoded.encode()))
        actual = diagnostic.reconstruct(failure, "qwen3:0.6b")["body"]["options"]
        self.assertEqual(actual, {"temperature": 0.699999988079071, "top_p": 0.8999999761581421,
            "top_k": 21, "min_p": 0.05000000074505806, "repeat_penalty": 1.100000023841858,
            "num_ctx": 8192, "num_predict": 2048})


class DiagnosticExecutionTests(unittest.TestCase):
    def test_evidence_links_and_oversized_files_are_rejected_before_open(self):
        with tempfile.TemporaryDirectory() as directory:
            base = Path(directory)
            root, outside = base / "owned", base / "outside"
            root.mkdir()
            outside.mkdir()
            (outside / "failure.json").write_text('{"synthetic":"outside"}')
            (root / "desktop").symlink_to(outside, target_is_directory=True)
            with patch.object(canary.os, "open") as opened:
                with self.assertRaisesRegex(ValueError, "Linked"):
                    diagnostic.read_json(root / "desktop/failure.json", root)
                opened.assert_not_called()
            (root / "desktop").unlink()
            (root / "desktop").mkdir()
            with patch.object(canary.os, "open") as opened:
                with self.assertRaisesRegex(ValueError, "Outside"):
                    diagnostic.read_json(root / "../outside/failure.json", root)
                opened.assert_not_called()
            target = root / "desktop/failure.json"
            for kind in ("symlink", "hardlink", "oversized", "fifo"):
                with self.subTest(kind=kind):
                    if kind == "symlink":
                        target.symlink_to(outside / "failure.json")
                    elif kind == "hardlink":
                        os.link(outside / "failure.json", target)
                    elif kind == "fifo":
                        os.mkfifo(target)
                    else:
                        with target.open("wb") as stream:
                            stream.truncate(2 * 1024 * 1024 + 1)
                    with patch.object(canary.os, "open") as opened:
                        with self.assertRaises(ValueError):
                            diagnostic.read_json(target, root)
                        opened.assert_not_called()
                    target.unlink()

    def test_evidence_read_is_bounded_and_hashes_the_same_validated_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            target = root / "failure.json"
            raw = b'{"synthetic":"inside"}\n'
            target.write_bytes(raw)
            with patch.object(Path, "read_bytes", side_effect=AssertionError("unbounded reread")):
                parsed, hashed = diagnostic.read_json(target, root)
            self.assertEqual(parsed, {"synthetic": "inside"})
            self.assertEqual(hashed, hashlib.sha256(raw).hexdigest())

    def prepare(self, root):
        (root / "desktop").mkdir()
        models = {name: {"name": name, "digest": pin["digest_prefix"] + "0" * 52, "size": 1}
                  for name, pin in canary.MODELS.items()}
        source = {"source_sha256": "current"}
        parent = {"schema": canary.CANARY_SCHEMA, "live_service_exercised": True, "source": source,
                  "runtime_download": {"version": canary.VERSION, "sha256": canary.ASSET_SHA256,
                                       "published_checksum_matched": True}, "models": models,
                  "runtime_binary_sha256": "a" * 64}
        child = {"source": source, "status": "fail", "ollama_config": canary.desktop_configuration(models)}
        for path, value in [(root / "receipt.json", parent),
                            (root / "desktop/LIVE_DESKTOP_SMOKE_RECEIPT.json", child),
                            (root / "desktop/failure.json", fixture())]:
            path.write_text(json.dumps(value))
        return source

    def test_exactly_two_separate_observations_change_only_system_instruction(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self.prepare(root)
            with patch.object(diagnostic, "capture_source_identity", return_value=source), \
                 patch.object(diagnostic, "observe", side_effect=[{"status": "observed", "answer": "HELLO"},
                                                                  {"status": "observed", "answer": "RETRY"}]) as observe:
                result = diagnostic.diagnose(root, root)
            self.assertEqual(observe.call_count, 2)
            baseline, candidate = [item.args[0] for item in observe.call_args_list]
            candidate = copy.deepcopy(candidate)
            self.assertEqual(candidate["messages"][0]["content"], baseline["messages"][0]["content"] + "\n" + diagnostic.INSTRUCTION)
            candidate["messages"][0] = baseline["messages"][0]
            self.assertEqual(candidate, baseline)
            self.assertEqual(result["status"], "observed")
            self.assertEqual(json.loads((root / "desktop/LIVE_DESKTOP_SMOKE_RECEIPT.json").read_text())["status"], "fail")

    def test_stale_source_model_runtime_or_digest_block_before_any_request(self):
        changes = [
            ("receipt.json", lambda p: p.update(source={"source_sha256": "stale"})),
            ("receipt.json", lambda p: p["runtime_download"].update(version="other")),
            ("desktop/LIVE_DESKTOP_SMOKE_RECEIPT.json", lambda p: p["ollama_config"].update(chat_model_digest="0" * 64)),
            ("desktop/failure.json", lambda p: notebook(p)["latest_assistant"]["prompt_receipt"].update(prompt_digest="0" * 64)),
        ]
        for path, change in changes:
            with self.subTest(path=path), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                source = self.prepare(root)
                value = json.loads((root / path).read_text())
                change(value)
                (root / path).write_text(json.dumps(value))
                with patch.object(diagnostic, "capture_source_identity", return_value=source), \
                     patch.object(diagnostic, "observe") as observe:
                    result = diagnostic.diagnose(root, root)
                observe.assert_not_called()
                self.assertEqual(result["status"], "blocked")
                self.assertEqual(result["calls"], [])

    def test_transport_error_preserves_request_and_prevents_retry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = self.prepare(root)
            with patch.object(diagnostic, "capture_source_identity", return_value=source), \
                 patch.object(diagnostic, "observe", side_effect=TimeoutError("bounded timeout")) as observe:
                result = diagnostic.diagnose(root, root)
            self.assertEqual(observe.call_count, 1)
            self.assertEqual(result["status"], "error")
            self.assertEqual(result["calls"][0]["request"]["messages"][-1]["content"], "RETRY")


class StreamAndCompositionTests(unittest.TestCase):
    def test_parent_reuses_reader_and_never_reads_or_echoes_outside_or_oversized_result(self):
        self.assertIs(diagnostic.read_json, canary.read_owned_json)
        for mode in ("ancestor_link", "hardlink", "oversized"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                base = Path(directory)
                root, outside = base / "owned", base / "outside"
                root.mkdir()
                outside.mkdir()
                raw = json.dumps({"status": "observed", "verified_request": {
                    "request_material": {"messages": [{"content": "OWNED_OUTSIDE_MARKER"}]}}})
                (outside / "receipt.json").write_text(raw)
                target = root / "failed-turn-diagnostic"
                if mode == "ancestor_link":
                    target.symlink_to(outside, target_is_directory=True)
                else:
                    target.mkdir()
                    if mode == "hardlink":
                        os.link(outside / "receipt.json", target / "receipt.json")
                    else:
                        with (target / "receipt.json").open("wb") as stream:
                            stream.truncate(2 * 1024 * 1024 + 1)
                original = RuntimeError("original native failure")
                def run(label, command, seconds):
                    if label == "native-desktop-workflow":
                        raise original
                    raise RuntimeError("diagnostic launch failed")
                captured, receipt = io.StringIO(), {}
                with redirect_stdout(captured), patch.object(canary.os, "open") as opened:
                    with self.assertRaises(RuntimeError) as caught:
                        canary.run_desktop_with_diagnostic(run, [], root, root,
                                                          canary.OwnedProcesses(), receipt, lambda: None)
                    opened.assert_not_called()
                self.assertIs(caught.exception, original)
                self.assertNotIn("OWNED_OUTSIDE_MARKER", captured.getvalue())
                self.assertIn("summary_error", receipt["failure_diagnostic"])
                self.assertNotIn("sha256", receipt["failure_diagnostic"])

    def test_parent_emits_validated_partial_result_and_hash_without_second_read(self):
        for child_failed in (False, True):
            with self.subTest(child_failed=child_failed), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                result = {"status": "observed", "verified_request": {"request_material_sha256": "a" * 64,
                    "request_material": {"model": "qwen3:0.6b", "messages": [{"content": "RETRY"}]}},
                    "calls": [{"label": "baseline", "status": "observed", "answer": "HELLO"}]}
                raw = json.dumps(result).encode()
                original = RuntimeError("original native failure")
                def run(label, command, seconds):
                    if label == "native-desktop-workflow":
                        raise original
                    (root / "failed-turn-diagnostic").mkdir()
                    (root / "failed-turn-diagnostic/receipt.json").write_bytes(raw)
                    if child_failed:
                        raise RuntimeError("diagnostic failed after saving observation")
                captured, receipt = io.StringIO(), {}
                with redirect_stdout(captured), \
                     patch.object(Path, "read_text", side_effect=AssertionError("unbounded text read")), \
                     patch.object(canary, "sha256", side_effect=AssertionError("unvalidated hash reread")):
                    with self.assertRaises(RuntimeError) as caught:
                        canary.run_desktop_with_diagnostic(run, [], root, root,
                                                          canary.OwnedProcesses(), receipt, lambda: None)
                self.assertIs(caught.exception, original)
                summary = json.loads(captured.getvalue().removeprefix("FAILED_TURN_DIAGNOSTIC "))
                self.assertEqual(summary["current_query"], "RETRY")
                self.assertEqual(summary["observations"][0]["answer"], "HELLO")
                self.assertEqual(summary["command_status"], "error" if child_failed else "observed")
                self.assertEqual(receipt["failure_diagnostic"]["sha256"], hashlib.sha256(raw).hexdigest())

    def test_parent_summary_preserves_decisive_content_and_bounds_large_output(self):
        result = {"status": "observed", "verified_request": {"request_material_sha256": "a" * 64,
            "request_material": {"model": "qwen3:0.6b", "messages": [{"content": "RETRY"}]}},
            "calls": [{"label": "baseline", "status": "observed", "answer": "HELLO"},
                      {"label": "latest_user_instruction_experiment", "status": "error",
                       "answer": "x" * 10000, "error": "y" * 10000}]}
        summary = canary.failed_turn_summary(result)
        self.assertEqual(summary["current_query"], "RETRY")
        self.assertEqual(summary["user_turn_sha256"], diagnostic.digest("RETRY"))
        self.assertEqual(summary["observations"][0]["answer"], "HELLO")
        self.assertTrue(summary["observations"][1]["answer_truncated"])
        self.assertLess(len(json.dumps(summary)), 6000)

    def test_raw_stream_bound_terminal_protocol_and_no_redirect_or_proxy_target(self):
        body = diagnostic.reconstruct(fixture(), "qwen3:0.6b")["body"]
        terminal = {"model": "qwen3:0.6b", "message": {"content": "RETRY"}, "done": True}
        cases = [(json.dumps(terminal).encode() + b"\n", True),
                 (b"", False), (b"x" * (16 * 1024 + 1), False),
                 (json.dumps({**terminal, "model": "wrong"}).encode(), False),
                 (json.dumps({**terminal, "done": False}).encode(), False),
                 ((json.dumps(terminal) + "\n") .encode() * 2, False)]
        for raw, valid in cases:
            with self.subTest(valid=valid, length=len(raw)), tempfile.TemporaryDirectory() as directory:
                response = io.BytesIO(raw)
                response.status = 200
                with patch.object(diagnostic.urllib.request, "build_opener") as build:
                    build.return_value.open.return_value = response
                    if valid:
                        observed = diagnostic.observe(body, Path(directory) / "response.jsonl")
                        self.assertEqual(observed["answer"], "RETRY")
                        self.assertEqual(observed["raw_response_sha256"], diagnostic.digest(raw))
                    else:
                        with self.assertRaises(ValueError):
                            diagnostic.observe(body, Path(directory) / "response.jsonl")
                self.assertEqual(build.call_args.args[0].proxies, {})
                self.assertIsInstance(build.call_args.args[1], diagnostic.NoRedirect)
                self.assertEqual(build.return_value.open.call_count, 1)
                self.assertEqual(build.return_value.open.call_args.args[0].full_url, canary.ENDPOINT + "/api/chat")

    def test_deadline_expires_without_stream_retry(self):
        response = io.BytesIO(b"unused")
        response.status = 200
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(diagnostic.urllib.request, "build_opener") as build, \
             patch.object(diagnostic.time, "monotonic", side_effect=[0, 91]):
            build.return_value.open.return_value = response
            with self.assertRaisesRegex(ValueError, "deadline"):
                diagnostic.observe({"model": "qwen3:0.6b"}, Path(directory) / "response.jsonl")
            self.assertEqual(build.return_value.open.call_count, 1)

    @unittest.skipUnless(sys.platform == "linux", "the diagnostic owns a Linux process timer")
    def test_blocking_read_cannot_extend_per_request_deadline(self):
        class SlowResponse(io.BytesIO):
            status = 200
            def readline(self, *_args):
                time.sleep(2)
                raise AssertionError("deadline did not interrupt blocking read")
        previous = signal.getsignal(signal.SIGALRM)
        with tempfile.TemporaryDirectory() as directory, \
             patch.object(diagnostic.urllib.request, "build_opener") as build, \
             patch.object(diagnostic, "DEADLINE_SECONDS", 0.05):
            build.return_value.open.return_value = SlowResponse()
            started = time.monotonic()
            with self.assertRaisesRegex(TimeoutError, "deadline"):
                diagnostic.observe({"model": "qwen3:0.6b"}, Path(directory) / "response.jsonl")
            self.assertLess(time.monotonic() - started, 1)
        self.assertEqual(signal.getsignal(signal.SIGALRM), previous)
        self.assertEqual(signal.getitimer(signal.ITIMER_REAL), (0.0, 0.0))

    def test_redirect_handler_never_constructs_a_second_request(self):
        handler = diagnostic.NoRedirect()
        request = diagnostic.urllib.request.Request(canary.ENDPOINT + "/api/chat", data=b"{}")
        for status in (301, 302, 303, 307, 308):
            with self.subTest(status=status), patch.object(handler, "parent", create=True) as parent:
                with self.assertRaisesRegex(ValueError, "redirects"):
                    handler.http_error_302(request, io.BytesIO(), status, "redirect",
                                           {"location": "http://outside.invalid/chat"})
                parent.open.assert_not_called()

    def test_original_native_failure_survives_diagnostic_success_error_and_save_error(self):
        for mode in ("success", "error", "save_error"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as directory:
                root = Path(directory)
                original = RuntimeError("original native failure")
                calls, receipt = [], {}
                def run(label, command, seconds):
                    calls.append((label, seconds))
                    if label == "native-desktop-workflow":
                        raise original
                    if mode == "error":
                        raise TimeoutError("diagnostic timeout")
                    (root / "failed-turn-diagnostic").mkdir()
                    (root / "failed-turn-diagnostic/receipt.json").write_text('{"status":"observed"}')
                def save():
                    if mode == "save_error":
                        raise OSError("evidence disk error")
                with self.assertRaises(RuntimeError) as caught:
                    canary.run_desktop_with_diagnostic(run, ["native"], root, root,
                                                        canary.OwnedProcesses(), receipt, save)
                self.assertIs(caught.exception, original)
                self.assertEqual(calls[0], ("native-desktop-workflow", 1200))
                if mode != "save_error":
                    self.assertEqual(calls[1], ("failed-turn-diagnostic", 190))

    def test_cancellation_never_starts_diagnostic_and_success_never_starts_it(self):
        for mode in ("cancel_exception", "cancel_flag", "success"):
            with self.subTest(mode=mode):
                owned, calls, receipt = canary.OwnedProcesses(), [], {}
                def run(label, command, seconds):
                    calls.append(label)
                    if mode == "cancel_exception":
                        raise canary.CanaryCancelled("cancelled")
                    if mode == "cancel_flag":
                        owned.cancel_signal = signal.SIGTERM
                        raise RuntimeError("native failed while cancellation pending")
                if mode == "success":
                    canary.run_desktop_with_diagnostic(run, [], Path("."), Path("."), owned, receipt, lambda: None)
                else:
                    with self.assertRaises(RuntimeError):
                        canary.run_desktop_with_diagnostic(run, [], Path("."), Path("."), owned, receipt, lambda: None)
                self.assertEqual(calls, ["native-desktop-workflow"])

    def test_cancellation_during_diagnostic_is_classified_and_original_fail_survives(self):
        original = RuntimeError("original native failure")
        owned, receipt = canary.OwnedProcesses(), {}
        def run(label, command, seconds):
            if label == "native-desktop-workflow":
                raise original
            owned.cancel_signal = signal.SIGTERM
            raise canary.CanaryCancelled("cancelled during diagnostic")
        with self.assertRaises(RuntimeError) as caught:
            canary.run_desktop_with_diagnostic(run, [], Path("."), Path("."), owned, receipt, lambda: None)
        self.assertIs(caught.exception, original)
        self.assertEqual(receipt["failure_diagnostic"]["status"], "cancelled")


if __name__ == "__main__":
    unittest.main()
