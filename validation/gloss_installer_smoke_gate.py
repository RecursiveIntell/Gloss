#!/usr/bin/env python3
"""Build and replay Linux AppImage payload and native UI from a clean boundary.

With --require-integrated, the hosted real-model canary drives the extracted
AppRun through all current native acceptance cases. This does not publish a
release or certify signing or other distributions. Missing capabilities block.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import json
import os
from pathlib import Path
import platform
import shlex
import shutil
import signal
import subprocess
import sys
import tempfile
import time
import uuid

SCRIPTS = Path(__file__).resolve().parents[1] / "scripts"
if str(SCRIPTS) not in sys.path:
    sys.path.insert(0, str(SCRIPTS))
from source_snapshot import capture_source_identity
from gloss_desktop_smoke_harness import file_sha256
from live_desktop_smoke import load_prebuilt_config, result_exit_code
from live_ollama_canary import (CANARY_SCHEMA, desktop_configuration,
                                validate_desktop_receipt, validate_models)


class PackageBlocked(Exception):
    """A required capability or source boundary is unavailable."""


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


def evidence(path: Path, root: Path) -> dict:
    return {"path": str(path.relative_to(root)), "sha256": file_sha256(path)}


def run_command(command: list[str], cwd: Path, log: Path, timeout: int,
                cooperative_cleanup: bool = False) -> dict:
    started = now()
    timed_out = False
    with log.open("w") as output:
        process = subprocess.Popen(command, cwd=cwd, stdout=output,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            timed_out = True
            try:
                if cooperative_cleanup:
                    # The direct canary leader must unwind its separate service
                    # and active-command sessions before extraction is removed.
                    process.terminate()
                else:
                    os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            try:
                # Canary command + service cleanup each permit 5s TERM + 5s
                # KILL/join. Allow both plus bounded receipt finalization.
                process.wait(timeout=30 if cooperative_cleanup else 10)
            except subprocess.TimeoutExpired:
                pass
            # The leader may exit on TERM while an owned descendant ignores
            # it. Always close the group, then join the leader before return.
            try:
                os.killpg(process.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            process.wait(timeout=10)
            code = None
            output.write(f"\nCommand exceeded {timeout} seconds\n")
    if code != 0:
        # Hosted artifact downloads can be unavailable; retain a bounded useful
        # failure tail in the job log while keeping the complete log on disk.
        with log.open("rb") as stream:
            stream.seek(0, os.SEEK_END)
            stream.seek(max(0, stream.tell() - 32 * 1024))
            tail = stream.read().decode("utf-8", errors="replace")
        print(f"Failed command log tail ({log.name}, last 32 KiB):\n{tail}", flush=True)
    return {"command": command, "cwd": str(cwd), "started_at": started,
            "finished_at": now(), "exit_code": code, "timed_out": timed_out,
            "status": "pass" if code == 0 else "fail"}


def require_elf(path: Path) -> None:
    if not path.is_file() or path.is_symlink():
        raise ValueError(f"Expected a regular ELF file: {path}")
    with path.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise ValueError(f"Not an ELF executable: {path}")
    if not os.access(path, os.X_OK):
        raise ValueError(f"Executable permission missing: {path}")


def select_fresh_artifact(bundle: Path, started_ns: int) -> Path:
    artifacts = [path for path in bundle.glob("*.AppImage")
                 if path.is_file() and not path.is_symlink()
                 and path.stat().st_mtime_ns >= started_ns]
    if len(artifacts) != 1:
        raise ValueError(f"Expected exactly one newly built AppImage, found {len(artifacts)}")
    require_elf(artifacts[0])
    return artifacts[0]


def validate_payload(root: Path, product: str) -> dict:
    if not root.is_dir():
        raise ValueError("AppImage extraction did not produce squashfs-root")
    for path in root.rglob("*"):
        if path.is_symlink() and not path.resolve().is_relative_to(root.resolve()):
            raise ValueError(f"AppImage symlink escapes payload: {path.relative_to(root)}")
    application = root / "AppRun"
    if not application.is_file() or not os.access(application, os.X_OK):
        raise ValueError("AppImage is missing an executable AppRun")
    binary = root / "usr/bin/gloss"
    require_elf(binary)
    desktops = list(root.glob("*.desktop"))
    if len(desktops) != 1:
        raise ValueError("Expected one root desktop entry in AppImage")
    fields = dict(line.split("=", 1) for line in desktops[0].read_text().splitlines()
                  if "=" in line and not line.lstrip().startswith("#"))
    if fields.get("Type") != "Application" or fields.get("Name") != product:
        raise ValueError("AppImage desktop entry does not identify Gloss")
    if not fields.get("Exec") or Path(shlex.split(fields["Exec"])[0]).name != "gloss":
        raise ValueError("AppImage desktop entry does not launch gloss")
    icon = fields.get("Icon", "")
    if not icon or "/" in icon or not any(path.is_file() for suffix in ("png", "svg", "xpm")
                                            for path in root.rglob(f"{icon}.{suffix}")):
        raise ValueError("AppImage desktop icon is missing")
    return {"application": str(application.absolute()), "application_sha256": file_sha256(application),
            "binary": str(binary.resolve()), "binary_sha256": file_sha256(binary),
            "desktop_entry": str(desktops[0].relative_to(root)),
            "payload_files": sorted(str(path.relative_to(root)) for path in root.rglob("*")
                                    if path.is_file())}


def require_packaged_baseline(receipt: dict, source: dict, artifact_sha256: str) -> None:
    if result_exit_code(receipt, require_baseline=True) != 0:
        raise ValueError("Packaged native startup and notebook restart baseline did not pass")
    if receipt.get("source") != source:
        raise ValueError("Packaged desktop receipt belongs to another source snapshot")
    if receipt.get("prebuilt_config", {}).get("artifact_sha256") != artifact_sha256:
        raise ValueError("Packaged desktop receipt does not bind this AppImage")


def require_packaged_workflow(output: Path, source: dict, prebuilt: dict) -> Path:
    """Bind the existing canary and native acceptance owners to this package."""
    hosted = json.loads((output / "receipt.json").read_text())
    if (not isinstance(hosted, dict)
            or hosted.get("schema") != CANARY_SCHEMA or hosted.get("status") != "pass"
            or hosted.get("source") != source or hosted.get("source_after") != source
            or hosted.get("prebuilt_config") != prebuilt
            or hosted.get("desktop_requested") is not True
            or hosted.get("desktop_status") != "pass"
            or hosted.get("live_service_exercised") is not True):
        raise ValueError("Packaged real-model canary is incomplete or belongs to another source/build")
    models = hosted.get("models")
    if not isinstance(models, dict) or not all(isinstance(model, dict) for model in models.values()):
        raise ValueError("Packaged canary model identities are missing")
    configuration = desktop_configuration(validate_models({"models": list(models.values())}))
    if json.loads((output / "desktop-ollama-config.json").read_text()) != configuration:
        raise ValueError("Packaged desktop configuration differs from the owned pinned models")
    desktop = output / "desktop" / "LIVE_DESKTOP_SMOKE_RECEIPT.json"
    child = json.loads(desktop.read_text())
    if not isinstance(child, dict):
        raise ValueError("Packaged desktop receipt is not an object")
    validate_desktop_receipt(child, source, configuration, prebuilt)
    descriptor = {**evidence(desktop, output), "integrated_status": child["integrated_status"],
                  "full_acceptance_status": child["status"]}
    if hosted.get("desktop_receipt") != descriptor:
        raise ValueError("Packaged canary does not bind the current native child receipt")
    for field, name in (("binary", "gloss"), ("launcher", "packaged-AppRun"), ("log", "build.log")):
        observed = child["build"][field]
        if observed.get("path") != name or observed.get("sha256") != file_sha256(desktop.parent / name):
            raise ValueError(f"Packaged desktop {field} evidence changed or selected another file")
    return desktop


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path.cwd())
    parser.add_argument("--build", action="store_true", help="Required: rebuild from this clean source")
    parser.add_argument("--require-integrated", action="store_true",
                        help="Require all native acceptance cases on this AppImage using owned real models on a GitHub-hosted runner.")
    parser.add_argument("--receipt", type=Path, help="New output path; historical run receipts are never used")
    args = parser.parse_args()
    repo = args.repo.resolve()
    run_id = str(uuid.uuid4())
    receipt_path = (args.receipt or repo / ".codex-run-receipts" / f"installer-{run_id}" / "receipt.json").resolve()
    if receipt_path.exists():
        print("Refusing to overwrite an existing package receipt", file=sys.stderr)
        return 2
    if receipt_path.is_relative_to(repo / "docs"):
        print("Package receipts must not overwrite historical documentation", file=sys.stderr)
        return 2
    if receipt_path.is_relative_to(repo) and subprocess.run(
        ["git", "check-ignore", "--quiet", "--", str(receipt_path)], cwd=repo, check=False,
    ).returncode != 0:
        print("Use an ignored evidence directory or an output outside the repository", file=sys.stderr)
        return 2
    output = receipt_path.parent
    output.mkdir(parents=True, exist_ok=True)
    logs = output / f"evidence-{run_id}"
    logs.mkdir()
    receipt = {"schema": "GlossInstallerSmokeReceiptV2", "run_id": run_id,
               "scope": ("linux_appimage_payload_and_integrated_native_workflow" if args.require_integrated
                         else "linux_appimage_payload_and_native_baseline"), "started_at": now(),
               "status": "blocked", "package_smoke_passed": False,
               "integrated_requested": args.require_integrated,
               "integrated_status": "pending" if args.require_integrated else "not_requested",
               "installed_launch_exercised": False, "release_grade": False,
               "release_blocker": True, "commands": [], "failures": [],
               "platform": {"system": platform.system(), "machine": platform.machine(),
                            "kernel": platform.release()},
               "unsupported_profiles": ["rpm", "deb", "linux-arm64", "macos", "windows"],
               "remaining_blockers": ["Full packaged provider/import/recovery workflow is not exercised by the baseline.",
                                      "Signing and non-Linux/non-Ubuntu distribution compatibility are not certified."]}
    try:
        if sys.platform == "linux":
            receipt["platform"]["os_release"] = platform.freedesktop_os_release()
        source = capture_source_identity(repo)
        receipt["source_before"] = source
        if not source["worktree_clean"]:
            raise PackageBlocked("Package evidence requires a clean committed source snapshot")
        if not args.build:
            raise PackageBlocked("Pass --build; an existing artifact alone has no current build proof")
        if sys.platform != "linux" or platform.machine() not in ("x86_64", "AMD64"):
            raise PackageBlocked("This gate currently supports Linux x86_64 AppImage only")
        config = json.loads((repo / "src-tauri/tauri.conf.json").read_text())
        targets = config.get("bundle", {}).get("targets", [])
        targets = [targets] if isinstance(targets, str) else targets
        receipt["configured_targets"] = targets
        if [str(target).lower() for target in targets] != ["appimage"]:
            raise PackageBlocked("This gate requires the AppImage-only configured release profile")
        required_tools = ("npm", "cargo", "mksquashfs", "tauri-driver", "WebKitWebDriver", "dbus-run-session")
        if args.require_integrated:
            required_tools += ("xdotool",)
        missing = [tool for tool in required_tools
                   if shutil.which(tool) is None]
        if missing or not os.environ.get("DISPLAY"):
            raise PackageBlocked(f"Missing package replay capabilities: tools={missing}, DISPLAY={bool(os.environ.get('DISPLAY'))}")
        metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--no-deps", "--format-version", "1"], cwd=repo))
        bundle = Path(metadata["target_directory"]) / "release/bundle/appimage"
        build_log = logs / "build.log"
        started_ns = time.time_ns()
        build = run_command(["npm", "run", "tauri:build:release"], repo, build_log, 3600)
        build["log"] = evidence(build_log, output)
        receipt["commands"].append(build)
        if build["exit_code"] != 0:
            raise ValueError("Canonical locked AppImage build failed")
        if capture_source_identity(repo) != source:
            raise ValueError("Source changed during AppImage build")
        artifact = select_fresh_artifact(bundle, started_ns)
        archive = output / artifact.name
        if archive.exists():
            raise ValueError("Refusing to overwrite an existing AppImage evidence artifact")
        shutil.copy2(artifact, archive)
        receipt["artifact"] = evidence(archive, output)
        receipt["artifact"]["size_bytes"] = archive.stat().st_size
        with tempfile.TemporaryDirectory(prefix="gloss-appimage-replay-") as temporary:
            extraction = Path(temporary)
            extract_log = logs / "extract.log"
            extract = run_command([str(archive), "--appimage-extract"], extraction, extract_log, 120)
            extract["log"] = evidence(extract_log, output)
            receipt["commands"].append(extract)
            if extract["exit_code"] != 0:
                raise ValueError("AppImage clean extraction failed")
            payload = validate_payload(extraction / "squashfs-root", config["productName"])
            receipt["payload"] = payload
            prebuilt = {"schema": "gloss-desktop-prebuilt/v1", "source": source,
                        **{key: payload[key] for key in ("application", "application_sha256", "binary", "binary_sha256")},
                        "artifact_sha256": receipt["artifact"]["sha256"],
                        "build_command": build["command"], "build_log": str(build_log), "build_exit_code": 0}
            manifest = logs / "prebuilt-config.json"
            manifest.write_text(json.dumps(prebuilt, indent=2) + "\n")
            desktop_output = output / ("real-model-workflow" if args.require_integrated else "desktop-replay")
            desktop_log = logs / "desktop-replay.log"
            if args.require_integrated:
                # Inherit the required outer DBus session; directly own the
                # canary leader so it can cooperate with timeout cancellation.
                command = [sys.executable, str(repo / "scripts/live_ollama_canary.py"),
                           "--repo", str(repo), "--desktop", "--prebuilt-config", str(manifest),
                           "--output", str(desktop_output)]
            else:
                command = ["dbus-run-session", "--", sys.executable, str(repo / "scripts/live_desktop_smoke.py"),
                           "--repo", str(repo), "--prebuilt-config", str(manifest),
                           "--require-baseline", "--output", str(desktop_output)]
            replay = run_command(command, extraction, desktop_log, 3300 if args.require_integrated else 600,
                                 cooperative_cleanup=args.require_integrated)
            replay["log"] = evidence(desktop_log, output)
            receipt["commands"].append(replay)
            if replay["exit_code"] != 0:
                raise ValueError("Extracted AppImage native desktop replay failed")
            if args.require_integrated:
                if load_prebuilt_config(manifest, source) != prebuilt:
                    raise ValueError("Extracted package manifest changed during real-model replay")
                desktop_receipt = require_packaged_workflow(desktop_output, source, prebuilt)
                receipt["canary_receipt"] = evidence(desktop_output / "receipt.json", output)
            else:
                desktop_receipt = desktop_output / "LIVE_DESKTOP_SMOKE_RECEIPT.json"
                require_packaged_baseline(json.loads(desktop_receipt.read_text()), source, receipt["artifact"]["sha256"])
            receipt["desktop_receipt"] = evidence(desktop_receipt, output)
        if file_sha256(archive) != receipt["artifact"]["sha256"]:
            raise ValueError("AppImage artifact changed during package replay")
        receipt["installed_launch_exercised"] = True
        receipt["package_smoke_passed"] = True
        receipt["status"] = "pass"
    except PackageBlocked as error:
        receipt["failures"].append(str(error))
    except (OSError, ValueError, KeyError, subprocess.SubprocessError) as error:
        receipt["status"] = "fail"
        receipt["failures"].append(str(error))
    finally:
        if "source_before" in receipt:
            try:
                receipt["source_after"] = capture_source_identity(repo)
                receipt["source_unchanged"] = receipt["source_after"] == receipt["source_before"]
                if not receipt["source_unchanged"]:
                    receipt["status"] = "fail"
                    receipt["package_smoke_passed"] = False
                    receipt["failures"].append("Source changed during package validation")
            except (OSError, ValueError, subprocess.SubprocessError) as error:
                receipt["status"] = "fail"
                receipt["package_smoke_passed"] = False
                receipt["failures"].append(f"Could not recheck source identity: {error}")
        if args.require_integrated:
            receipt["integrated_status"] = receipt["status"]
            if receipt["status"] == "pass":
                receipt["remaining_blockers"] = ["Signing and non-Linux/non-Ubuntu distribution compatibility are not certified."]
        receipt["finished_at"] = now()
        receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")
        print(json.dumps({"status": receipt["status"], "receipt": str(receipt_path), "failures": receipt["failures"]}, indent=2))
    return 0 if receipt["status"] == "pass" else 2 if receipt["status"] == "blocked" else 1


if __name__ == "__main__":
    raise SystemExit(main())
