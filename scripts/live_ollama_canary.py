#!/usr/bin/env python3
"""Run real tiny-model owners on a disposable GitHub-hosted Linux runner.

No existing Ollama service, user model directory, credentials, cloud provider,
GUI or HTTP fixture is used. A missing service/model is a failed required gate.
The downloaded release, model identities, command output and native artifacts
are bound to the source snapshot in the resulting evidence directory.
"""
from __future__ import annotations

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import signal
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request

from source_snapshot import capture_source_identity

# Official release asset digests, verified against the release's expanded assets.
# https://github.com/ollama/ollama/releases/expanded_assets/v0.11.10
VERSION = "0.11.10"
ASSET = "ollama-linux-amd64.tgz"
ASSET_SHA256 = "da2be71e972752e3099ef1d1226050d2b3d1ce79a4fec3bdb6edc70a7601e060"
# The official expanded-assets page reports 1.16 GB for this full AMD64 bundle;
# 1.5 GB allows approximately 29% unit/transfer margin, not an unbounded fetch.
MAX_ARCHIVE_BYTES = 1_500_000_000
RELEASE = f"https://github.com/ollama/ollama/releases/download/v{VERSION}"
MODELS = {
    # Official tags: https://ollama.com/library/all-minilm:22m
    "all-minilm:22m": {"digest_prefix": "1b226e2802db", "max_bytes": 100_000_000},
    # Official tags: https://ollama.com/library/qwen3:0.6b
    "qwen3:0.6b": {"digest_prefix": "7df6b6e09427", "max_bytes": 700_000_000},
}
ENDPOINT = "http://127.0.0.1:11435"
TEST = "live_ollama_embed_publish_reload_chat_and_precancel"


def now() -> str:
    return datetime.now(timezone.utc).isoformat()


def sha256(path: Path) -> str:
    with path.open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def validate_release(archive: Path, checksums: Path) -> dict:
    matching = [line.split()[0] for line in checksums.read_text().splitlines()
                if len(line.split()) == 2 and Path(line.split()[1].lstrip("*")).name == ASSET]
    if matching != [ASSET_SHA256]:
        raise ValueError("Published checksum does not match the reviewed pinned release")
    actual = sha256(archive)
    if actual != ASSET_SHA256:
        raise ValueError("Downloaded Ollama archive SHA256 mismatch")
    return {"version": VERSION, "asset": ASSET, "url": f"{RELEASE}/{ASSET}",
            "sha256": actual, "bytes": archive.stat().st_size,
            "published_checksum_matched": True}


def validate_models(tags: dict) -> dict:
    found = {}
    for name, expected in MODELS.items():
        rows = [row for row in tags.get("models", []) if row.get("name") == name]
        if len(rows) != 1:
            raise ValueError(f"Required live model unavailable or duplicated: {name}")
        row = rows[0]
        digest = row.get("digest", "")
        size = row.get("size", 0)
        if (not isinstance(digest, str) or len(digest) != 64
                or any(char not in "0123456789abcdef" for char in digest)
                or not digest.startswith(expected["digest_prefix"])):
            raise ValueError(f"Required model digest changed: {name}")
        if type(size) is not int or not 0 < size <= expected["max_bytes"]:
            raise ValueError(f"Required model exceeds reviewed download size: {name}")
        found[name] = row
    return found


