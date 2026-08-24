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

    warnings: list[str] = []
    checks = {
        "features const": "FASTEMBED_DOWNLOAD_CONSENT" in features,
        "runtime default migration": (
            'set_setting(features::FASTEMBED_DOWNLOAD_CONSENT, "true")' in state
            and "unwrap_or(true)" in state
        ),
        "native provider policy": "EmbeddingService::from_configured_provider" in state,
        "shared consent policy": (
            "new_with_download_policy" in embed
            and "if !download_consent" in embed
            and "candle_model_is_cached" in embed
        ),
        "cache/consent tests": (
            "consent_required_only_when_model_is_missing" in embed
            and "candle_model_cache_detection_and_ref_repair" in embed
        ),
        "semantic runtime field": "fastembed_download_consent" in adapter,
        "semantic shared service": "shared_fastembed_service(&cache_dir, download_consent)" in adapter,
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
        if data.get("default_consent") is not True:
            warnings.append(
                "historical FastEmbed consent receipt records the pre-repair default; "
                "current source migration is the active policy"
            )
        if not data.get("native_dense_indexing_guarded"):
            failures.append("FastEmbed consent receipt does not mark native guard active")
        if not data.get("semantic_memory_fastembed_guarded"):
            failures.append("FastEmbed consent receipt does not mark semantic-memory guard active")

    print(json.dumps({"ok": not failures, "failures": failures, "warnings": warnings}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
