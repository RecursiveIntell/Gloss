#!/usr/bin/env python3
"""Current-run release eligibility gate for Gloss."""
from __future__ import annotations
import argparse, json, re, sys
from pathlib import Path

RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"

def read_current_run(repo: Path) -> str | None:
    p = repo / "docs/codex-runs/CURRENT_RUN.md"
    if not p.exists():
        return None
    text = p.read_text(encoding="utf-8", errors="replace")
    if RUN_ID in text:
        return RUN_ID
    # fallback: first all-caps-ish run token
    m = re.search(r"[A-Z][A-Z0-9_\-]{5,}", text)
    return m.group(0) if m else text.strip().splitlines()[0].strip() if text.strip() else None

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    findings = []
    current = read_current_run(repo)
    if current != RUN_ID:
        findings.append({"severity":"error","code":"current-run-mismatch","detail":f"expected {RUN_ID}, got {current!r}"})
    run_dir = repo / "docs/codex-runs" / RUN_ID
    receipt_path = run_dir / "FINAL_RECEIPT.json"
    if not receipt_path.exists():
        findings.append({"severity":"error","code":"missing-final-receipt","detail":str(receipt_path.relative_to(repo))})
        receipt = {}
    else:
        try:
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        except Exception as exc:
            findings.append({"severity":"error","code":"invalid-final-receipt-json","detail":str(exc)})
            receipt = {}

    release_ready = bool(receipt.get("release_ready"))
    desktop = receipt.get("desktop_smoke") or {}
    validation = receipt.get("validation") or {}

    if release_ready:
        if not desktop.get("completed") or desktop.get("skipped"):
            findings.append({"severity":"error","code":"release-ready-without-desktop-smoke","detail":"release_ready true but desktop smoke not completed"})
        for key in ["npm_run_build", "cargo_fmt_check", "cargo_test_default", "active_validation_scope", "feature_flags_static", "gloss_button_up_gate", "fresh_unzip_replay"]:
            if validation.get(key) not in {"passed", "pass", True}:
                findings.append({"severity":"error","code":"release-ready-without-required-validation","detail":f"{key}={validation.get(key)!r}"})
    else:
        # Block release but exit nonzero only if the user is asking eligibility.
        findings.append({"severity":"error","code":"release-not-ready","detail":"FINAL_RECEIPT release_ready is false or missing"})

    result = {"eligible": not findings, "current_run": current, "run_id": RUN_ID, "findings": findings}
    print(json.dumps(result, indent=2))
    return 0 if not findings else 1

if __name__ == "__main__":
    raise SystemExit(main())
