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
    sources = text(repo / "src-tauri/src/commands/sources/mod.rs")
    ui = text(repo / "src/components/sources/SourcesPanel.tsx")
    run_id = current_run(repo) or "__missing__"
    receipt_path = repo / "docs/codex-runs" / run_id / "AUDIO_METADATA_RECEIPT.json"

    for marker in [
        'key: "audio"',
        'source_type: Some("audio")',
        "AudioMetadataReceiptV1",
        "cached Whisper CLI transcription",
    ]:
        if marker not in capability:
            failures.append(f"missing audio capability marker: {marker}")

    for marker in [
        "ExtractAudioMetadata",
        "audio_metadata_probe",
        "[source_audio_path]",
        "audio_metadata_description",
        "Transcription:",
        "audio_metadata_description_is_searchable_and_discloses_transcription_status",
    ]:
        if marker not in jobs:
            failures.append(f"missing audio metadata job marker: {marker}")

    for marker in [
        "queue_audio_metadata_job",
        'Some("audio") => MAX_AUDIO_FILE_SIZE',
        '"audio" => match queue_audio_metadata_job',
    ]:
        if marker not in sources:
            failures.append(f"missing audio source routing marker: {marker}")

    if "audio metadata" not in ui:
        failures.append("source panel does not disclose audio metadata support")
    if '"mp3"' not in ui or '"wav"' not in ui or '"m4a"' not in ui:
        failures.append("source picker does not include expected audio extensions")

    if not receipt_path.exists():
        failures.append(f"missing audio metadata receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid audio metadata receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "AudioMetadataReceiptV1":
            failures.append("audio metadata receipt schema mismatch")
        if not receipt.get("ffprobe_tool_receipt_required"):
            failures.append("audio receipt does not require ffprobe tool receipt")
        if receipt.get("transcription_support") not in {"cached_whisper_optional", "not_implemented"}:
            failures.append("audio receipt does not explicitly mark transcription support boundary")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
