#!/usr/bin/env python3
"""Real Linux Tauri/WebDriver baseline in disposable data directories.

Builds and drives the native app. By default it exits 2 while full acceptance
cases remain unobserved. --require-baseline enforces only its two real UI cases
for CI without changing the incomplete release receipt. No IPC is mocked.
"""
from __future__ import annotations

import argparse
import base64
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request
import uuid

SCRIPT_DIR = str(Path(__file__).resolve().parent)
if SCRIPT_DIR not in sys.path:
    sys.path.insert(0, SCRIPT_DIR)
from gloss_desktop_smoke_harness import LIVE_SCHEMA, REQUIRED_LIVE_CASES, file_sha256
from source_snapshot import capture_source_identity

BASELINE_CASES = ("startup_idle", "notebook_crud_restart")


class BaselineBlocked(Exception):
    """A required capability is absent before native execution."""


def result_exit_code(receipt: dict, require_baseline: bool) -> int:
    """Do not convert a generic partial/blocked receipt into CI success."""
    if receipt.get("status") == "fail":
        return 1
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
        self.endpoint = f"http://127.0.0.1:{port}"
        self.session: str | None = None
        self.trace: list[dict] = []

    def request(self, method: str, path: str, payload: dict | None = None):
        request = urllib.request.Request(
            self.endpoint + path,
            data=json.dumps(payload).encode() if payload is not None else None,
            headers={"Content-Type": "application/json"},
            method=method,
        )
        try:
            with urllib.request.urlopen(request, timeout=30) as response:
                result = json.load(response)
        except urllib.error.HTTPError as error:
            raise RuntimeError(f"WebDriver {method} {path}: {error.read().decode(errors='replace')}") from error
        value = result.get("value")
        self.trace.append({"at": now(), "method": method, "path": path, "request": payload,
                           "response": "PNG captured" if path.endswith("/screenshot") else value})
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

    def wait(self, condition, timeout: int = 30):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            value = condition()
            if value:
                return value
            time.sleep(0.2)
        raise RuntimeError("native UI condition timed out")

    def element(self, selector: str) -> str:
        value = self.call("POST", "/element", {"using": "css selector", "value": selector})
        return value["element-6066-11e4-a52e-4f735466cecf"]

    def click(self, selector: str):
        self.call("POST", f"/element/{self.element(selector)}/click", {})

    def stop(self):
        if self.session:
            try:
                self.request("DELETE", f"/session/{self.session}")
            finally:
                self.session = None

    def case(self, case_id: str, observation: str, root: Path) -> dict:
        screenshot = root / f"{case_id}.png"
        screenshot.write_bytes(base64.b64decode(self.call("GET", "/screenshot"), validate=True))
        log = root / f"{case_id}.json"
        log.write_text(json.dumps(self.trace, indent=2) + "\n")
        return {"id": case_id, "status": "pass", "observation": observation,
                "evidence": [evidence(log, root, "runtime_log"), evidence(screenshot, root, "screenshot")]}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path, help="new evidence directory (default: ignored .codex-run-receipts)")
    parser.add_argument("--require-baseline", action="store_true",
                        help="Exit 0 only if native startup and notebook persistence cases pass. Full release stays blocked.")
    args = parser.parse_args()
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
    try:
        receipt["source"] = capture_source_identity(repo)
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

        command = ["npm", "exec", "--", "tauri", "build", "--debug", "--no-bundle", "--features", "semantic-memory-turbo-quant"]
        build_log = root / "build.log"
        with build_log.open("w") as stream:
            code = subprocess.run(command, cwd=repo, stdout=stream, stderr=subprocess.STDOUT, check=False, timeout=3600).returncode
        if code:
            raise RuntimeError(f"native build failed with exit {code} (see build.log)")
        metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--no-deps", "--format-version", "1"], cwd=repo))
        application = Path(metadata["target_directory"]) / "debug" / "gloss"
        copied_binary = root / "gloss"
        shutil.copy2(application, copied_binary)
        receipt["build"] = {"command": command, "exit_code": code, "source": receipt["source"],
                            "binary": evidence(copied_binary, root, "executable"),
                            "log": evidence(build_log, root, "build_log")}
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
                                   cwd=repo, env=env, stdout=driver_log, stderr=subprocess.STDOUT)
        driver = WebDriver(port)
        deadline = time.monotonic() + 30
        while True:
            if process.poll() is not None:
                raise RuntimeError("tauri-driver exited before accepting a session")
            try:
                driver.request("GET", "/status")
                break
            except urllib.error.URLError:
                if time.monotonic() >= deadline:
                    raise RuntimeError("tauri-driver startup timed out")
                time.sleep(0.2)
        driver.start(copied_binary)
        driver.wait(lambda: driver.execute("return !!window.__TAURI_INTERNALS__ && document.body.innerText.includes('No notebooks yet')"))
        receipt["live_desktop_exercised"] = True
        receipt["cases"].append(driver.case("startup_idle", "Native Tauri window reached the empty notebook view using an isolated profile with no queued source jobs.", root))
        name = f"Desktop smoke {run_id[:8]}"
        driver.click('button[title="Create notebook"]')
        element = driver.element('input[aria-label="New notebook name"]')
        driver.call("POST", f"/element/{element}/value", {"text": name + "\ue007"})
        driver.wait(lambda: driver.execute("return Array.from(document.querySelectorAll('span')).some(e => e.textContent === arguments[0])", [name]))
        driver.stop()
        driver.start(copied_binary)
        driver.wait(lambda: driver.execute("return Array.from(document.querySelectorAll('span')).some(e => e.textContent === arguments[0])", [name]))
        row = driver.execute("return Array.from(document.querySelectorAll('span')).find(e => e.textContent === arguments[0]).parentElement", [name])
        driver.call("POST", "/actions", {"actions": [{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": [
            {"type": "pointerMove", "duration": 100, "origin": row, "x": 0, "y": 0}]}]})
        driver.click('button[title="Delete notebook"]')
        driver.click('button[aria-label="Cancel notebook deletion"]')
        driver.wait(lambda: driver.execute("return Array.from(document.querySelectorAll('span')).some(e => e.textContent === arguments[0]) && !document.querySelector('button[aria-label=\"Cancel notebook deletion\"]')", [name]))
        driver.call("POST", "/actions", {"actions": [{"type": "pointer", "id": "mouse", "parameters": {"pointerType": "mouse"}, "actions": [
            {"type": "pointerMove", "duration": 100, "origin": row, "x": 0, "y": 0}]}]})
        driver.click('button[title="Delete notebook"]')
        driver.click('button[aria-label^="Confirm delete notebook "]')
        driver.wait(lambda: driver.execute("return document.body.innerText.includes('No notebooks yet')"))
        driver.stop()
        driver.start(copied_binary)
        driver.wait(lambda: driver.execute("return document.body.innerText.includes('No notebooks yet')"))
        receipt["cases"].append(driver.case("notebook_crud_restart", "Created a notebook through the UI, observed it after native session restart, cancelled deletion and observed the retained notebook, then explicitly confirmed deletion and verified it after a second restart.", root))
        receipt["blockers"].append("Remaining import, real-provider chat, retrieval, cancellation, prompt/model, Notes and source recovery cases require live observations. Baseline is not release-grade.")
    except BaselineBlocked:
        pass
    except (OSError, RuntimeError, KeyError, ValueError, subprocess.SubprocessError) as error:
        receipt["status"] = "fail"
        receipt["blockers"].append(str(error))
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
                receipt["cases"].append({"id": case_id, "status": "blocked", "observation": "Not exercised by this baseline driver", "evidence": []})
        try:
            if "source" in receipt and capture_source_identity(repo) != receipt["source"]:
                receipt["status"] = "fail"
                receipt["blockers"].append("source changed during desktop observations")
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
        path = root / "LIVE_DESKTOP_SMOKE_RECEIPT.json"
        path.write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"status": receipt["status"], "baseline_status": receipt["baseline_status"],
                          "receipt": str(path), "blockers": receipt["blockers"]}, indent=2))
    return result_exit_code(receipt, args.require_baseline)


if __name__ == "__main__":
    raise SystemExit(main())
