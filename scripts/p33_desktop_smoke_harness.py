#!/usr/bin/env python3
from __future__ import annotations

import argparse
import base64
import datetime as dt
import json
import os
import platform
import shutil
import subprocess
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Any

RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"
SMOKE_MODEL = "qwen3.5:0.8b"
SMOKE_SOURCE_TITLE = "p33-desktop-smoke-source.md"
SMOKE_SOURCE_TEXT = (
    "P33 desktop smoke source.\n\n"
    "The P33 desktop smoke answer is ORCHID-913. "
    "When asked for the smoke answer, cite this source as [1].\n"
)
SMOKE_PROMPT = (
    "Using the selected source, answer exactly: "
    "The P33 desktop smoke answer is ORCHID-913 [1]."
)


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def tool_path(name: str) -> str | None:
    found = shutil.which(name)
    if found:
        return found
    cargo_bin = Path.home() / ".cargo" / "bin" / name
    if cargo_bin.exists():
        return str(cargo_bin)
    return None


def run(cmd: list[str], repo: Path, env: dict[str, str] | None = None) -> dict[str, object]:
    started = time.monotonic()
    proc = subprocess.run(
        cmd,
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        env=env,
    )
    return {
        "cmd": cmd,
        "returncode": proc.returncode,
        "duration_ms": int((time.monotonic() - started) * 1000),
        "output_tail": proc.stdout[-4000:],
    }


def webdriver_request(
    method: str,
    url: str,
    payload: dict[str, Any] | None = None,
    timeout: int = 30,
) -> dict[str, Any]:
    data = None if payload is None else json.dumps(payload).encode("utf-8")
    req = urllib.request.Request(
        url,
        data=data,
        headers={"Content-Type": "application/json"},
        method=method,
    )
    raw = urllib.request.urlopen(req, timeout=timeout).read()
    if not raw:
        return {}
    return json.loads(raw)


class WebDriverSession:
    def __init__(self, base_url: str, session_id: str):
        self.base_url = base_url.rstrip("/")
        self.session_id = session_id

    def endpoint(self, path: str) -> str:
        return f"{self.base_url}/session/{self.session_id}{path}"

    def execute(self, script: str, timeout: int = 30) -> Any:
        response = webdriver_request(
            "POST",
            self.endpoint("/execute/sync"),
            {"script": script, "args": []},
            timeout=timeout,
        )
        return response.get("value")

    def execute_async(self, script: str, timeout: int = 120) -> Any:
        response = webdriver_request(
            "POST",
            self.endpoint("/execute/async"),
            {"script": script, "args": []},
            timeout=timeout,
        )
        return response.get("value")

    def invoke(self, command: str, payload: dict[str, Any] | None = None, timeout: int = 120) -> Any:
        script = (
            "const done = arguments[arguments.length - 1];"
            f"window.__TAURI_INTERNALS__.invoke({json.dumps(command)}, {json.dumps(payload or {})})"
            ".then((value) => done({ ok: true, value }))"
            ".catch((error) => done({ ok: false, error: String(error) }));"
        )
        result = self.execute_async(script, timeout=timeout)
        if not isinstance(result, dict) or not result.get("ok"):
            raise RuntimeError(f"invoke {command} failed: {result}")
        return result.get("value")

    def screenshot(self) -> str:
        response = webdriver_request("GET", self.endpoint("/screenshot"), timeout=30)
        return str(response.get("value") or "")

    def quit(self) -> None:
        try:
            webdriver_request("DELETE", self.endpoint(), timeout=5)
        except Exception:
            pass


def wait_for_source_ready(session: WebDriverSession, notebook_id: str, source_id: str) -> dict[str, Any]:
    deadline = time.monotonic() + 60
    last_source: dict[str, Any] | None = None
    while time.monotonic() < deadline:
        sources = session.invoke("list_sources", {"notebookId": notebook_id}, timeout=30)
        for source in sources:
            if source.get("id") == source_id:
                last_source = source
                if source.get("status") == "ready":
                    return source
                if source.get("status") == "error":
                    raise RuntimeError(f"source ingestion failed: {source.get('error_message')}")
        time.sleep(1)
    raise TimeoutError(f"source did not become ready: {last_source}")


def wait_for_assistant_message(
    session: WebDriverSession,
    notebook_id: str,
    conversation_id: str,
    message_id: str,
) -> dict[str, Any]:
    deadline = time.monotonic() + 180
    last_messages: list[dict[str, Any]] = []
    while time.monotonic() < deadline:
        messages = session.invoke(
            "load_messages",
            {"notebookId": notebook_id, "conversationId": conversation_id},
            timeout=30,
        )
        last_messages = messages
        for message in messages:
            if (
                message.get("id") == message_id
                and message.get("role") == "assistant"
                and str(message.get("content") or "").strip()
            ):
                return message
        time.sleep(2)
    raise TimeoutError(f"assistant message did not arrive: {last_messages[-3:]}")


