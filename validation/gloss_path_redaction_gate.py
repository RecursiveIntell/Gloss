#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path


RUNTIME_FILES = [
    "src-tauri/src/state.rs",
    "src-tauri/src/jobs/mod.rs",
    "src-tauri/src/ingestion/embed.rs",
    "src-tauri/src/memory/semantic_memory_adapter.rs",
    "src-tauri/src/commands/settings.rs",
    "src-tauri/src/commands/sources/mod.rs",
    "src-tauri/src/tool_invocation.rs",
]


def text(path: Path) -> str:
    return path.read_text(errors="ignore") if path.exists() else ""


def current_run(repo: Path) -> str | None:
    match = re.search(
        r"Current run:\s*`?([^`\n]+)`?",
        text(repo / "docs/codex-runs/CURRENT_RUN.md"),
    )
    return match.group(1).strip() if match else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    failures: list[str] = []
    helper = text(repo / "src-tauri/src/redaction.rs")
    receipt = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "PATH_REDACTION_RECEIPT.json"
    )

    checks = {
        "redaction helper": "pub fn redact_path" in helper and "pub fn redact_text_paths" in helper,
        "redaction tests": "redact_path_keeps_only_leaf_for_absolute_paths" in helper,
        "tool stderr redaction": "redact_text_paths" in text(repo / "src-tauri/src/tool_invocation.rs"),
        "embedding errors omit raw cache paths": (
            "cache_dir" in text(repo / "src-tauri/src/ingestion/embed.rs")
            and ".display()" not in text(repo / "src-tauri/src/ingestion/embed.rs")
            and "hf_home.display()" not in text(repo / "src-tauri/src/ingestion/embed.rs")
        ),
        "settings diagnostics redaction": "cache_dir: redact_path(&cache_dir)" in text(repo / "src-tauri/src/commands/settings.rs"),
        "jobs image redaction": "redact_path(&full_path)" in text(repo / "src-tauri/src/jobs/mod.rs"),
        "state tracing redaction": "redact_path(&notebook_db_path)" in text(repo / "src-tauri/src/state.rs"),
    }
    for name, ok in checks.items():
        if not ok:
            failures.append(f"missing {name}")

    for rel in RUNTIME_FILES:
        body = text(repo / rel)
        if ".display()" in body:
            failures.append(f"{rel} still formats raw Path::display()")

    if not receipt.exists():
        failures.append(f"missing path redaction receipt: {receipt.relative_to(repo)}")
    else:
        try:
            data = json.loads(receipt.read_text())
        except Exception as exc:
            failures.append(f"invalid path redaction receipt JSON: {exc}")
            data = {}
        if data.get("schema") != "PathRedactionReceiptV1":
            failures.append("path redaction receipt schema mismatch")
        if not data.get("runtime_path_redaction_active"):
            failures.append("path redaction receipt does not mark runtime redaction active")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
