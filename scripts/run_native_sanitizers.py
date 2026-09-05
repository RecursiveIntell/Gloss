#!/usr/bin/env python3
"""Run real native dense-owner tests with C++ Address/LeakSanitizer on Linux.

This instruments USearch's C++ allocation boundary, not Rust. Leak checking is
mandatory here: a runtime/permission failure is a failed gate, never a pass.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

from source_snapshot import capture_source_identity


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--receipt", type=Path, required=True)
    args = parser.parse_args()
    root = args.root.resolve()
    source = capture_source_identity(root)
    result: dict[str, object] = {
        "schema": "GlossNativeSanitizerReceiptV1", "source_before": source,
        "instrumentation": "C++ AddressSanitizer and LeakSanitizer; Rust is not instrumented",
        "status": "failed", "commands": [],
    }
    exit_code = 1
    try:
        if sys.platform != "linux":
            raise RuntimeError("This gate requires Linux and libasan")
        probe = subprocess.run(["c++", "-print-file-name=libasan.so"], check=True,
                               capture_output=True, text=True)
        libasan = Path(probe.stdout.strip()).resolve(strict=True)
        environment = dict(os.environ)
        # Cargo gives this variable precedence over RUSTFLAGS. Inherited flags
        # must not silently disable the instrumentation requested by this gate.
        environment.pop("CARGO_ENCODED_RUSTFLAGS", None)
        environment["CARGO_TARGET_DIR"] = str(root / "target" / "native-sanitizers")
        environment["CXXFLAGS"] = "-fsanitize=address -fno-omit-frame-pointer"
        environment["RUSTFLAGS"] = "-C link-arg=-fsanitize=address"
        command = ["cargo", "rustc", "--locked", "--manifest-path",
                   "validation/native_harness/Cargo.toml", "--lib", "--profile", "test",
                   "--message-format=json",
                   "--", "-C", "link-arg=-lasan"]
        build = subprocess.run(command, cwd=root, env=environment, stdout=subprocess.PIPE,
                               text=True, check=False)
        result["commands"].append({"argv": command, "exit_code": build.returncode})
        if build.returncode:
            raise RuntimeError(f"Instrumented native build failed: exit={build.returncode}")
        artifacts = []
        for line in build.stdout.splitlines():
            event = json.loads(line)
            if (event.get("reason") == "compiler-artifact" and event.get("executable")
                    and event.get("profile", {}).get("test")
                    and event.get("target", {}).get("name") == "gloss_native_contract_tests"):
                artifacts.append(event["executable"])
        if len(artifacts) != 1:
            raise RuntimeError(f"Expected one native test executable, found {len(artifacts)}")
        environment["LD_PRELOAD"] = str(libasan)
        environment["ASAN_OPTIONS"] = "detect_leaks=1:halt_on_error=1"
        # Prevent inherited options from suppressing leak errors or changing exit status.
        environment["LSAN_OPTIONS"] = "exitcode=23"
        command = [artifacts[0], "dense::audit_dense_tests", "--test-threads=1"]
        run = subprocess.run(command, cwd=root, env=environment, capture_output=True,
                             text=True, check=False)
        sys.stderr.write(run.stdout + run.stderr)
        result["commands"].append({"argv": command, "exit_code": run.returncode})
        result["test_output"] = run.stdout
        result["sanitizer_output"] = run.stderr
        result["source_after"] = capture_source_identity(root)
        if result["source_after"] != source:
            raise RuntimeError("Source changed during sanitizer run")
        # A misspelled filter exits successfully while running zero tests.
        if "running 0 tests" in run.stdout or "test result: ok." not in run.stdout:
            raise RuntimeError("Native test execution did not report a nonempty passing suite")
        exit_code = run.returncode
        if exit_code == 0:
            result["status"] = "passed"
    except (OSError, ValueError, RuntimeError, subprocess.SubprocessError) as exc:
        result["error"] = str(exc)
    result["exit_code"] = exit_code
    args.receipt.parent.mkdir(parents=True, exist_ok=True)
    args.receipt.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"status": result["status"], "receipt": str(args.receipt)}))
    return exit_code


if __name__ == "__main__":
    raise SystemExit(main())
