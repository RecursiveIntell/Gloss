#!/usr/bin/env python3
import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

GATES = [
    "validation/gloss_current_run_truth_gate.py",
    "validation/gloss_stale_pass_surface_gate.py",
    "validation/gloss_package_scope_gate.py",
    "validation/gloss_semantic_memory_runtime_truth_gate.py",
    "validation/gloss_retrieval_decision_gate.py",
    "validation/gloss_generation_receipt_gate.py",
    "validation/gloss_prompt_receipt_gate.py",
    "validation/gloss_decoding_settings_gate.py",
    "validation/gloss_timeout_partial_continuation_gate.py",
    "validation/gloss_inspector_dock_gate.py",
    "validation/gloss_live_semantic_memory_smoke_gate.py",
    "validation/gloss_turboquant_runtime_gate.py",
    "validation/gloss_security_egress_gate.py",
    "validation/gloss_fastembed_download_consent_gate.py",
    "validation/gloss_secret_store_permissions_gate.py",
    "validation/gloss_tool_invocation_receipt_gate.py",
    "validation/gloss_path_redaction_gate.py",
    "validation/gloss_import_capability_gate.py",
    "validation/gloss_document_extractors_gate.py",
    "validation/gloss_legacy_office_extractors_gate.py",
    "validation/gloss_audio_metadata_gate.py",
    "validation/gloss_audio_transcription_gate.py",
    "validation/gloss_url_import_gate.py",
    "validation/gloss_youtube_transcript_gate.py",
    "validation/gloss_studio_artifacts_gate.py",
    "validation/gloss_db_doctor_gate.py",
    "validation/gloss_failed_import_quarantine_gate.py",
    "validation/gloss_import_performance_gate.py",
    "validation/gloss_notebook_portability_gate.py",
    "validation/gloss_desktop_smoke_gate.py",
]

def run(repo: Path, cmd: list[str]):
    proc = subprocess.run(cmd, cwd=repo, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    return {"cmd": cmd, "returncode": proc.returncode, "passed": proc.returncode == 0, "stdout_tail": proc.stdout[-4000:], "stderr_tail": proc.stderr[-4000:]}

def current_run(repo: Path) -> str | None:
    import re
    text = (repo / "docs/codex-runs/CURRENT_RUN.md").read_text(errors="ignore")
    match = re.search(r"Current run:\s*`?([^`\n]+)`?", text)
    return match.group(1).strip() if match else None

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--run-id", default="CURRENT")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    run_id = current_run(repo) if args.run_id == "CURRENT" else args.run_id
    run_dir = repo / "docs/codex-runs" / (run_id or "__missing__")
    run_dir.mkdir(parents=True, exist_ok=True)
    results = [run(repo, [sys.executable, gate, "--repo", "."]) for gate in GATES]
    release_candidate_gate_passed = all(result["passed"] for result in results)
    payload = {
        "schema": "GlossReleaseCandidateGateV1",
        "run_id": run_id,
        "recorded_utc": datetime.now(timezone.utc).isoformat(),
        "release_candidate_gate_passed": release_candidate_gate_passed,
        "release_ready": False,
        "public_claim_ready": False,
        "reason_release_ready_false": "This aggregate proves only current release-candidate sub-gates. Full broad-spec, live desktop GUI smoke, installed package post-launch workflow/AppImage packaging, and full benchmark/UI frame-budget performance certification remain separate release blockers.",
        "commands": results,
    }
    out = run_dir / "RELEASE_CANDIDATE_GATE_RESULTS.json"
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"ok": release_candidate_gate_passed, "wrote": str(out), "failed": [r["cmd"][1] for r in results if not r["passed"]]}, indent=2))
    return 0 if release_candidate_gate_passed else 1

if __name__ == "__main__":
    raise SystemExit(main())