def parse_assistant_evidence(message: dict[str, Any]) -> tuple[list[dict[str, Any]], dict[str, Any]]:
    raw = message.get("citations")
    if not raw:
        return [], {}
    if isinstance(raw, str):
        payload = json.loads(raw)
    else:
        payload = raw
    citations = payload.get("citations") or []
    evidence = payload.get("evidence") or {}
    return citations, evidence


def make_receipt(
    repo: Path,
    receipt_path: Path,
    tools: dict[str, str | None],
    commands: list[dict[str, object]],
    blocked: bool,
    blockers: list[str],
    extra: dict[str, Any] | None = None,
) -> dict[str, Any]:
    receipt: dict[str, Any] = {
        "schema": "GlossP33DesktopSmokeReceiptV1",
        "run_id": RUN_ID,
        "recorded_time": utc_now(),
        "completed": False,
        "blocked": blocked,
        "blockers": blockers,
        "platform": {
            "os": platform.system(),
            "release": platform.release(),
            "machine": platform.machine(),
            "display": os.environ.get("DISPLAY"),
            "wayland_display": os.environ.get("WAYLAND_DISPLAY"),
            "xdg_session_type": os.environ.get("XDG_SESSION_TYPE"),
        },
        "tools": tools,
        "commands": commands,
        "app_launched": False,
        "source_ingested": False,
        "prompt_submitted": False,
        "response_non_empty": False,
        "chat_attempt_trace_captured": False,
        "citations": [],
        "retrieval_backend_used": None,
        "retrieval_mode": None,
        "screenshot_path": None,
        "stdout_log_path": None,
        "stderr_log_path": None,
        "release_implication": "release_ready must remain false until a full desktop RAG smoke receipt passes p33_desktop_smoke_gate.py",
    }
    if extra:
        receipt.update(extra)
    receipt_path.write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    print(json.dumps(receipt, indent=2))
    return receipt


