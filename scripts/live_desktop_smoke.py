#!/usr/bin/env python3
"""Real Linux Tauri/WebDriver workflows in disposable data directories.

Builds and drives the native app. By default it exits 2 while full acceptance
cases remain unobserved. --require-baseline enforces only its two real UI cases
for CI without changing the incomplete release receipt. --require-integrated
requires all twelve real UI cases against an owned Ollama runtime. A source-bound
--prebuilt-config replays the extracted AppImage launcher in place. No IPC is mocked.
"""
from __future__ import annotations

import argparse
import base64
from datetime import datetime, timezone
import http.client
import json
import ipaddress
import os
from pathlib import Path
import re
import shutil
import socket
import subprocess
import sys
import time
import traceback
import urllib.error
import urllib.parse
import uuid

SCRIPT_DIR = str(Path(__file__).resolve().parent)
if SCRIPT_DIR not in sys.path:
    sys.path.insert(0, SCRIPT_DIR)
from gloss_desktop_smoke_harness import LIVE_SCHEMA, REQUIRED_LIVE_CASES, file_sha256
from source_snapshot import capture_source_identity

BASELINE_CASES = ("startup_idle", "notebook_crud_restart")
OLLAMA_CONFIG_SCHEMA = "gloss-desktop-ollama-config/v1"
ELEMENT_KEY = "element-6066-11e4-a52e-4f735466cecf"
MESSAGE_EVIDENCE_SELECTOR = '.gloss-assistant-bubble button[title="Evidence"][aria-controls^="evidence-"]'


def load_ollama_config(path: Path) -> dict:
    return validate_ollama_config(json.loads(path.read_text()))


def validate_ollama_config(config: dict) -> dict:
    """Accept only the owned loopback runtime/model snapshot, never credentials."""
    keys = {"schema", "provider", "base_url", "runtime_version", "chat_model", "embedding_model",
            "chat_model_digest", "embedding_model_digest"}
    if not isinstance(config, dict) or set(config) != keys:
        raise ValueError("desktop Ollama config has missing or unrecognized fields")
    if config["schema"] != OLLAMA_CONFIG_SCHEMA or config["provider"] != "ollama":
        raise ValueError("desktop config must identify the owned Ollama runtime")
    if any(not isinstance(value, str) or not value.strip() for value in config.values()):
        raise ValueError("desktop config fields must be nonempty strings")
    url = urllib.parse.urlsplit(config["base_url"])
    try:
        local = ipaddress.ip_address(url.hostname or "").is_loopback
        port = url.port
    except ValueError:
        local, port = False, None
    if url.scheme != "http" or not local or not port or url.username or url.password or url.query or url.fragment or url.path not in ("", "/"):
        raise ValueError("desktop config requires an explicit HTTP loopback IP and port")
    for key in ("chat_model_digest", "embedding_model_digest"):
        if not re.fullmatch(r"(?:sha256:)?[0-9a-f]{64}", config[key]):
            raise ValueError(f"{key} must be a complete SHA-256 digest")
    return config