def api(path: str, body: dict | None = None) -> dict:
    request = urllib.request.Request(ENDPOINT + path,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Content-Type": "application/json"})
    # Never let environment proxies change the loopback target.
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(request, timeout=5) as response:
        data = response.read(1024 * 1024 + 1)
    if len(data) > 1024 * 1024:
        raise ValueError("Canary metadata response exceeds 1 MiB")
    return json.loads(data)


def terminate_group(process: subprocess.Popen) -> None:
    """Stop owned compilers/model runners together with their parent command."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        pass
    # The parent can exit before its children; still close the owned group.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=5)


def execute(root: Path, output: Path) -> int:
    output.mkdir(parents=True, exist_ok=False)
    receipt = {"schema": "GlossHostedOllamaCanaryV1", "status": "running",
        "started_utc": now(), "live_service_exercised": False,
        "source": capture_source_identity(root), "commands": [],
        "pins": {"version": VERSION, "archive_sha256": ASSET_SHA256, "models": MODELS},
        "coverage_limits": ["Hosted synthetic tiny-model canary only",
                            "No user-installed model, desktop GUI, or Tauri orchestration proof"]}
    receipt_path = output / "receipt.json"

    def save() -> None:
        receipt_path.write_text(json.dumps(receipt, indent=2) + "\n")

    def run(label: str, command: list[str], seconds: int, env: dict | None = None) -> None:
        entry = {"id": label, "argv": command, "started_utc": now(), "timeout_seconds": seconds}
        receipt["commands"].append(entry)
        save()
        with (output / f"{label}.log").open("wb") as log:
            process = subprocess.Popen(command, cwd=root, env=env, stdout=log,
                                       stderr=subprocess.STDOUT, start_new_session=True)
            try:
                entry["exit_code"] = process.wait(timeout=seconds)
            except subprocess.TimeoutExpired:
                terminate_group(process)
                entry["exit_code"] = 124
                entry["error"] = "command deadline exceeded"
        entry["finished_utc"] = now()
        save()
        if entry["exit_code"] != 0:
            raise RuntimeError(f"{label} failed (exit {entry['exit_code']}); see {label}.log")

    save()
    try:
        if (os.environ.get("GITHUB_ACTIONS") != "true"
                or os.environ.get("RUNNER_ENVIRONMENT") != "github-hosted"
                or platform.system() != "Linux" or platform.machine() != "x86_64"):
            raise RuntimeError("This gate requires an isolated GitHub-hosted Linux x86_64 runner")
        # A pre-existing identity belongs to another installation; never reuse it.
        if (Path.home() / ".ollama").exists():
            raise RuntimeError("Refusing to use an existing Ollama installation identity")
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", 11435))
        receipt["runner"] = {"os": platform.platform(), "machine": platform.machine(),
                              "environment": os.environ["RUNNER_ENVIRONMENT"]}
        with tempfile.TemporaryDirectory(prefix="gloss-live-ollama-", dir=os.environ.get("RUNNER_TEMP")) as scratch:
            work = Path(scratch)
            archive = work / ASSET
            checksums = output / "upstream-sha256sum.txt"
            run("download-checksums", ["curl", "--fail", "--location", "--silent", "--show-error",
                "--proto", "=https", "--connect-timeout", "15", "--max-time", "30",
                "--max-filesize", "1048576", "--output", str(checksums), f"{RELEASE}/sha256sum.txt"], 40)
            run("download-runtime", ["curl", "--fail", "--location", "--silent", "--show-error",
                "--proto", "=https", "--connect-timeout", "15", "--max-time", "360",
                "--max-filesize", str(MAX_ARCHIVE_BYTES), "--output", str(archive), f"{RELEASE}/{ASSET}"], 370)
            receipt["runtime_download"] = validate_release(archive, checksums)
            run("extract-runtime", ["tar", "-xzf", str(archive), "-C", str(work)], 90)
            binary = work / "bin" / "ollama"
            if not binary.is_file():
                raise RuntimeError("Pinned archive did not contain bin/ollama")
            receipt["runtime_binary_sha256"] = sha256(binary)
            # Preserve the hosted runner's existing HOME; models and working files
            # use an isolated directory. No user/API credentials enter this env.
            service_env = {key: os.environ[key] for key in ("PATH", "HOME", "TMPDIR") if key in os.environ}
            service_env.update({"LANG": "C.UTF-8", "OLLAMA_HOST": "127.0.0.1:11435",
                "OLLAMA_MODELS": str(work / "models"), "OLLAMA_NO_CLOUD": "1",
                "OLLAMA_MAX_LOADED_MODELS": "1", "OLLAMA_NUM_PARALLEL": "1",
                "OLLAMA_KEEP_ALIVE": "60s", "OLLAMA_CONTEXT_LENGTH": "1024",
                "CUDA_VISIBLE_DEVICES": "-1", "ROCR_VISIBLE_DEVICES": "-1"})
            with (output / "ollama-service.log").open("wb") as log:
                service = subprocess.Popen([str(binary), "serve"], cwd=work,
                    env=service_env, stdout=log, stderr=subprocess.STDOUT,
                    start_new_session=True)
                try:
                    deadline = time.monotonic() + 30
                    while True:
                        if service.poll() is not None:
                            raise RuntimeError("Isolated Ollama service exited during startup")
                        try:
                            version = api("/api/version")
                            break
                        except (OSError, ValueError):
                            if time.monotonic() >= deadline:
                                raise RuntimeError("Isolated Ollama service unavailable after 30s")
                            time.sleep(0.25)
                    if version.get("version") != VERSION:
                        raise RuntimeError(f"Service version does not match pinned release: {version}")
                    (output / "service-version.json").write_text(json.dumps(version, indent=2) + "\n")
                    run("ollama-version", [str(binary), "--version"], 10, service_env)
                    for name, seconds in [("all-minilm:22m", 120), ("qwen3:0.6b", 240)]:
                        run("pull-" + name.split(":")[0], [str(binary), "pull", name], seconds, service_env)
                    tags = api("/api/tags")
                    (output / "model-tags.json").write_text(json.dumps(tags, indent=2) + "\n")
                    models = validate_models(tags)
                    receipt["models"] = models
                    for name in MODELS:
                        (output / ("model-show-" + name.split(":")[0] + ".json")).write_text(
                            json.dumps(api("/api/show", {"model": name}), indent=2) + "\n")
                    test_env = os.environ.copy()
                    test_env.update({"GLOSS_LIVE_OLLAMA_CANARY": "1", "OLLAMA_HOST": "127.0.0.1:11435",
                        "GLOSS_LIVE_OLLAMA_RECEIPT": str(output / "owners.json"),
                        "GLOSS_LIVE_EMBED_DIGEST": models["all-minilm:22m"]["digest"],
                        "GLOSS_LIVE_CHAT_DIGEST": models["qwen3:0.6b"]["digest"]})
                    run("rust-live-owners", ["cargo", "test", "--locked", "--manifest-path",
                        "validation/native_harness/Cargo.toml", "--test", "live_ollama_canary", "--",
                        "--ignored", "--exact", TEST, "--nocapture", "--test-threads=1"], 720, test_env)
                    owner = json.loads((output / "owners.json").read_text())
                    if (owner.get("status") != "pass" or owner.get("real_service_exercised") is not True
                            or owner.get("http_fixture_used") is not False):
                        raise RuntimeError("Required live owner receipt is missing or invalid")
                    receipt["live_service_exercised"] = True
                    (output / "loaded-models.json").write_text(json.dumps(api("/api/ps"), indent=2) + "\n")
                finally:
                    terminate_group(service)
        receipt["source_after"] = capture_source_identity(root)
        if receipt["source_after"] != receipt["source"]:
            raise RuntimeError("Source changed during the live canary")
        receipt["status"] = "pass"
    except Exception as error:
        receipt["status"] = "fail"
        receipt["error"] = f"{type(error).__name__}: {error}"
    finally:
        receipt["finished_utc"] = now()
        receipt["evidence"] = [{"path": str(path.relative_to(output)), "sha256": sha256(path)}
            for path in sorted(output.rglob("*")) if path.is_file() and path != receipt_path]
        save()
    print(json.dumps({"status": receipt["status"], "receipt": str(receipt_path),
                      "error": receipt.get("error")}, indent=2))
    return 0 if receipt["status"] == "pass" else 1


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    return execute(args.repo.resolve(), args.output.resolve())


if __name__ == "__main__":
    raise SystemExit(main())
