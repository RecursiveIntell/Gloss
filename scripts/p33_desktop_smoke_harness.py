#!/usr/bin/env python3
from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import platform
import shutil
import subprocess
import time
from pathlib import Path

RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"


def utc_now() -> str:
    return dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def run(cmd: list[str], repo: Path) -> dict[str, object]:
    started = time.monotonic()
    proc = subprocess.run(cmd, cwd=repo, text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT)
    return {
        "cmd": cmd,
        "returncode": proc.returncode,
        "duration_ms": int((time.monotonic() - started) * 1000),
        "output_tail": proc.stdout[-4000:],
    }


def main() -> int:
    parser = argparse.ArgumentParser(description="Write P33 desktop smoke receipt.")
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

    commands: list[dict[str, object]] = []
    if not args.skip_build:
        commands.append(run(["npm", "run", "build"], repo))

    tools = {
        "tauri_driver": shutil.which("tauri-driver"),
        "webkit_webdriver": shutil.which("WebKitWebDriver"),
        "edge_driver": shutil.which("msedgedriver"),
        "xvfb_run": shutil.which("xvfb-run"),
    }
    blockers = []
    if not tools["tauri_driver"]:
        blockers.append("tauri-driver is not installed")
    if not tools["webkit_webdriver"]:
        blockers.append("WebKitWebDriver is not installed")

    receipt = {
        "schema": "GlossP33DesktopSmokeReceiptV1",
        "run_id": RUN_ID,
        "recorded_time": utc_now(),
        "completed": False,
        "blocked": bool(blockers),
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
    receipt_path.write_text(json.dumps(receipt, indent=2), encoding="utf-8")
    print(json.dumps(receipt, indent=2))
    return 0 if receipt["completed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
