#!/usr/bin/env python3
"""Final stabilization gate for the active Gloss release-candidate run."""
from __future__ import annotations
import argparse, json, re, subprocess, sys
from pathlib import Path

RUN_ID = "GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519"

def read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""

def run_json(cmd, repo: Path):
    try:
        out = subprocess.check_output(cmd, cwd=repo, text=True, stderr=subprocess.STDOUT)
        try:
            return json.loads(out), 0, out
        except Exception:
            return None, 0, out
    except subprocess.CalledProcessError as exc:
        out = exc.output or ""
        try:
            return json.loads(out), exc.returncode, out
        except Exception:
            return None, exc.returncode, out

def current_run(repo: Path):
    text = read(repo / "docs/codex-runs/CURRENT_RUN.md")
    return RUN_ID if RUN_ID in text else None

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    findings = []

    if current_run(repo) != RUN_ID:
        findings.append({"severity":"error","code":"current-run-not-active-release-candidate","detail":f"docs/codex-runs/CURRENT_RUN.md must point at {RUN_ID}"})

    run_dir = repo / "docs/codex-runs" / RUN_ID
    if not run_dir.exists():
        findings.append({"severity":"error","code":"missing-run-dir","detail":str(run_dir)})

    # Active validation scope gate.
    if (repo / "scripts/check_gloss_active_validation_scope.py").exists():
        data, rc, out = run_json([sys.executable, "scripts/check_gloss_active_validation_scope.py", "--repo", str(repo)], repo)
        if rc != 0:
            findings.append({"severity":"error","code":"active-validation-scope-failed","detail":out[-2000:]})
    else:
        findings.append({"severity":"error","code":"missing-active-scope-script","detail":"scripts/check_gloss_active_validation_scope.py missing"})

    # Feature flag static gate.
    if (repo / "scripts/check_feature_flags_static.py").exists():
        data, rc, out = run_json([sys.executable, "scripts/check_feature_flags_static.py", "--repo", str(repo)], repo)
        if rc != 0:
            findings.append({"severity":"error","code":"feature-flags-static-failed","detail":out[-2000:]})
    else:
        findings.append({"severity":"error","code":"missing-feature-flags-static-script","detail":"scripts/check_feature_flags_static.py missing"})

    # Root README product-facing gate.
    readme = read(repo / "README.md")
    head = readme[:1200].lower()
    if not readme.strip():
        findings.append({"severity":"error","code":"missing-readme","detail":"README.md empty/missing"})
    if "codex pass" in head or "chat runtime fix codex pass" in head or "start_here" in head:
        findings.append({"severity":"error","code":"readme-is-codex-pass","detail":"root README still appears to be a Codex pass README"})
    if "local-first" not in head and "local first" not in head:
        findings.append({"severity":"warning","code":"readme-missing-local-first","detail":"root README should describe local-first product stance"})

    # Final receipt gate.
    receipt_path = run_dir / "FINAL_RECEIPT.json"
    receipt = {}
    if not receipt_path.exists():
        findings.append({"severity":"error","code":"missing-final-receipt","detail":str(receipt_path.relative_to(repo) if receipt_path.is_absolute() else receipt_path)})
    else:
        try:
            receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
        except Exception as exc:
            findings.append({"severity":"error","code":"invalid-final-receipt","detail":str(exc)})

    desktop = receipt.get("desktop_smoke") or {}
    validation = receipt.get("validation") or {}
    flags = receipt.get("feature_flags") or {}
    release_ready = bool(receipt.get("release_ready"))

    if not flags.get("experimental_features_default_off", False):
        findings.append({"severity":"error","code":"experimental-features-not-default-off","detail":"receipt does not prove experimental defaults off"})
    if not flags.get("semantic_memory_preview_gated", False):
        findings.append({"severity":"error","code":"semantic-memory-preview-not-proven-gated","detail":"receipt does not prove semantic-memory preview gating"})
    if not desktop.get("completed"):
        findings.append({"severity":"error","code":"desktop-smoke-not-completed","detail":"desktop smoke not completed"})
    if desktop.get("skipped"):
        findings.append({"severity":"error","code":"desktop-smoke-skipped","detail":"desktop smoke skipped"})

    required_final_files = [
        "STARTUP_PREFLIGHT.md",
        "PHASE_REPORTS.md",
        "COMMANDS_RUN.md",
        "CHANGED_FILES.txt",
        "VALIDATION_RESULTS.md",
        "DESKTOP_SMOKE_RESULTS.md",
        "FINAL_RECEIPT.json",
        "FINAL_AUDITOR_HANDOFF.md",
        "ROLLBACK_PLAN.md",
    ]
    for name in required_final_files:
        if not (run_dir / name).exists():
            findings.append({"severity":"error","code":"missing-final-artifact","detail":name})

    if release_ready:
        for key in ["npm_run_build", "cargo_fmt_check", "cargo_test_default", "active_validation_scope", "feature_flags_static", "release_eligibility_current", "gloss_button_up_gate", "fresh_unzip_replay"]:
            if validation.get(key) not in {"passed", "pass", True}:
                findings.append({"severity":"error","code":"release-ready-without-required-validation","detail":f"{key}={validation.get(key)!r}"})
    else:
        findings.append({"severity":"error","code":"release-not-ready","detail":"release_ready false/missing; acceptable only if final response says GUI redesign is blocked"})

    errors = [f for f in findings if f.get("severity") == "error"]
    result = {
        "ok": not errors,
        "run_id": RUN_ID,
        "release_ready": release_ready and not errors,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1

if __name__ == "__main__":
    raise SystemExit(main())