def main() -> int:
    parser = argparse.ArgumentParser(description="Run and write P33 desktop smoke receipt.")
    parser.add_argument("--repo", default=".")
    parser.add_argument(
        "--receipt",
        default=f"docs/codex-runs/{RUN_ID}/desktop_smoke/final_desktop_smoke.json",
    )
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    receipt_path = repo / args.receipt
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    stdout_log = receipt_path.parent / "tauri_driver_stdout.log"
    stderr_log = receipt_path.parent / "tauri_driver_stderr.log"
    screenshot_path = receipt_path.parent / "final_desktop_smoke.png"

    env = dict(os.environ)
    env["PATH"] = f"{Path.home() / '.cargo' / 'bin'}:{env.get('PATH', '')}"
    tools = {
        "tauri_driver": tool_path("tauri-driver"),
        "webkit_webdriver": tool_path("WebKitWebDriver"),
        "edge_driver": tool_path("msedgedriver"),
        "xvfb_run": tool_path("xvfb-run"),
    }
    commands: list[dict[str, object]] = []
    blockers = []
    if not tools["tauri_driver"]:
        blockers.append("tauri-driver is not installed")
    if not tools["webkit_webdriver"]:
        blockers.append("WebKitWebDriver is not installed")
    if blockers:
        make_receipt(repo, receipt_path, tools, commands, True, blockers)
        return 1

    if not args.skip_build:
        commands.append(run(["npm", "run", "tauri", "--", "build", "--debug", "--no-bundle"], repo, env=env))
        if commands[-1]["returncode"] != 0:
            make_receipt(repo, receipt_path, tools, commands, True, ["debug Tauri build failed"])
            return 1

    app = repo / "src-tauri" / "target" / "debug" / "gloss"
    if not app.exists():
        make_receipt(repo, receipt_path, tools, commands, True, [f"debug app binary missing: {app}"])
        return 1

    tauri_driver: subprocess.Popen[str] | None = None
    session: WebDriverSession | None = None
    stdout_handle = stdout_log.open("w", encoding="utf-8")
    stderr_handle = stderr_log.open("w", encoding="utf-8")
    try:
        tauri_driver = subprocess.Popen(
            [tools["tauri_driver"] or "tauri-driver"],
            cwd=repo,
            text=True,
            stdout=stdout_handle,
            stderr=stderr_handle,
            env=env,
        )
        time.sleep(2)
        response = webdriver_request(
            "POST",
            "http://127.0.0.1:4444/session",
            {
                "capabilities": {
                    "alwaysMatch": {
                        "browserName": "wry",
                        "tauri:options": {"application": str(app)},
                    }
                }
            },
            timeout=30,
        )
        session_id = response.get("value", {}).get("sessionId")
        if not session_id:
            raise RuntimeError(f"WebDriver session failed: {response}")
        session = WebDriverSession("http://127.0.0.1:4444", session_id)
        time.sleep(4)
        body_text = session.execute("return document.body.innerText")
        if "Gloss" not in str(body_text):
            raise RuntimeError(f"Gloss UI did not launch: {body_text!r}")

        session.invoke("update_provider", {"id": "ollama", "enabled": True, "baseUrl": "http://localhost:11434"})
        session.invoke("refresh_models", {"providerId": "ollama"}, timeout=120)
        session.invoke("update_setting", {"key": "default_provider", "value": "ollama"})
        session.invoke("update_setting", {"key": "default_model", "value": SMOKE_MODEL})
        session.invoke("update_setting", {"key": "memory_backend", "value": "gloss-local"})

        notebook_id = session.invoke(
            "create_notebook",
            {"name": f"P33 desktop smoke {utc_now()}"},
            timeout=30,
        )
        session.invoke("set_active_notebook", {"notebookId": notebook_id}, timeout=30)
        source_id = session.invoke(
            "add_source_paste",
            {
                "notebookId": notebook_id,
                "title": SMOKE_SOURCE_TITLE,
                "text": SMOKE_SOURCE_TEXT,
            },
            timeout=30,
        )
        source = wait_for_source_ready(session, notebook_id, source_id)
        conversation_id = session.invoke("create_conversation", {"notebookId": notebook_id}, timeout=30)
        message_id = f"p33-desktop-smoke-{int(time.time())}"
        session.invoke(
            "send_message",
            {
                "notebookId": notebook_id,
                "conversationId": conversation_id,
                "query": SMOKE_PROMPT,
                "sourceScope": {"kind": "all"},
                "model": SMOKE_MODEL,
                "messageId": message_id,
            },
            timeout=60,
        )
        assistant = wait_for_assistant_message(session, notebook_id, conversation_id, message_id)
        trace = session.invoke("get_last_chat_attempt_trace", {}, timeout=30)
        citations, evidence = parse_assistant_evidence(assistant)
        screenshot_b64 = session.screenshot()
        if screenshot_b64:
            screenshot_path.write_bytes(base64.b64decode(screenshot_b64))

        completed = all(
            [
                str(assistant.get("content") or "").strip(),
                trace and trace.get("schema") == "ChatAttemptTraceV1",
                citations,
                evidence.get("backend_used"),
                evidence.get("retrieval_mode"),
            ]
        )
        extra = {
            "completed": completed,
            "blocked": not completed,
            "blockers": [] if completed else ["desktop smoke completed without required citation/evidence fields"],
            "app_launched": True,
            "source_ingested": source.get("status") == "ready",
            "prompt_submitted": True,
            "response_non_empty": bool(str(assistant.get("content") or "").strip()),
            "chat_attempt_trace_captured": bool(trace and trace.get("schema") == "ChatAttemptTraceV1"),
            "provider": "ollama",
            "model": SMOKE_MODEL,
            "notebook_id": notebook_id,
            "source_id": source_id,
            "source_status": source.get("status"),
            "conversation_id": conversation_id,
            "message_id": message_id,
            "assistant_response_preview": str(assistant.get("content") or "")[:500],
            "chat_attempt_trace_ref": str(Path.home() / ".local/share/gloss/chat-attempt-traces/latest.json"),
            "chat_attempt_trace": trace,
            "citations": citations,
            "retrieval_backend_requested": evidence.get("backend_requested"),
            "retrieval_backend_used": evidence.get("backend_used"),
            "retrieval_mode": evidence.get("retrieval_mode"),
            "retrieval_receipt_id": evidence.get("receipt_id"),
            "retrieval_outcome": evidence.get("retrieval_outcome"),
            "screenshot_path": str(screenshot_path.relative_to(repo)) if screenshot_b64 else None,
            "stdout_log_path": str(stdout_log.relative_to(repo)),
            "stderr_log_path": str(stderr_log.relative_to(repo)),
            "release_implication": "desktop RAG smoke proof captured; release readiness still requires package replay and final gate",
        }
        receipt = make_receipt(repo, receipt_path, tools, commands, not completed, extra["blockers"], extra)
        return 0 if receipt.get("completed") else 1
    except Exception as exc:
        make_receipt(
            repo,
            receipt_path,
            tools,
            commands,
            True,
            [str(exc)],
            {
                "stdout_log_path": str(stdout_log.relative_to(repo)),
                "stderr_log_path": str(stderr_log.relative_to(repo)),
            },
        )
        return 1
    finally:
        if session is not None:
            session.quit()
        if tauri_driver is not None:
            tauri_driver.terminate()
            try:
                tauri_driver.wait(timeout=5)
            except subprocess.TimeoutExpired:
                tauri_driver.kill()
        stdout_handle.close()
        stderr_handle.close()


if __name__ == "__main__":
    raise SystemExit(main())
