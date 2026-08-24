#!/usr/bin/env python3
import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

GATES = [
    # AGENTS.md required gates (mandatory)
    "validation/validate_source_send_gate.py",
    "validation/validate_frontend_event_routing.py",
    "validation/validate_chat_terminal_contract.py",
    "validation/validate_provider_lan_policy.py",
    "validation/validate_release_receipt_consistency.py",
    # Semantic / memory gates
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
    # Security / egress
    "validation/gloss_security_egress_gate.py",
    "validation/gloss_fastembed_download_consent_gate.py",
    "validation/gloss_secret_store_permissions_gate.py",
    "validation/gloss_tool_invocation_receipt_gate.py",
    "validation/gloss_path_redaction_gate.py",
    # Import / ingestion
    "validation/gloss_import_capability_gate.py",
    "validation/gloss_document_extractors_gate.py",
    "validation/gloss_legacy_office_extractors_gate.py",
    "validation/gloss_audio_metadata_gate.py",
    "validation/gloss_audio_transcription_gate.py",
    "validation/gloss_url_import_gate.py",
    "validation/gloss_youtube_transcript_gate.py",
    # Studio / notebook gates
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
    results = []
    for gate in GATES:
        if gate.startswith("validation/validate_"):
            # validate_* gates accept a positional repo path
            results.append(run(repo, [sys.executable, gate, str(repo)]))
        elif gate == "validation/gloss_live_receipt_gate.py":
            # needs --run-id
            results.append(run(repo, [sys.executable, gate, "--repo", str(repo), "--run-id", str(run_id or "")]))
        else:
            # gloss_* gates accept --repo flag
            results.append(run(repo, [sys.executable, gate, "--repo", str(repo)]))
    release_candidate_gate_passed = all(result["passed"] for result in results)
    # A scripted receipt is not live evidence. Require the canonical desktop
    # receipt to explicitly record a headed/live exercise and release-grade
    # status before promoting either release_ready or public_claim_ready.
    desktop_receipt = run_dir / "LIVE_DESKTOP_SMOKE_RECEIPT.json"
    desktop_release_grade = False
    if desktop_receipt.exists():
        try:
            desktop = json.loads(desktop_receipt.read_text())
            desktop_release_grade = (
                desktop.get("live_desktop_exercised") is True
                and desktop.get("release_grade") is True
                and not desktop.get("failures")
            )
        except (OSError, json.JSONDecodeError):
            desktop_release_grade = False
    release_ready = release_candidate_gate_passed and desktop_release_grade
    reason = (
        "release_ready: static gates passed and live desktop smoke is release-grade"
        if release_ready
        else "release_ready=false: static gates may pass, but a live release-grade desktop smoke receipt is missing"
    )
    payload = {
        "schema": "GlossReleaseCandidateGateV1",
        "run_id": run_id,
        "recorded_utc": datetime.now(timezone.utc).isoformat(),
        "release_candidate_gate_passed": release_candidate_gate_passed,
        "release_ready": release_ready,
        "public_claim_ready": release_ready,
        "live_desktop_release_grade": desktop_release_grade,
        "reason_release_ready_false": reason if not release_ready else "",
        "commands": results,
    }
    out = run_dir / "RELEASE_CANDIDATE_GATE_RESULTS.json"
    out.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n")
    print(json.dumps({"ok": release_candidate_gate_passed, "wrote": str(out), "failed": [r["cmd"][1] for r in results if not r["passed"]]}, indent=2))
    return 0 if release_candidate_gate_passed else 1

if __name__ == "__main__":
    raise SystemExit(main())
