#!/usr/bin/env python3
"""Quarantine stale Gloss Codex/pass artifacts without destructive deletion.

Default mode is dry-run. Use --apply only after reviewing candidates. The script
moves stale root-level pass artifacts into docs/codex-runs/archive/stale-root-<run-id>/
and writes STALE_PASS_CLEANUP_MANIFEST.json under the current run directory.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
from datetime import datetime, timezone
from pathlib import Path
from typing import Dict, List, Any

ROOT_CANDIDATES = [
    "ACCEPTANCE_GATES.md",
    "AUDIT_COMMAND_RESULTS.txt",
    "CLEANUP_AND_QUARANTINE_RULES.md",
    "FORBIDDEN_LEFTOVERS.md",
    "HARD_AUDIT_FINDINGS.md",
    "ISSUE_LEDGER_NEXT_PASS.csv",
    "PACK_METADATA.json",
    "PROMPT_TO_PASTE_IN_CODEX.md",
    "ROLLBACK_PLAN.md",
    "SOURCE_OF_TRUTH_MAP.md",
    "AGENTS.md.bak.GLOSS_P33_RELEASE_CANDIDATE_SM_TQ_SETTINGS_GUI_20260519",
    "p33boot",
    "phase_prompts",
    "manual_injections",
    "prompts",
    "receipts",
    "reports",
    "codex-control-pack",
]
SKILL_STALE_MARKERS = ["p31", "p32", "p32r3", "p33", "p34", "p35", "p36"]


def file_digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def tree_digest(path: Path) -> Dict[str, Any]:
    files: List[Dict[str, Any]] = []
    if path.is_file():
        return {"kind": "file", "files": [{"path": path.name, "sha256": file_digest(path), "bytes": path.stat().st_size}]}
    for child in sorted(p for p in path.rglob("*") if p.is_file()):
        rel = child.relative_to(path).as_posix()
        files.append({"path": rel, "sha256": file_digest(child), "bytes": child.stat().st_size})
    h = hashlib.sha256()
    for rec in files:
        h.update(rec["path"].encode("utf-8"))
        h.update(rec["sha256"].encode("utf-8"))
    return {"kind": "directory", "tree_sha256": h.hexdigest(), "files": files}


def collect_candidates(repo: Path) -> List[Path]:
    out: List[Path] = []
    for name in ROOT_CANDIDATES:
        p = repo / name
        if p.exists():
            out.append(p)
    agents_skills = repo / ".agents" / "skills"
    if agents_skills.exists():
        for skill in sorted(agents_skills.iterdir()):
            lname = skill.name.lower()
            if any(marker in lname for marker in SKILL_STALE_MARKERS):
                out.append(skill)
    # Do not move docs/codex-runs history; only root-level active contamination.
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    parser.add_argument("--run-id", default="GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525")
    parser.add_argument("--apply", action="store_true")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    run_dir = repo / "docs" / "codex-runs" / args.run_id
    archive_root = repo / "docs" / "codex-runs" / "archive" / f"stale-root-{args.run_id}"
    run_dir.mkdir(parents=True, exist_ok=True)
    archive_root.mkdir(parents=True, exist_ok=True)

    manifest: Dict[str, Any] = {
        "schema": "GlossStalePassCleanupManifestV1",
        "run_id": args.run_id,
        "recorded_utc": datetime.now(timezone.utc).isoformat(),
        "repo": str(repo),
        "apply": args.apply,
        "candidates": [],
        "entries": [],
        "moved": [],
        "skipped": [],
    }

    for src in collect_candidates(repo):
        rel = src.relative_to(repo).as_posix()
        digest = tree_digest(src)
        sha256_tree_digest = digest.get("tree_sha256") or digest["files"][0]["sha256"]
        rec: Dict[str, Any] = {"source": rel, "digest": digest}
        manifest["candidates"].append(rec)
        dest = archive_root / rel.replace("/", "__")
        if not args.apply:
            manifest["skipped"].append({"source": rel, "reason": "dry-run"})
            continue
        if dest.exists():
            manifest["skipped"].append({"source": rel, "reason": f"destination exists: {dest}"})
            continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        shutil.move(str(src), str(dest))
        dest_rel = dest.relative_to(repo).as_posix()
        manifest["moved"].append({"source": rel, "destination": dest_rel})
        manifest["entries"].append(
            {
                "path": rel,
                "action": "moved_to_archive",
                "destination": dest_rel,
                "reason": "old pass artifact at active root",
                "sha256_tree_digest": sha256_tree_digest,
            }
        )

    out = run_dir / "STALE_PASS_CLEANUP_MANIFEST.json"
    out.write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"wrote": str(out), "apply": args.apply, "candidates": len(manifest["candidates"]), "moved": len(manifest["moved"])}, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
