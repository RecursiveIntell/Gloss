#!/usr/bin/env python3
"""Build and validate Gloss Linux installer artifacts for the current run."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import shutil
import os
import signal
import subprocess
import sys
import tarfile
import tempfile
from datetime import datetime, timezone
from pathlib import Path


def _run(repo: Path, command: list[str]) -> dict:
    completed = subprocess.run(
        command,
        cwd=repo,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    return {
        "command": " ".join(command),
        "exit_code": completed.returncode,
        "stdout_tail": completed.stdout[-6000:],
        "stderr_tail": completed.stderr[-6000:],
    }


def _run_env(repo: Path, command: list[str], env: dict[str, str]) -> dict:
    completed = subprocess.run(
        command,
        cwd=repo,
        env={**os.environ, **env},
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=False,
    )
    return {
        "command": " ".join(command),
        "exit_code": completed.returncode,
        "stdout_tail": completed.stdout[-6000:],
        "stderr_tail": completed.stderr[-6000:],
    }


def _launch_for_window(repo: Path, command: list[str], env: dict[str, str], timeout_seconds: int) -> dict:
    proc = subprocess.Popen(
        command,
        cwd=repo,
        env={**os.environ, **env},
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        start_new_session=True,
    )
    timed_out = False
    try:
        stdout, stderr = proc.communicate(timeout=timeout_seconds)
    except subprocess.TimeoutExpired:
        timed_out = True
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except ProcessLookupError:
            pass
        try:
            stdout, stderr = proc.communicate(timeout=4)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            stdout, stderr = proc.communicate(timeout=4)
    return {
        "command": " ".join(command),
        "exit_code": proc.returncode,
        "timed_out": timed_out,
        "stdout_tail": (stdout or "")[-6000:],
        "stderr_tail": (stderr or "")[-6000:],
    }


def _read_json(path: Path) -> dict:
    return json.loads(path.read_text(encoding="utf-8"))


def _current_run(repo: Path) -> str | None:
    text = (repo / "docs/codex-runs/CURRENT_RUN.md").read_text(encoding="utf-8", errors="replace")
    match = re.search(r"Current run:\s*`?([^`\n]+)`?", text)
    return match.group(1).strip() if match else None


def _sha256(path: Path) -> str:
    hasher = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            hasher.update(chunk)
    return hasher.hexdigest()


def _newest(paths: list[Path]) -> Path | None:
    existing = [path for path in paths if path.exists()]
    return max(existing, key=lambda path: path.stat().st_mtime) if existing else None


def _tool(name: str) -> dict:
    path = shutil.which(name)
    return {"name": name, "path": path, "available": path is not None}


def _safe_tar_extract(archive: Path, destination: Path) -> list[str]:
    with tarfile.open(archive, "r:*") as tar:
        names = tar.getnames()
        for name in names:
            pure = Path(name)
            if pure.is_absolute() or ".." in pure.parts:
                raise ValueError(f"unsafe tar member: {name}")
        tar.extractall(destination)
        return names


def _read_first(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""


def _validate_desktop_file(text: str) -> list[str]:
    failures: list[str] = []
    required = {
        "Type=Application",
        "Name=Gloss",
        "Exec=gloss",
        "Icon=gloss",
        "Terminal=false",
    }
    for marker in required:
        if marker not in text:
            failures.append(f"desktop file missing {marker}")
    return failures


def _validate_extracted_payload(root: Path, prefix: str) -> tuple[list[str], list[str]]:
    failures: list[str] = []
    files = sorted(str(path.relative_to(root)) for path in root.rglob("*") if path.is_file())
    prefix = prefix.strip("/")
    base = f"{prefix}/" if prefix else ""
    expected_binary = f"{base}usr/bin/gloss"
    expected_desktop = f"{base}usr/share/applications/Gloss.desktop"
    if expected_binary not in files:
        failures.append(f"payload missing {expected_binary}")
    if expected_desktop not in files:
        failures.append(f"payload missing {expected_desktop}")
    icon_files = [file for file in files if file.endswith("/apps/gloss.png")]
    if len(icon_files) < 3:
        failures.append("payload missing expected hicolor app icons")
    desktop_text = _read_first(root / expected_desktop)
    failures.extend(_validate_desktop_file(desktop_text))
    return failures, files


def _validate_rpm(repo: Path, artifact: Path) -> dict:
    result = {
        "target": "rpm",
        "artifact": str(artifact.relative_to(repo)),
        "artifact_sha256": _sha256(artifact),
        "artifact_size_bytes": artifact.stat().st_size,
        "status": "pass",
        "failures": [],
        "commands": [],
        "payload_files": [],
    }
    for command in [
        ["rpm", "-qip", str(artifact)],
        ["rpm", "-K", str(artifact)],
        ["rpm", "-qlp", str(artifact)],
        ["rpm", "-qpR", str(artifact)],
    ]:
        command_result = _run(repo, command)
        result["commands"].append(command_result)
        if command_result["exit_code"] != 0:
            result["failures"].append(f"{command_result['command']} exited {command_result['exit_code']}")
    with tempfile.TemporaryDirectory(prefix="gloss-rpm-smoke-") as tmp:
        tmp_path = Path(tmp)
        extract = _run(
            repo,
            [
                "bash",
                "-lc",
                f"rpm2cpio {str(artifact)!r} | (cd {str(tmp_path)!r} && cpio -idmu >/dev/null)",
            ],
        )
        result["commands"].append(extract)
        if extract["exit_code"] != 0:
            result["failures"].append("rpm payload extraction failed")
        else:
            failures, files = _validate_extracted_payload(tmp_path, "")
            result["failures"].extend(failures)
            result["payload_files"] = files
    if result["failures"]:
        result["status"] = "fail"
    return result


def _validate_deb(repo: Path, artifact: Path) -> dict:
    result = {
        "target": "deb",
        "artifact": str(artifact.relative_to(repo)),
        "artifact_sha256": _sha256(artifact),
        "artifact_size_bytes": artifact.stat().st_size,
        "status": "pass",
        "failures": [],
        "commands": [],
        "control": {},
        "payload_files": [],
        "validation_mode": "dpkg-deb" if shutil.which("dpkg-deb") else "ar_tar_fallback",
    }
    with tempfile.TemporaryDirectory(prefix="gloss-deb-smoke-") as tmp:
        tmp_path = Path(tmp)
        if shutil.which("dpkg-deb"):
            for command in [
                ["dpkg-deb", "--field", str(artifact)],
                ["dpkg-deb", "--contents", str(artifact)],
            ]:
                command_result = _run(repo, command)
                result["commands"].append(command_result)
                if command_result["exit_code"] != 0:
                    result["failures"].append(
                        f"{command_result['command']} exited {command_result['exit_code']}"
                    )
            extract = _run(repo, ["dpkg-deb", "-x", str(artifact), str(tmp_path / "data")])
            result["commands"].append(extract)
        else:
            extract = subprocess.run(
                ["ar", "x", str(artifact)],
                cwd=tmp_path,
                text=True,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                check=False,
            )
            result["commands"].append(
                {
                    "command": f"ar x {artifact}",
                    "exit_code": extract.returncode,
                    "stdout_tail": extract.stdout[-6000:],
                    "stderr_tail": extract.stderr[-6000:],
                }
            )
            if extract.returncode == 0:
                try:
                    control_dir = tmp_path / "control"
                    data_dir = tmp_path / "data"
                    control_dir.mkdir()
                    data_dir.mkdir()
                    _safe_tar_extract(tmp_path / "control.tar.gz", control_dir)
                    _safe_tar_extract(tmp_path / "data.tar.gz", data_dir)
                except Exception as exc:  # noqa: BLE001 - receipt should include exact extraction failure.
                    result["failures"].append(f"deb ar/tar extraction failed: {exc}")
            else:
                result["failures"].append("deb ar extraction failed")
        control_text = _read_first(tmp_path / "control/control")
        for line in control_text.splitlines():
            if ":" in line:
                key, value = line.split(":", 1)
                result["control"][key.strip()] = value.strip()
        if result["control"].get("Package") != "gloss":
            result["failures"].append("deb control Package is not gloss")
        if result["control"].get("Architecture") not in {"amd64", "x86_64"}:
            result["failures"].append("deb control Architecture is not amd64/x86_64")
        data_root = tmp_path / "data"
        if data_root.exists():
            failures, files = _validate_extracted_payload(data_root, "")
            result["failures"].extend(failures)
            result["payload_files"] = files
        else:
            result["failures"].append("deb payload was not extracted")
    if result["failures"]:
        result["status"] = "fail"
    return result


def _launch_extracted_package(repo: Path, rpm_artifact: Path | None) -> dict:
    result = {
        "schema": "GlossInstalledPackageLaunchSmokeV1",
        "status": "blocked",
        "source_target": "rpm",
        "source_artifact": str(rpm_artifact.relative_to(repo)) if rpm_artifact else None,
        "isolated_home": True,
        "private_dbus_session": shutil.which("dbus-run-session") is not None,
        "display_available": bool(os.environ.get("DISPLAY") or os.environ.get("WAYLAND_DISPLAY")),
        "launch_timeout_seconds": 8,
        "exit_code": None,
        "stayed_alive_until_timeout": False,
        "created_app_databases": False,
        "stdout_tail": "",
        "stderr_tail": "",
        "failures": [],
    }
    required_tools = ["rpm2cpio", "cpio"]
    missing = [tool for tool in required_tools if shutil.which(tool) is None]
    if shutil.which("dbus-run-session") is None:
        missing.append("dbus-run-session")
    if missing:
        result["failures"].append(f"missing launch tooling: {missing}")
        return result
    if rpm_artifact is None:
        result["failures"].append("missing rpm artifact for installed launch extraction")
        return result
    if not result["display_available"]:
        result["failures"].append("no DISPLAY or WAYLAND_DISPLAY available for installed launch")
        return result

    with tempfile.TemporaryDirectory(prefix="gloss-installed-launch-") as tmp:
        tmp_path = Path(tmp)
        extract = _run(
            repo,
            [
                "bash",
                "-lc",
                f"rpm2cpio {str(rpm_artifact)!r} | (cd {str(tmp_path)!r} && cpio -idmu >/dev/null)",
            ],
        )
        if extract["exit_code"] != 0:
            result["failures"].append("rpm extraction for installed launch failed")
            result["extract_command"] = extract
            return result
        for name in ["home", "config", "cache", "data", "runtime"]:
            (tmp_path / name).mkdir(parents=True, exist_ok=True)
        (tmp_path / "runtime").chmod(0o700)
        binary = tmp_path / "usr/bin/gloss"
        if not binary.exists():
            result["failures"].append("extracted installed launch binary missing")
            return result
        env = {
            "HOME": str(tmp_path / "home"),
            "XDG_CONFIG_HOME": str(tmp_path / "config"),
            "XDG_CACHE_HOME": str(tmp_path / "cache"),
            "XDG_DATA_HOME": str(tmp_path / "data"),
            "XDG_RUNTIME_DIR": str(tmp_path / "runtime"),
            "WEBKIT_DISABLE_DMABUF_RENDERER": "1",
        }
        command = [
            "dbus-run-session",
            "--",
            str(binary),
        ]
        launch = _launch_for_window(repo, command, env, int(result["launch_timeout_seconds"]))
        result["exit_code"] = launch["exit_code"]
        result["stdout_tail"] = launch["stdout_tail"]
        result["stderr_tail"] = launch["stderr_tail"]
        result["stayed_alive_until_timeout"] = launch["timed_out"] is True
        data_dir = tmp_path / "data/gloss"
        result["created_app_databases"] = (data_dir / "gloss.db").exists() and (data_dir / "queue.db").exists()
        lowered = f"{launch['stdout_tail']}\n{launch['stderr_tail']}".lower()
        fatal_markers = ["thread 'main' panicked", "segmentation fault", "traceback", "webkit process crashed"]
        for marker in fatal_markers:
            if marker in lowered:
                result["failures"].append(f"fatal launch marker present: {marker}")
        if not result["stayed_alive_until_timeout"]:
            result["failures"].append(f"installed launch exited before timeout with code {launch['exit_code']}")
        if not result["created_app_databases"]:
            result["failures"].append("installed launch did not create isolated app databases")
    result["status"] = "pass" if not result["failures"] else "fail"
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--receipt")
    args = parser.parse_args()

    repo = Path(args.repo).resolve()
    run_id = _current_run(repo)
    if not run_id:
        print(json.dumps({"ok": False, "failures": ["missing current run id"]}, indent=2))
        return 1
    receipt_path = (
        Path(args.receipt).resolve()
        if args.receipt
        else repo / "docs" / "codex-runs" / run_id / "INSTALLER_SMOKE_RECEIPT.json"
    )
    tauri_conf = _read_json(repo / "src-tauri/tauri.conf.json")
    targets = tauri_conf.get("bundle", {}).get("targets", [])
    if isinstance(targets, str):
        targets = [targets]
    build_command = _run(repo, ["npm", "run", "tauri:build:release"]) if args.build else None

    tools = {name: _tool(name) for name in ["rpm", "rpm2cpio", "cpio", "dpkg-deb", "ar", "tar", "appimagetool", "linuxdeploy"]}
    failures: list[str] = []
    if build_command and build_command["exit_code"] != 0:
        failures.append("tauri release bundle build failed")

    target_results: list[dict] = []
    rpm_artifact: Path | None = None
    if "rpm" in targets:
        missing_tools = [name for name in ["rpm", "rpm2cpio", "cpio"] if not tools[name]["available"]]
        artifact = _newest(list((repo / "target/release/bundle/rpm").glob("*.rpm")))
        rpm_artifact = artifact
        if missing_tools:
            target_results.append({"target": "rpm", "status": "blocked", "failures": [f"missing tools: {missing_tools}"]})
            failures.append("rpm smoke missing required tooling")
        elif artifact is None:
            target_results.append({"target": "rpm", "status": "blocked", "failures": ["missing rpm artifact"]})
            failures.append("rpm artifact missing")
        else:
            target_results.append(_validate_rpm(repo, artifact))
    if "deb" in targets:
        missing_tools = [name for name in ["ar", "tar"] if not tools[name]["available"]]
        artifact = _newest(list((repo / "target/release/bundle/deb").glob("*.deb")))
        if missing_tools and not tools["dpkg-deb"]["available"]:
            target_results.append({"target": "deb", "status": "blocked", "failures": [f"missing tools: {missing_tools}"]})
            failures.append("deb smoke missing required tooling")
        elif artifact is None:
            target_results.append({"target": "deb", "status": "blocked", "failures": ["missing deb artifact"]})
            failures.append("deb artifact missing")
        else:
            target_results.append(_validate_deb(repo, artifact))

    for target_result in target_results:
        if target_result.get("status") != "pass":
            failures.extend(f"{target_result.get('target')} smoke: {failure}" for failure in target_result.get("failures", []))
    launch_result = _launch_extracted_package(repo, rpm_artifact)
    if launch_result.get("status") != "pass":
        failures.extend(f"installed launch smoke: {failure}" for failure in launch_result.get("failures", []))

    unsupported_targets = []
    if "appimage" not in {str(target).lower() for target in targets}:
        unsupported_targets.append(
            {
                "target": "appimage",
                "status": "not_configured",
                "reason": "Tauri bundle target is not configured and appimagetool/linuxdeploy are not both available",
                "appimagetool_available": tools["appimagetool"]["available"],
                "linuxdeploy_available": tools["linuxdeploy"]["available"],
            }
        )

    payload = {
        "schema": "GlossInstallerSmokeReceiptV1",
        "run_id": run_id,
        "recorded_utc": datetime.now(timezone.utc).isoformat(),
        "status": "fail" if failures else "pass",
        "configured_targets": targets,
        "package_smoke_passed": not failures,
        "installed_launch_exercised": launch_result.get("status") == "pass",
        "release_grade": False,
        "release_blocker": True,
        "release_decision": "configured_linux_package_and_installed_launch_smoke_passed_workflow_missing"
        if not failures
        else "configured_linux_package_smoke_failed",
        "build_command": build_command,
        "tools": tools,
        "target_results": target_results,
        "installed_launch_result": launch_result,
        "unsupported_targets": unsupported_targets,
        "failures": failures,
        "remaining_blockers": [
            "Installed package GUI workflow smoke beyond launch is not exercised.",
            "AppImage is not configured because appimagetool/linuxdeploy are missing.",
            "Live desktop GUI workflow smoke remains separate from installer payload smoke.",
        ],
    }
    receipt_path.parent.mkdir(parents=True, exist_ok=True)
    receipt_path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"ok": not failures, "receipt": str(receipt_path), "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
