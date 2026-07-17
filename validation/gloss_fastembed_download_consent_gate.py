#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path


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

    features = text(repo / "src-tauri/src/features.rs")
    state = text(repo / "src-tauri/src/state.rs")
    embed = text(repo / "src-tauri/src/ingestion/embed.rs")
    adapter = text(repo / "src-tauri/src/memory/semantic_memory_adapter.rs")
    settings_ui = text(repo / "src/components/settings/SettingsDialog/index.tsx")
    receipt = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "FASTEMBED_DOWNLOAD_CONSENT_RECEIPT.json"
    )

    checks = {
        "features const": "FASTEMBED_DOWNLOAD_CONSENT" in features,
        "default false": 'set_setting(features::FASTEMBED_DOWNLOAD_CONSENT, "false")' in state,
        "native policy": "EmbeddingService::new_with_download_policy" in state,
        "shared policy helper": "require_fastembed_download_consent" in embed,
        "empty cache test": "empty_fastembed_cache_requires_explicit_download_consent" in embed,
        "semantic runtime field": "fastembed_download_consent" in adapter,
        "semantic policy helper": "require_fastembed_download_consent" in adapter,
        "ui consent toggle": "fastembed_download_consent" in settings_ui,
    }
    for name, ok in checks.items():
        if not ok:
            failures.append(f"missing {name}")

    if not receipt.exists():
        failures.append(f"missing FastEmbed consent receipt: {receipt.relative_to(repo)}")
    else:
        try:
            data = json.loads(receipt.read_text())
        except Exception as exc:
            failures.append(f"invalid FastEmbed consent receipt JSON: {exc}")
            data = {}
        if data.get("schema") != "FastEmbedDownloadConsentReceiptV1":
            failures.append("FastEmbed consent receipt schema mismatch")
        if data.get("default_consent") is not False:
            failures.append("FastEmbed consent receipt must record default_consent=false")
        if not data.get("native_dense_indexing_guarded"):
            failures.append("FastEmbed consent receipt does not mark native guard active")
        if not data.get("semantic_memory_fastembed_guarded"):
            failures.append("FastEmbed consent receipt does not mark semantic-memory guard active")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
