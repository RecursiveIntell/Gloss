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

    capability = text(repo / "src-tauri/src/ingestion/import_capability.rs")
    jobs = text(repo / "src-tauri/src/jobs/mod.rs")
    ui = text(repo / "src/components/sources/SourcesPanel.tsx")
    run_id = current_run(repo) or "__missing__"
    receipt_path = repo / "docs/codex-runs" / run_id / "AUDIO_TRANSCRIPTION_RECEIPT.json"

    for marker in [
        "AudioTranscriptionReceiptV1",
        "cached Whisper CLI transcription",
        "transcription is skipped unless a local Whisper model is already cached",
    ]:
        if marker not in capability:
            failures.append(f"missing audio transcription capability marker: {marker}")

    for marker in [
        "maybe_transcribe_audio",
        "audio_transcription_whisper",
        "GLOSS_WHISPER_MODEL",
        "GLOSS_WHISPER_MODEL_DIR",
        "cached_whisper_model_path",
        "MAX_AUDIO_TRANSCRIPTION_DURATION_SECS",
        "MAX_AUDIO_TRANSCRIPT_SEGMENTS",
        "whisper_transcript_text",
        "AudioProcessingMetadataV1",
        "tool_invocation_receipts",
        "whisper_transcript_json_formats_timestamped_text",
    ]:
        if marker not in jobs:
            failures.append(f"missing audio transcription job marker: {marker}")

    if "cached Whisper audio transcription" not in ui:
        failures.append("source panel does not disclose cached Whisper audio transcription support")
    if "legacy Office CLI extraction" not in ui:
        failures.append("source panel does not disclose degraded legacy Office CLI extraction")

    if not receipt_path.exists():
        failures.append(f"missing audio transcription receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid audio transcription receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "AudioTranscriptionImplementationReceiptV1":
            failures.append("audio transcription receipt schema mismatch")
        if receipt.get("support") != "supported_degraded":
            failures.append("audio transcription support is not supported_degraded")
        if not receipt.get("cached_model_required"):
            failures.append("audio transcription receipt does not require cached local model")
        if not receipt.get("tool_receipt_required"):
            failures.append("audio transcription receipt does not require tool invocation receipts")
        if receipt.get("runtime_receipt_schema") != "AudioProcessingMetadataV1":
            failures.append("audio transcription runtime metadata schema mismatch")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
