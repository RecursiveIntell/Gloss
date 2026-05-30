#!/usr/bin/env python3
import argparse
import json
import os
from pathlib import Path

ALLOWED_TOP = {"Gloss", "Libraries"}

def load_manifest(repo: Path, explicit: str | None):
    if explicit:
        path = repo / explicit
        if path.exists():
            return path, json.loads(path.read_text())
        return None, None
    # Try run directory first
    run_dir = repo / "docs/codex-runs"
    if run_dir.exists():
        manifests = sorted(run_dir.rglob("Gloss-generic-rust-next-codex-context-*.manifest.json"))
        if manifests:
            path = manifests[-1]
            return path, json.loads(path.read_text())
    # Fall back to repo root (legacy)
    manifests = sorted(repo.glob("Gloss-generic-rust-next-codex-context-*.manifest.json"))
    if manifests:
        path = manifests[-1]
        return path, json.loads(path.read_text())
    return None, None

def included_paths(data):
    for rec in data.get("files", []):
        if isinstance(rec, dict) and rec.get("path"):
            yield rec["path"]
    for rec in data.get("decisions", []):
        if isinstance(rec, dict) and rec.get("decision") == "include" and rec.get("path"):
            yield rec["path"]

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--manifest")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    path, data = load_manifest(repo, args.manifest)
    failures = []
    warnings = []
    if data is None:
        warnings.append("no Gloss package manifest found (generated sidecar absent is clean)")
        print(json.dumps({"ok": True, "manifest": None, "failures": failures, "warnings": warnings}, indent=2))
        return 0
    violations = sorted({p for p in included_paths(data) if p.split(os.sep, 1)[0] not in ALLOWED_TOP})
    if violations:
        failures.append(f"package manifest includes {len(violations)} paths outside Gloss/Libraries")
    print(json.dumps({
        "ok": not failures,
        "manifest": str(path.relative_to(repo)),
        "top_level_violations_count": len(violations),
        "top_level_violations": violations[:100],
        "failures": failures,
        "warnings": [],
    }, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