def load_prebuilt_config(path: Path, source: dict) -> dict:
    config = json.loads(path.read_text())
    keys = {"schema", "source", "application", "application_sha256", "binary", "binary_sha256",
            "artifact_sha256", "build_command", "build_log", "build_exit_code"}
    if not isinstance(config, dict) or set(config) != keys or config.get("schema") != "gloss-desktop-prebuilt/v1":
        raise ValueError("invalid prebuilt desktop config schema/fields")
    if config["source"] != source or not source.get("worktree_clean"):
        raise ValueError("prebuilt application must match the current clean source snapshot")
    if type(config["build_exit_code"]) is not int or config["build_exit_code"] != 0:
        raise ValueError("prebuilt application requires a successful build")
    if not isinstance(config["build_command"], list) or not config["build_command"] or not all(isinstance(item, str) and item for item in config["build_command"]):
        raise ValueError("prebuilt application build command missing")
    for key in ("application", "binary", "build_log"):
        if not isinstance(config[key], str) or not Path(config[key]).is_absolute() or not Path(config[key]).is_file():
            raise ValueError(f"prebuilt {key} must be an existing absolute file")
    for key in ("application", "binary"):
        if not isinstance(config[key + "_sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", config[key + "_sha256"]) or file_sha256(Path(config[key])) != config[key + "_sha256"]:
            raise ValueError(f"prebuilt {key} digest mismatch")
    if not isinstance(config["artifact_sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", config["artifact_sha256"]):
        raise ValueError("prebuilt AppImage archive digest missing")
    return config


class BaselineBlocked(Exception):
    """A required capability is absent before native execution."""


def result_exit_code(receipt: dict, require_baseline: bool = False, require_integrated: bool = False) -> int:
    """Do not convert a generic partial/blocked receipt into CI success."""
    if receipt.get("status") == "fail":
        return 1
    cases = receipt.get("cases", [])
    if receipt.get("status") not in ("pass", "blocked") or not isinstance(cases, list) or not all(isinstance(case, dict) for case in cases):
        return 2
    if require_integrated:
        try:
            validate_ollama_config(receipt.get("ollama_config"))
        except (ValueError, TypeError):
            return 2
        source, build = receipt.get("source"), receipt.get("build")
        if not isinstance(source, dict) or source.get("worktree_clean") is not True or not isinstance(build, dict):
            return 2
        if build.get("source") != source or type(build.get("exit_code")) is not int or build["exit_code"] != 0 or not isinstance(build.get("command"), list) or not build["command"]:
            return 2
        for field in ("binary", "log"):
            artifact = build.get(field)
            if not isinstance(artifact, dict) or not isinstance(artifact.get("path"), str) or not artifact["path"] or not re.fullmatch(r"[0-9a-f]{64}", str(artifact.get("sha256", ""))):
                return 2
        return 0 if (
            receipt.get("live_desktop_exercised") is True
            and receipt.get("integrated_status") == "pass"
            and all(receipt.get(flag) is False for flag in ("source_scope_widened", "hidden_fallback", "raw_uuid_flood"))
            and isinstance(receipt.get("ollama_config"), dict)
            and len(cases) == len(REQUIRED_LIVE_CASES)
            and all(sum(case.get("id") == case_id for case in cases) == 1
                    and any(case.get("id") == case_id and case.get("status") == "pass" for case in cases)
                    for case_id in REQUIRED_LIVE_CASES)
        ) else 2
    if not require_baseline:
        return 2
    cases = receipt.get("cases", [])
    actual = [case.get("id") for case in cases if case.get("status") == "pass"]
    return 0 if (
        receipt.get("live_desktop_exercised") is True
        and receipt.get("baseline_status") == "pass"
        and all(actual.count(case_id) == 1 for case_id in BASELINE_CASES)
    ) else 2


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


def evidence(path: Path, root: Path, kind: str) -> dict:
    return {"path": str(path.relative_to(root)), "sha256": file_sha256(path), "kind": kind}


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return sock.getsockname()[1]


class WebDriver:
    def __init__(self, port: int):
        self.port = port
        self.session: str | None = None
        self.trace: list[dict] = []

    def request(self, method: str, path: str, payload: dict | None = None):
        # urllib forces Connection: close, which the Tauri proxy forwards to
        # its pooled native WebDriver connection. Use an explicit loopback
        # connection without that hop-by-hop header or ambient proxy settings.
        connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=30)
        observation = {"at": now(), "method": method, "path": path, "request": payload}
        self.trace.append(observation)
        try:
            connection.request(method, path,
                               body=json.dumps(payload).encode() if payload is not None else None,
                               headers={"Content-Type": "application/json"})
            response = connection.getresponse()
            body = response.read()
            if response.status >= 400:
                raise RuntimeError(f"WebDriver {method} {path}: HTTP {response.status}: {body.decode(errors='replace')}")
            result = json.loads(body)
        except Exception as error:
            # A failed POST may already have acted. Preserve the attempted
            # command and propagate the failure without replaying the mutation.
            observation["error"] = f"{type(error).__name__}: {error}"
            raise
        finally:
            connection.close()
        value = result.get("value")
        observation["response"] = "PNG captured" if path.endswith("/screenshot") else value
        if isinstance(value, dict) and value.get("error"):
            raise RuntimeError(f"WebDriver: {value}")
        return value

    def start(self, application: Path):
        result = self.request("POST", "/session", {"capabilities": {"alwaysMatch": {
            "tauri:options": {"application": str(application)},
        }}})
        self.session = result["sessionId"]

    def call(self, method: str, path: str, payload: dict | None = None):
        if self.session is None:
            raise RuntimeError("no native WebDriver session")
        return self.request(method, f"/session/{self.session}{path}", payload)

    def execute(self, script: str, args: list | None = None):
        return self.call("POST", "/execute/sync", {"script": script, "args": args or []})

    def wait(self, condition, timeout: int = 30, label: str = "native UI condition"):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = condition()
            if value:
                return value
            time.sleep(0.2)
        raise RuntimeError(f"{label} timed out after {timeout}s")

    def element(self, selector: str) -> str:
        value = self.call("POST", "/element", {"using": "css selector", "value": selector})
        return value["element-6066-11e4-a52e-4f735466cecf"]

    def click(self, selector: str):
        self.call("POST", f"/element/{self.element(selector)}/click", {})

    def click_ref(self, element: dict):
        self.call("POST", f"/element/{element[ELEMENT_KEY]}/click", {})

    def find_visible(self, selector: str, text: str | None = None, last: bool = False):
        return self.execute("""const nodes=Array.from(document.querySelectorAll(arguments[0])).filter(e=>e.getClientRects().length && (!arguments[1] || e.textContent.trim()===arguments[1])); return nodes[arguments[2] ? nodes.length-1 : 0] || null""", [selector, text, last])

    def click_text(self, text: str, selector: str = "button", last: bool = False):
        element = self.wait(lambda: self.find_visible(selector, text, last), label=f"visible {text}")
        self.click_ref(element)

    def fill(self, selector: str, value: str):
        element = self.wait(lambda: self.find_visible(selector), label=f"visible {selector}")
        identifier = element[ELEMENT_KEY]
        self.call("POST", f"/element/{identifier}/click", {})
        # Actual keyboard input updates React through the native WebDriver.
        self.call("POST", f"/element/{identifier}/value", {"text": "\ue009a\ue000\ue003"})
        if value:
            self.call("POST", f"/element/{identifier}/value", {"text": value})

    def select(self, selector: str, value: str):
        element = self.wait(lambda: self.execute("""const s=document.querySelector(arguments[0]); return s && s.getClientRects().length ? Array.from(s.options).find(o=>o.value===arguments[1] && !o.disabled) || null : null""", [selector, value]), label=f"selectable {value}")
        self.click_ref(element)
        self.wait(lambda: self.execute("return document.querySelector(arguments[0])?.value===arguments[1]", [selector, value]), label=f"selected {value}")

    def text(self, selector: str = "body") -> str:
        return self.execute("return document.querySelector(arguments[0])?.innerText || ''", [selector])

    def snapshot(self, name: str, root: Path) -> Path:
        path = root / f"{name}-dom.json"
        state = self.execute("""return {text:document.body.innerText, controls:Array.from(document.querySelectorAll('input,select,textarea,button')).filter(e=>e.getClientRects().length).map(e=>({tag:e.tagName,label:e.getAttribute('aria-label'),title:e.title,text:e.innerText,value:e.value,disabled:e.disabled,checked:e.checked}))}""")
        path.write_text(json.dumps(state, indent=2) + "\n")
        return path

    def stop(self):
        if self.session:
            try:
                self.request("DELETE", f"/session/{self.session}")
            finally:
                self.session = None

    def case(self, case_id: str, observation: str, root: Path) -> dict:
        dom = self.snapshot(case_id, root)
        screenshot = root / f"{case_id}.png"
        screenshot.write_bytes(base64.b64decode(self.call("GET", "/screenshot"), validate=True))
        log = root / f"{case_id}.json"
        log.write_text(json.dumps(self.trace, indent=2) + "\n")
        return {"id": case_id, "status": "pass", "observation": observation,
                "evidence": [evidence(log, root, "runtime_log"), evidence(screenshot, root, "screenshot"), evidence(dom, root, "dom_snapshot")]}


def require_scope_evidence(values: dict, selected: int, excluded: int, retrieval: bool, *, degraded: bool = False) -> None:
    """Receipt assertions consume only values rendered by the actual UI."""
    scope = values.get("Scope", "")
    if f"{selected} selected, {excluded} excluded, 0 invalid" not in scope:
        raise RuntimeError(f"rendered retrieval scope differs from UI selection: {scope!r}")
    context = values.get("Context", "")
    if "preserved: yes" not in context:
        raise RuntimeError(f"rendered source scope was not preserved: {context!r}")
    match = re.match(r"(\d+) passages", context)
    if not match or (int(match[1]) > 0) != retrieval:
        raise RuntimeError(f"unexpected context passages: {context!r}")
    if values.get("Backend requested") != "gloss-local":
        raise RuntimeError("native workflow changed its explicitly selected Gloss local backend")
    used, mode, fallback = values.get("Backend used"), values.get("Retrieval"), values.get("Fallback")
    expected = {("gloss-local", "bm25_only")} if degraded else (
        {("native-hybrid", "hybrid_rrf"), ("native-dense", "dense_only")} if retrieval else {("gloss-local", "gloss-local")}
    )
    if (used, mode) not in expected:
        raise RuntimeError(f"unexpected native retrieval capability: backend={used!r}, mode={mode!r}, degraded={degraded}")
    if degraded:
        if not isinstance(fallback, str) or not re.search(r"dense|embed|stale|index", fallback, re.I):
            raise RuntimeError(f"expected dense-index degradation was not disclosed: {fallback!r}")
    elif fallback != "no":
        raise RuntimeError(f"healthy native workflow reported fallback: {fallback!r}")


class IntegratedWorkflow:
    """Real element/keyboard actions and rendered receipt checks, with no IPC access."""
    def __init__(self, driver: WebDriver, application: Path, root: Path, config: dict, receipt: dict):
        self.ui, self.application, self.root, self.config, self.receipt = driver, application, root, config, receipt
        self.case_id = "model_dropdown_and_prompt"
        self.scope_checks: list[dict] = []
        self.default_surfaces: list[dict] = []
        self.conversations: dict[str, str] = {}

    def check(self, condition: bool, description: str):
        if not condition:
            raise RuntimeError(f"{self.case_id}: {description}")
        self.ui.trace.append({"at": now(), "assertion": description, "result": "pass"})

    def record(self, case_id: str, description: str):
        self.receipt["cases"].append(self.ui.case(case_id, description, self.root))

    def focus_chat(self):
        self.ui.click('button[aria-label="Focus chat message"]')

    def sources(self):
        if self.ui.find_visible('button[aria-label="Open sources"]'):
            self.ui.click('button[aria-label="Open sources"]')

    def inspector(self, tab: str):
        if self.ui.find_visible('button[aria-label="Open inspector"]'):
            self.ui.click('button[aria-label="Open inspector"]')
        self.ui.click(f'button[aria-label="Inspector tab: {tab}"]')

    def create_notebook(self, name: str):
        self.remember_conversation()
        self.ui.click('button[title="Create notebook"]')
        self.ui.fill('input[aria-label="New notebook name"]', name + "\ue007")
        self.wait_active_notebook(name)

    def wait_active_notebook(self, name: str):
        self.ui.wait(lambda: self.ui.find_visible('button[aria-current="page"]:not(:disabled)', name), label=f"confirmed active notebook {name}")
        self.ui.wait(lambda: self.ui.find_visible('textarea[aria-label="Chat message"]'), label="active notebook chat")

    def remember_conversation(self):
        current = self.ui.execute("return {notebook: document.querySelector('button[aria-current=\"page\"]')?.textContent, conversation: document.querySelector('select[aria-label=\"Conversation\"]')?.value}")
        if current.get("notebook") and current.get("conversation"):
            self.conversations[current["notebook"]] = current["conversation"]

    def activate_notebook(self, notebook: str):
        self.ui.click_text(notebook)
        self.wait_active_notebook(notebook)
        if notebook in self.conversations:
            # Conversation selection itself is intentionally not persisted by
            # the product. Reopen the previously observed conversation through
            # its actual dropdown before checking durable message content.
            self.ui.select('select[aria-label="Conversation"]', self.conversations[notebook])

    def restart(self, notebook: str):
        self.remember_conversation()
        self.ui.stop()
        self.ui.start(self.application)
        self.ui.wait(lambda: self.ui.find_visible("button:not(:disabled)", notebook), label="notebook after native restart")
        self.activate_notebook(notebook)

    def settings(self):
        self.ui.click_text("Settings")
        self.ui.wait(lambda: self.ui.find_visible('input[aria-label="Ollama server URL"]'), label="settings dialog")

    def close_settings(self):
        self.ui.click_ref(self.ui.execute("return Array.from(document.querySelectorAll('h2')).find(e=>e.textContent==='Settings').parentElement.querySelector('button')"))
        self.ui.wait(lambda: not self.ui.find_visible('input[aria-label="Ollama server URL"]'), label="settings closed")

    def embedding_settings(self, model: str):
        self.ui.select('select[aria-label="Embedding backend"]', "ollama")
        for label, value in (("Embedding URL", self.config["base_url"]), ("Embedding model", model),
                             ("Embedding timeout seconds", "60"), ("Search timeout milliseconds", "120000"),
                             ("Chunk target tokens", "300")):
            self.ui.fill(f'input[aria-label="{label}"]', value)
        self.ui.click_text("Apply embedding and ingestion settings")
        self.ui.wait(lambda: self.ui.execute("return Array.from(document.querySelectorAll('button')).some(e=>e.textContent==='Apply embedding and ingestion settings' && e.disabled)"), label="embedding Apply acknowledged")
        self.check("Unsaved changes. Apply" not in self.ui.text(), "embedding settings have no unacknowledged draft")

    def configure(self):
        self.settings()
        self.ui.fill('input[aria-label="Ollama server URL"]', self.config["base_url"])
        self.ui.click_ref(self.ui.execute("return document.querySelector('input[aria-label=\"Ollama server URL\"]').parentElement.querySelector('button')"))
        self.ui.wait(lambda: self.ui.execute("return document.querySelector('input[aria-label=\"Ollama server URL\"]').parentElement.querySelector('button').disabled"), label="Ollama URL saved")
        self.ui.select('select:has(option[value="gloss-local"])', "gloss-local")
        def profile_applied():
            text = self.ui.text()
            self.check("Memory profile not applied" not in text and "Memory profile blocked" not in text,
                       "memory profile Apply succeeded")
            return "Memory profile applied" in text
        self.ui.wait(profile_applied, label="acknowledged memory profile Apply")
        self.embedding_settings(self.config["embedding_model"])
        self.close_settings()
        self.ui.click('button[title="Refresh model list from providers"]')
        self.ui.select('select[aria-label="Chat model and provider"]', "ollama::" + self.config["chat_model"])
        self.ui.select('select[aria-label="Response length"]', "short")
        self.ui.select('select[aria-label="Conversational style"]', "custom")
        self.ui.fill('input[aria-label="Custom conversation goal"]', "Be concise. Quote exact source facts and cite them using [1]. /no_think")
        self.ui.wait(lambda: "Saving model selection" not in self.ui.text(), label="chat model saved")
        self.settings()
        for label, expected in (("Ollama server URL", self.config["base_url"]), ("Embedding URL", self.config["base_url"]),
                                ("Embedding model", self.config["embedding_model"]), ("Embedding timeout seconds", "60")):
            self.check(self.ui.execute("return document.querySelector(arguments[0]).value", [f'input[aria-label="{label}"]']) == expected, f"reopened {label} matches applied value")
        self.ui.snapshot("applied-settings", self.root)
        self.close_settings()

    def answer_id(self) -> str | None:
        # The list is virtualized: rendered row counts are not monotonic.
        # Observe the last actual message's existing disclosure target instead.
        return self.ui.execute("return Array.from(document.querySelectorAll(arguments[0])).at(-1)?.getAttribute('aria-controls') || null", [MESSAGE_EVIDENCE_SELECTOR])

    def last_answer(self) -> str:
        return self.ui.execute("return Array.from(document.querySelectorAll('.gloss-assistant-bubble .prose')).at(-1)?.innerText || ''")

    def send(self, question: str, expected: str | None = None) -> str:
        self.focus_chat()
        previous = self.answer_id()
        self.ui.fill('textarea[aria-label="Chat message"]', question + " /no_think")
        self.ui.click('button[aria-label="Send message"]')
        self.ui.wait(lambda: self.answer_id() and self.answer_id() != previous and not self.ui.find_visible('button[aria-label="Stop generation"]'), timeout=180, label="persisted assistant answer")
        answer = self.last_answer()
        self.check(bool(answer.strip()), "assistant response is nonempty")
        if expected:
            self.check(expected in answer, f"assistant contains fixture fact {expected}")
        self.default_surface("answer")
        return answer

    def evidence(self, selected: int, excluded: int, retrieval: bool, *, degraded: bool = False) -> dict:
        button = self.ui.find_visible(MESSAGE_EVIDENCE_SELECTOR, last=True)
        drawer_id = self.ui.execute("return arguments[0].getAttribute('aria-controls')", [button])
        if not self.ui.execute("return arguments[0].getAttribute('aria-expanded')==='true'", [button]):
            self.ui.click_ref(button)
        self.ui.wait(lambda: self.ui.execute("return document.getElementById(arguments[0])?.getClientRects().length>0", [drawer_id]), label="message evidence disclosure")
        values = self.ui.execute("""const r=document.getElementById(arguments[0]); const g=r.querySelector('.grid'); return Object.fromEntries(Array.from(g.children).map(e=>[e.children[0].textContent.replace(/:\\s*$/, '').trim(), e.children[1].textContent.trim()]));""", [drawer_id])
        require_scope_evidence(values, selected, excluded, retrieval, degraded=degraded)
        self.check(values.get("Generation") == "completed", "rendered generation receipt is complete")
        self.scope_checks.append(values)
        self.ui.snapshot(f"evidence-{len(self.scope_checks)}", self.root)
        return values

    def default_surface(self, label: str):
        # Only normal answer/source/notebook text is checked. Explicitly opened
        # receipt inspectors intentionally display identifiers and are excluded.
        text = self.ui.execute("return Array.from(document.querySelectorAll('.gloss-assistant-bubble .prose, p[title]')).filter(e=>e.getClientRects().length).map(e=>e.innerText).join('\\n')")
        found = re.findall(r"\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b", text, re.I)
        self.check(not found, "normal answer/source presentation contains no raw UUIDs")
        self.default_surfaces.append({"label": label, "text": text, "uuid_count": len(found)})

    def source_rows(self) -> list[dict]:
        return self.ui.execute("""return Array.from(document.querySelectorAll('p[title]')).filter(e=>e.getClientRects().length && e.parentElement.parentElement.querySelector('button[title="Reindex for semantic-memory preview"]')).map(e=>({title:e.title,text:e.parentElement.innerText,status:e.nextElementSibling?.querySelector('span')?.textContent,error:e.nextElementSibling?.querySelector('span[title]')?.title || null}));""")

    def wait_sources(self, titles: set[str], ready: bool = True):
        self.sources()
        self.ui.wait(lambda: {row["title"] for row in self.source_rows()} == titles and (not ready or all(row["status"] == "ready" for row in self.source_rows())), timeout=180, label=f"sources {sorted(titles)} {'ready' if ready else 'visible'}")

    def choose_source(self, title: str):
        self.sources()
        self.ui.click_text("None")
        row = self.ui.execute("return Array.from(document.querySelectorAll('p[title]')).find(e=>e.title===arguments[0]).parentElement.parentElement", [title])
        self.ui.click_ref(self.ui.execute("return arguments[0].querySelector('button')", [row]))

    def paste(self, title: str, content: str):
        self.sources()
        self.ui.click_text("Paste")
        self.ui.fill('input[placeholder="Title (optional)"]', title)
        self.ui.fill('textarea[placeholder="Paste text here..."]', content)
        self.ui.click_text("Add Source")
        self.ui.wait(lambda: not self.ui.find_visible('textarea[placeholder="Paste text here..."]'), label="paste accepted")

    def folder(self, folder: Path):
        if not shutil.which("xdotool"):
            raise BaselineBlocked("xdotool is required to observe the real native folder chooser")
        self.sources()
        self.ui.click_text("Folder")
        def native(*args: str) -> str:
            result = subprocess.run(["xdotool", *args], capture_output=True, text=True, timeout=10, check=True)
            self.ui.trace.append({"at": now(), "native_ui_command": ["xdotool", *args], "stdout": result.stdout, "stderr": result.stderr, "exit_code": result.returncode})
            return result.stdout.strip()
        # Keyboard selection operates on the actual GTK chooser opened by the
        # Folder button. No dialog result, file path, store or IPC is injected.
        native("search", "--sync", "--onlyvisible", "--name", "Select|Open|Choose")
        name = native("getwindowfocus", "getwindowname")
        self.check(bool(re.search(r"select|open|choose", name, re.I)), f"native folder chooser focused: {name}")
        native("key", "--clearmodifiers", "ctrl+l")
        native("type", "--clearmodifiers", "--delay", "1", str(folder))
        native("key", "--clearmodifiers", "Return")
        time.sleep(0.3)
        # GTK may navigate to the directory first. Activating the dialog's
        # default action after navigation selects that directory.
        if re.search(r"select|open|choose", native("getwindowfocus", "getwindowname"), re.I):
            native("key", "--clearmodifiers", "Return")

    def run(self, name: str):
        self.create_notebook(name)
        self.configure()
        self.case_id = "chat_no_retrieval"
        greeting = self.send("Reply with exactly HELLO_GLOSS.", "HELLO_GLOSS")
        self.evidence(0, 0, False)
        self.record(self.case_id, "Configured the real loopback Ollama chat and embedding models through saved UI settings. With no sources, a real model response persisted with zero context passages and no backend fallback.")

        self.case_id = "chat_persistence_restart"
        self.restart(name)
        self.ui.wait(lambda: greeting == self.last_answer(), label="exact chat content after restart")
        self.check(self.ui.execute("return document.querySelector('select[aria-label=\"Chat model and provider\"]').value") == "ollama::" + self.config["chat_model"], "exact provider/model restored after restart")
        self.record(self.case_id, "Restarted the actual native process and observed identical persisted assistant content and the selected Ollama model.")

        self.case_id = "model_dropdown_and_prompt"
        self.inspector("Prompt")
        panel = self.ui.text("#inspector-panel-prompt")
        self.check(self.config["chat_model"] in panel and "ollama" in panel.lower(), "captured prompt panel identifies actual model and provider")
        self.check("System Prompt" in panel and "Copy full prompt" in panel, "full captured system prompt is visible")
        self.record(self.case_id, "Selected an installed Ollama model through the dropdown, observed its persistence after restart, and opened the actual captured full system prompt with model/provider receipts.")

        self.case_id = "chat_cancel_and_retry"
        self.focus_chat()
        before = self.answer_id()
        saved_answer = self.last_answer()
        self.ui.fill('textarea[aria-label="Chat message"]', "Write a long numbered list of 100 detailed facts about oceanography. /no_think")
        self.ui.click('button[aria-label="Send message"]')
        self.ui.wait(lambda: self.ui.find_visible('button[aria-label="Stop generation"]') and self.ui.execute("return Array.from(document.querySelectorAll('.gloss-assistant-bubble pre')).some(e=>e.innerText.trim().length>0)"), timeout=180, label="actual streamed model tokens before cancellation")
        self.ui.snapshot("stream-before-cancel", self.root)
        self.ui.click('button[aria-label="Stop generation"]')
        self.ui.wait(lambda: not self.ui.find_visible('button[aria-label="Stop generation"]') and self.ui.find_visible('[role="alert"]', "Chat cancelled by user"), label="explicit cancelled terminal event")
        self.check(self.answer_id() == before, "cancelled generation is not presented as a completed assistant answer")
        self.ui.snapshot("cancelled-terminal", self.root)
        self.restart(name)
        self.ui.wait(lambda: self.last_answer() == saved_answer and self.answer_id() == before, label="prior saved answer preserved after cancellation and restart")
        self.focus_chat()
        # Edit-and-rerun is the explicit UI retry action, bounded to a short
        # replacement prompt so completion rather than a second cancel is proven.
        self.ui.click_ref(self.ui.find_visible('button[title="Edit and rerun"]', last=True))
        self.ui.fill('textarea[aria-label="Chat message"]', "Reply with exactly RETRY_GLOSS. /no_think")
        previous = self.answer_id()
        self.ui.click('button[aria-label="Rerun edited message"]')
        self.ui.wait(lambda: self.answer_id() and self.answer_id() != previous and not self.ui.find_visible('button[aria-label="Stop generation"]'), timeout=180, label="explicit edit-and-rerun complete")
        self.check("RETRY_GLOSS" in self.last_answer(), "explicit retry produced a completed model response")
        self.evidence(0, 0, False)
        self.record(self.case_id, "Observed real streamed tokens, stopped through the UI, saw the explicit cancellation alert without a new completed answer, restarted and verified the prior saved answer, then used Edit and rerun for a successful explicit retry.")

        self.case_id = "folder_import_scope"
        folder = self.root / "folder-fixture"
        (folder / "facts").mkdir(parents=True)
        (folder / "other").mkdir()
        (folder / "facts" / "atlas.md").write_text("# Atlas launch plan\nThe Atlas launch code is GLACIER-ORBIT-417. The launch code GLACIER-ORBIT-417 is the exact approved Atlas code. The Atlas team meets on Tuesday in the north observatory.\n")
        (folder / "other" / "excluded.md").write_text("# Excluded notebook source\nThe unrelated project code is EXCLUDED-COPPER-829. This source is excluded from the Atlas question and must not enter its answer context.\n")
        outside = self.root / "outside-not-imported.md"
        outside.write_text("Outside folder boundary secret OUTSIDE-SILVER-936 must never be imported.\n")
        (folder / "outside-symlink.md").symlink_to(outside)
        self.folder(folder)
        titles = {"facts/atlas.md", "other/excluded.md"}
        self.wait_sources(titles)
        self.choose_source("facts/atlas.md")
        self.default_surface("folder-import")
        self.record(self.case_id, "Used the real native folder chooser to import two nested files. Only the exact fixture-relative titles appeared; the symlink outside the selected directory did not become a source. Selected only Atlas for retrieval.")

        self.case_id = "citation_evidence"
        answer = self.send("What is the exact Atlas launch code? Answer using the selected source and cite [1].", "GLACIER-ORBIT-417")
        self.check("EXCLUDED-COPPER-829" not in answer and "OUTSIDE-SILVER-936" not in answer, "excluded and outside fixture facts absent from answer")
        values = self.evidence(1, 1, True)
        self.check(re.match(r"[1-9]\d* valid, 0 filtered", values.get("Citations", "")) is not None, "citation evidence has valid source references")
        citation = self.ui.wait(lambda: self.ui.execute("return Array.from(document.querySelectorAll('.gloss-assistant-bubble button')).filter(e=>e.getClientRects().length && e.textContent.includes('facts/atlas.md')).at(-1) || null"), label="Atlas citation button")
        self.ui.click_ref(citation)
        self.ui.wait(lambda: "Source Viewer" in self.ui.text() and "GLACIER-ORBIT-417" in self.ui.text(), label="cited source viewer")
        self.record(self.case_id, "A real scoped model answer cited Atlas, the rendered receipt reported one selected/one excluded source with preserved context, and clicking the citation opened the actual source text.")
        self.ui.click_ref(self.ui.execute("return Array.from(document.querySelectorAll('span')).find(e=>e.textContent==='Source Viewer').parentElement.parentElement.parentElement.querySelector('button')"))

        self.case_id = "retrieval_backend_and_degradation"
        self.settings()
        self.embedding_settings("gloss-nonexistent-embedding-fixture:missing")
        self.close_settings()
        self.paste("Recovery fixture", "Recoverable source text must survive a failed embedding call. The recovery marker is RECOVERY-AMBER-528.")
        titles.add("Recovery fixture")
        self.wait_sources(titles, ready=False)
        self.ui.wait(lambda: any(row["title"] == "Recovery fixture" and row["status"] == "error" for row in self.source_rows()), timeout=180, label="visible failed source import")
        original_error = next(row["error"] for row in self.source_rows() if row["title"] == "Recovery fixture")
        self.ui.snapshot("embedding-failure", self.root)
        self.choose_source("facts/atlas.md")
        self.send("What is the Atlas launch code? Cite the selected text.", "GLACIER-ORBIT-417")
        degraded = self.evidence(1, 2, True, degraded=True)
        self.check("100%" not in degraded.get("Dense coverage", "") and re.search(r"bm25|degrad|stale|blocked|missing|unavailable", " ".join(degraded.values()), re.I) is not None, "rendered retrieval receipt discloses reduced dense capability")
        self.ui.snapshot("degraded-native-retrieval", self.root)
        self.settings()
        self.embedding_settings(self.config["embedding_model"])
        self.close_settings()
        self.sources()
        recovery = self.ui.execute("return Array.from(document.querySelectorAll('p[title]')).find(e=>e.title==='Recovery fixture').parentElement.parentElement")
        self.ui.click_ref(self.ui.execute("return arguments[0].querySelector('button[title=\"Retry ingestion\"]')", [recovery]))
        self.ui.wait(lambda: any(row["title"] == "Recovery fixture" and (row["status"] == "ready" or (row["status"] == "error" and row["error"] != original_error)) for row in self.source_rows()), timeout=180, label="explicit retry produced a new terminal source state")
        self.inspector("Health")
        self.ui.click_text("Rebuild dense index")
        self.ui.wait(lambda: "Dense index ready:" in self.ui.text("#inspector-panel-diagnostics"), timeout=180, label="explicit native index rebuild")
        self.wait_sources(titles)
        self.choose_source("facts/atlas.md")
        first_notebook_answer = self.send("State the exact Atlas launch code and cite [1].", "GLACIER-ORBIT-417")
        repaired = self.evidence(1, 2, True)
        self.check(repaired.get("Dense coverage", "").startswith("100%"), "native dense coverage restored after explicit retry and rebuild")
        self.record(self.case_id, "Applied a missing embedding model, observed a failed source with preserved text and visibly degraded BM25 retrieval, then restored the real model through Apply, retried the source, rebuilt native dense search and observed 100% dense coverage in a new answer.")

        self.case_id = "notes_persistence"
        self.inspector("Notes")
        self.ui.click_text("New note")
        self.ui.fill("#new-note-title", "Persistent workflow note")
        note = "NOTES-INDIGO-642 survives a real native restart. " * 12
        self.ui.fill("#new-note-content", note)
        self.ui.click_text("Save Note")
        self.ui.wait(lambda: "Persistent workflow note" in self.ui.text("#inspector-panel-notes"), label="saved note")
        self.restart(name)
        self.inspector("Notes")
        self.ui.wait(lambda: "Persistent workflow note" in self.ui.text("#inspector-panel-notes"), label="note restored after restart")
        expand = self.ui.find_visible('button[aria-label="Read full note Persistent workflow note"]')
        if expand:
            self.ui.click_ref(expand)
        self.check(note.strip() in self.ui.text("#inspector-panel-notes"), "full note content survives restart")
        self.record(self.case_id, "Created a long note through the Notes form, restarted the native process, expanded the saved note and observed its full exact text.")

        self.case_id = "notebook_switch_isolation"
        self.focus_chat()
        self.ui.wait(lambda: self.last_answer() == first_notebook_answer, label="first-notebook history loaded before isolation switch")
        other = name + " isolated"
        self.create_notebook(other)
        self.sources()
        self.check(not self.source_rows(), "second notebook starts with no sources from the first")
        self.focus_chat()
        self.check(not self.last_answer(), "second notebook has no first-notebook assistant messages")
        self.inspector("Notes")
        self.check("NOTES-INDIGO-642" not in self.ui.text("#inspector-panel-notes"), "second notebook has no first-notebook notes")
        self.paste("Isolated source", "The second notebook contains only ISOLATED-VIOLET-753. Atlas facts belong to the other notebook.")
        self.wait_sources({"Isolated source"})
        self.choose_source("Isolated source")
        self.send("What exact marker is in this selected notebook source? Cite [1].", "ISOLATED-VIOLET-753")
        self.evidence(1, 0, True)
        self.remember_conversation()
        self.activate_notebook(name)
        self.wait_sources(titles)
        self.focus_chat()
        self.ui.wait(lambda: self.last_answer() == first_notebook_answer, label="exact first-notebook answer restored after switching back")
        self.check("ISOLATED-VIOLET-753" not in self.last_answer(), "restored first-notebook answer excludes the second-notebook marker")
        self.record(self.case_id, "Created a second notebook, observed separate sources/chat/Notes, ingested and queried its distinct marker through a scoped real model answer, then switched back and observed the first notebook's sources and exact prior answer.")

        self.case_id = "source_delete_restart"
        self.sources()
        self.ui.click_text("Select all")
        self.ui.click_text("Delete selected")
        self.ui.wait(lambda: not self.source_rows() and "Loaded" not in self.ui.text(), label="selected sources deleted")
        self.restart(name)
        self.sources()
        self.check(not self.source_rows(), "deleted sources stay absent after restart")
        self.send("Reply with exactly SOURCES_REMOVED.", "SOURCES_REMOVED")
        self.evidence(0, 0, False)
        self.inspector("Notes")
        self.check("Persistent workflow note" in self.ui.text("#inspector-panel-notes"), "source deletion preserves saved Notes")
        self.record(self.case_id, "Deleted all selected sources through the UI, restarted, observed no source rows and a fresh zero-context chat receipt, while the saved note remained.")

        observations = self.root / "safety-observations.json"
        observations.write_text(json.dumps({"rendered_scope_receipts": self.scope_checks, "default_surfaces": self.default_surfaces}, indent=2) + "\n")
        self.receipt["safety_observations"] = evidence(observations, self.root, "runtime_log")
        # These flags describe the recorded workflow scope only. Each is set
        # after the corresponding observed assertions above, never from defaults.
        self.receipt["source_scope_widened"] = False
        self.receipt["hidden_fallback"] = False
        self.receipt["raw_uuid_flood"] = False


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, help="new evidence directory (default: ignored .codex-run-receipts)")
    parser.add_argument("--require-baseline", action="store_true",
                        help="Exit 0 only if native startup and notebook persistence cases pass. Full release stays blocked.")
    parser.add_argument("--ollama-config", type=Path, help="Typed owned loopback Ollama runtime/model snapshot for the integrated UI workflow")
    parser.add_argument("--require-integrated", action="store_true", help="Require all twelve observed native UI cases using --ollama-config")
    parser.add_argument("--prebuilt-config", type=Path, help="Source-bound extracted AppImage build evidence. Launch AppRun in place without rebuilding.")
    args = parser.parse_args()
    if args.require_integrated and not args.ollama_config:
        parser.error("--require-integrated requires --ollama-config")
    repo = args.repo.resolve()
    run_id = str(uuid.uuid4())
    root = (args.output or repo / ".codex-run-receipts" / f"desktop-{run_id}").resolve()
    # Never reuse a profile or overwrite an earlier receipt.
    root.mkdir(parents=True, exist_ok=False)
    receipt = {"schema": LIVE_SCHEMA, "run_id": run_id, "started_at": now(),
               "status": "blocked", "runtime": "native_tauri", "live_desktop_exercised": False,
               "source_scope_widened": None, "hidden_fallback": None, "raw_uuid_flood": None,
               "isolated_data_root": str(root / "profile"), "cases": [], "blockers": []}
    process = None
    driver = None
    driver_log = None
    build_log = root / "build.log"
    workflow = None
    prebuilt_config = None
    try:
        receipt["source"] = capture_source_identity(repo)
        if args.ollama_config:
            receipt["ollama_config"] = load_ollama_config(args.ollama_config)
        prerequisites = [
            (sys.platform == "linux", "this driver currently supports Linux only"),
            (receipt["source"]["worktree_clean"], "commit the source snapshot before running live evidence"),
            (bool(os.environ.get("DISPLAY")), "no DISPLAY available (use xvfb-run on a supported Linux host)"),
            (bool(shutil.which("tauri-driver")), "tauri-driver is not installed"),
            (bool(shutil.which("WebKitWebDriver")), "WebKitWebDriver is not installed"),
            (bool(shutil.which("npm")), "npm is not installed"),
            (bool(shutil.which("cargo")), "cargo is not installed"),
        ]
        for available, blocker in prerequisites:
            if not available:
                receipt["blockers"].append(blocker)
        if receipt["blockers"]:
            raise BaselineBlocked

        build_log = root / "build.log"
        copied_binary = root / "gloss"
        if args.prebuilt_config:
            prebuilt_config = load_prebuilt_config(args.prebuilt_config, receipt["source"])
            receipt["prebuilt_config"] = prebuilt_config
            command, code = prebuilt_config["build_command"], prebuilt_config["build_exit_code"]
            shutil.copy2(prebuilt_config["build_log"], build_log)
            shutil.copy2(prebuilt_config["binary"], copied_binary)
            application = Path(prebuilt_config["application"])
            shutil.copy2(application, root / "packaged-AppRun")
        else:
            command = ["npm", "exec", "--", "tauri", "build", "--debug", "--no-bundle", "--features", "semantic-memory-turbo-quant"]
            with build_log.open("w") as stream:
                code = subprocess.run(command, cwd=repo, stdout=stream, stderr=subprocess.STDOUT, check=False, timeout=3600).returncode
            if code:
                raise RuntimeError(f"native build failed with exit {code} (see build.log)")
            metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=repo))
            built_application = Path(metadata["target_directory"]) / "debug" / "gloss"
            shutil.copy2(built_application, copied_binary)
            application = copied_binary
        receipt["build"] = {"command": command, "exit_code": code, "source": receipt["source"],
                            "binary": evidence(copied_binary, root, "executable"),
                            "log": evidence(build_log, root, "build_log")}
        if prebuilt_config:
            receipt["build"]["launcher"] = evidence(root / "packaged-AppRun", root, "executable")
            receipt["build"]["artifact_sha256"] = prebuilt_config["artifact_sha256"]
        if capture_source_identity(repo) != receipt["source"]:
            raise RuntimeError("source changed during the native build")

        env = os.environ.copy()
        for key, directory in [("XDG_DATA_HOME", "data"), ("XDG_CONFIG_HOME", "config"), ("XDG_CACHE_HOME", "cache")]:
            location = root / "profile" / directory
            location.mkdir(parents=True)
            env[key] = str(location)
        # directories::ProjectDirs uses XDG_DATA_HOME on Linux. An empty profile
        # means no user notebooks, provider credentials or queued jobs are read.
        port, native_port = free_port(), free_port()
        while native_port == port:
            native_port = free_port()
        driver_log = (root / "driver.log").open("w")
        process = subprocess.Popen(["tauri-driver", "--port", str(port), "--native-port", str(native_port)],
                                   cwd=application.parent if prebuilt_config else repo, env=env, stdout=driver_log, stderr=subprocess.STDOUT)
        driver = WebDriver(port)
        deadline = time.monotonic() + 30
        while True:
            if process.poll() is not None:
                raise RuntimeError("tauri-driver exited before accepting a session")
            try:
                driver.request("GET", "/status")
                break
            except (urllib.error.URLError, OSError):
                if time.monotonic() >= deadline:
                    raise RuntimeError("tauri-driver startup timed out")
                time.sleep(0.2)
        driver.start(application)
        driver.wait(lambda: driver.execute("return !!window.__TAURI_INTERNALS__ && document.body.innerText.includes('No notebooks yet')"))
        receipt["live_desktop_exercised"] = True
        receipt["cases"].append(driver.case("startup_idle", "Native Tauri window reached the empty notebook view using an isolated profile with no queued source jobs.", root))
        name = f"Desktop smoke {run_id[:8]}"
        driver.click('button[title="Create notebook"]')
        element = driver.element('input[aria-label="New notebook name"]')
        driver.call("POST", f"/element/{element}/value", {"text": name + "\ue007"})
        driver.wait(lambda: driver.find_visible('button[aria-current="page"]:not(:disabled)', name), label="created and activated notebook")
        driver.stop()
        driver.start(application)
        driver.wait(lambda: driver.find_visible('button[aria-current="page"]:not(:disabled)', name), label="confirmed active notebook after restart")
        row = driver.execute("return Array.from(document.querySelectorAll('button')).find(e => e.textContent === arguments[0]).parentElement", [name])
        driver.call("POST", "/actions", {"actions": [{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": [
            {"type": "pointerMove", "duration": 100, "origin": row, "x": 0, "y": 0}]}]})
        driver.click('button[title="Delete notebook"]')
        driver.click('button[aria-label="Cancel notebook deletion"]')
        driver.wait(lambda: driver.find_visible("button", name) and not driver.find_visible('button[aria-label="Cancel notebook deletion"]'), label="cancelled deletion retained notebook")
        driver.call("POST", "/actions", {"actions": [{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": [
            {"type": "pointerMove", "duration": 100, "origin": row, "x": 0, "y": 0}]}]})
        driver.click('button[title="Delete notebook"]')
        driver.click('button[aria-label^="Confirm delete notebook "]')
        driver.wait(lambda: driver.execute("return document.body.innerText.includes('No notebooks yet')"))
        driver.stop()
        driver.start(application)
        driver.wait(lambda: driver.execute("return document.body.innerText.includes('No notebooks yet')"))
        receipt["cases"].append(driver.case("notebook_crud_restart", "Created a notebook through the UI, observed it after native session restart, cancelled deletion and observed the retained notebook, then explicitly confirmed deletion and verified it after a second restart.", root))
        if args.ollama_config:
            workflow = IntegratedWorkflow(driver, application, root, receipt["ollama_config"], receipt)
            workflow.run(f"Integrated {run_id[:8]}")
        else:
            receipt["blockers"].append("Remaining import, real-provider chat, retrieval, cancellation, prompt/model, Notes and source recovery cases require live observations. Baseline is not release-grade.")
    except BaselineBlocked as error:
        if str(error):
            receipt["blockers"].append(str(error))
    except Exception as error:
        receipt["status"] = "fail"
        receipt["blockers"].append(str(error))
        details = {"case": workflow.case_id if workflow else "baseline-or-build", "error": str(error)[-2000:], "traceback": traceback.format_exc()[-3000:]}
        if driver:
            details["last_webdriver_request"] = json.dumps(driver.trace[-1])[-2000:] if driver.trace else None
            try:
                details["visible_ui_text"] = driver.text()[-8000:]
                driver.snapshot("failure", root)
                screenshot = driver.call("GET", "/screenshot")
                (root / "failure.png").write_bytes(base64.b64decode(screenshot, validate=True))
            except Exception as capture_error:
                details["capture_error"] = str(capture_error)
        if build_log.is_file():
            details["build_log_tail"] = build_log.read_text(errors="replace")[-8000:]
        if driver_log:
            driver_log.flush()
            details["driver_log_tail"] = (root / "driver.log").read_text(errors="replace")[-6000:]
        (root / "failure.json").write_text(json.dumps(details, indent=2) + "\n")
        print("NATIVE_DESKTOP_FAILURE " + json.dumps(details), flush=True)
    finally:
        if driver:
            try:
                driver.stop()
            except Exception as error:
                receipt["status"] = "fail"
                receipt["blockers"].append(f"session cleanup failed: {error}")
        if process:
            try:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)
            except (OSError, subprocess.SubprocessError) as error:
                receipt["status"] = "fail"
                receipt["blockers"].append(f"native driver cleanup failed: {error}")
        if driver_log:
            driver_log.close()
        if driver:
            (root / "webdriver-trace.json").write_text(json.dumps(driver.trace, indent=2) + "\n")
        completed = {case["id"] for case in receipt["cases"]}
        for case_id in REQUIRED_LIVE_CASES:
            if case_id not in completed:
                receipt["cases"].append({"id": case_id, "status": "blocked", "observation": "Not exercised by this driver run", "evidence": []})
        try:
            if "source" in receipt and capture_source_identity(repo) != receipt["source"]:
                receipt["status"] = "fail"
                receipt["blockers"].append("source changed during desktop observations")
            if prebuilt_config:
                load_prebuilt_config(args.prebuilt_config, receipt["source"])
        except (OSError, RuntimeError, ValueError, subprocess.SubprocessError) as error:
            receipt["status"] = "fail"
            receipt["blockers"].append(f"could not recheck source identity: {error}")
        receipt["finished_at"] = now()
        receipt["baseline_status"] = (
            "fail" if receipt["status"] == "fail" else
            "pass" if all(any(case["id"] == case_id and case["status"] == "pass"
                              for case in receipt["cases"]) for case_id in BASELINE_CASES)
            else "blocked"
        )
        receipt["integrated_status"] = "pass" if args.ollama_config and receipt["status"] != "fail" and all(
            sum(case["id"] == case_id for case in receipt["cases"]) == 1 and
            any(case["id"] == case_id and case["status"] == "pass" for case in receipt["cases"])
            for case_id in REQUIRED_LIVE_CASES
        ) else "fail" if receipt["status"] == "fail" else "blocked"
        if receipt["integrated_status"] == "pass" and not receipt["blockers"] and all(receipt.get(flag) is False for flag in ("source_scope_widened", "hidden_fallback", "raw_uuid_flood")):
            receipt["status"] = "pass"
        path = root / "LIVE_DESKTOP_SMOKE_RECEIPT.json"
        path.write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"status": receipt["status"], "baseline_status": receipt["baseline_status"], "integrated_status": receipt["integrated_status"],
                          "receipt": str(path), "blockers": receipt["blockers"]}, indent=2))
    return result_exit_code(receipt, args.require_baseline, args.require_integrated)


if __name__ == "__main__":
    raise SystemExit(main())
