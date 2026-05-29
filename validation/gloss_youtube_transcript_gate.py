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
    extract = text(repo / "src-tauri/src/ingestion/extract.rs")
    sources = text(repo / "src-tauri/src/commands/sources/mod.rs")
    lib = text(repo / "src-tauri/src/lib.rs")
    tauri = text(repo / "src/lib/tauri.ts")
    store = text(repo / "src/stores/sourceStore.ts")
    ui = text(repo / "src/components/sources/SourcesPanel.tsx")
    run_id = current_run(repo) or "__missing__"
    receipt_path = repo / "docs/codex-runs" / run_id / "YOUTUBE_TRANSCRIPT_RECEIPT.json"

    for marker in [
        'key: "youtube_transcript"',
        'source_type: Some("youtube")',
        "SupportedDegraded",
        "YouTubeTranscriptReceiptV1",
        "public YouTube watch URLs",
    ]:
        if marker not in capability:
            failures.append(f"missing YouTube capability marker: {marker}")

    if '"paste" | "url" | "youtube"' not in extract:
        failures.append("extract_text does not route YouTube transcript sources through stored content_text")

    for marker in [
        "add_source_youtube_transcript",
        "YouTubeTranscriptReceipt",
        "MAX_YOUTUBE_TRANSCRIPT_BYTES",
        "MAX_YOUTUBE_TRANSCRIPT_SEGMENTS",
        "canonical_youtube_watch_url",
        "youtube_caption_base_url",
        "json3_transcript_segments",
        "transcript_text_and_spans",
        "timestamp_spans",
        "youtube_transcript_parses_strict_url_and_caption_track",
        "youtube_transcript_json3_formats_timestamped_text_and_spans",
    ]:
        if marker not in sources:
            failures.append(f"missing YouTube source marker: {marker}")

    if "add_source_youtube_transcript" not in lib:
        failures.append("YouTube transcript command is not registered in Tauri handler")
    if "addSourceYouTubeTranscript" not in tauri:
        failures.append("frontend Tauri wrapper does not expose YouTube transcript import")
    if "addSourceYouTubeTranscript" not in store:
        failures.append("source store does not expose YouTube transcript import")
    for marker in [
        "YouTube transcript fetch",
        "Add YouTube Transcript",
        "video download",
        "authenticated YouTube access",
    ]:
        if marker not in ui:
            failures.append(f"source panel missing YouTube transcript disclosure/control marker: {marker}")

    if not receipt_path.exists():
        failures.append(f"missing YouTube transcript receipt: {receipt_path.relative_to(repo)}")
    else:
        try:
            receipt = json.loads(receipt_path.read_text())
        except Exception as exc:
            failures.append(f"invalid YouTube transcript receipt JSON: {exc}")
            receipt = {}
        if receipt.get("schema") != "YouTubeTranscriptImplementationReceiptV1":
            failures.append("YouTube transcript implementation receipt schema mismatch")
        if receipt.get("support") != "supported_degraded":
            failures.append("YouTube transcript support is not supported_degraded")
        if not receipt.get("network_consent_required"):
            failures.append("YouTube transcript receipt does not require explicit network consent")
        if not receipt.get("timestamp_spans_recorded"):
            failures.append("YouTube transcript receipt does not record timestamp span support")
        if "YouTubeTranscriptReceiptV1" not in receipt.get("runtime_receipt_schema", ""):
            failures.append("YouTube transcript receipt does not name runtime receipt schema")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
