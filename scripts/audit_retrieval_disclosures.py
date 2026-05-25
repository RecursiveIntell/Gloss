#!/usr/bin/env python3
import argparse
import json
import pathlib
import sys

required = [
    "backend_requested", "backend_used", "retrieval_mode", "fallback_used",
    "fallback_reason", "degradation_markers", "source_scope_mode",
    "requested_source_ids", "selected_source_ids", "effective_source_ids", "invalid_source_ids", "excluded_source_ids",
    "invalid_source_count", "effective_source_count", "excluded_source_count",
    "context_passage_count", "citation_valid_count", "citation_invalid_count",
    "omitted_candidate_count", "source_scope_preserved", "index_status",
    "link_status", "receipt_id",
]

def audit_payload(payload):
    items = payload if isinstance(payload, list) else [payload]
    errors = []
    for i, item in enumerate(items):
        evidence = item.get("evidence") if isinstance(item, dict) else None
        citations = item.get("citations") if isinstance(item, dict) else None
        if not isinstance(evidence, dict):
            errors.append([i, "missing evidence object"])
            continue
        if not isinstance(citations, list):
            errors.append([i, "missing citations array"])
            citations = []
        for key in required:
            if key not in evidence:
                errors.append([i, f"missing evidence.{key}"])
        if evidence.get("fallback_used") and not evidence.get("fallback_reason"):
            errors.append([i, "fallback_used without fallback_reason"])
        for key in ["requested_source_ids", "selected_source_ids", "effective_source_ids", "invalid_source_ids", "excluded_source_ids"]:
            if key in evidence and not isinstance(evidence.get(key), list):
                errors.append([i, f"evidence.{key} must be an array"])
        if isinstance(evidence.get("excluded_source_ids"), list) and evidence.get("excluded_source_count") != len(evidence["excluded_source_ids"]):
            errors.append([i, "excluded_source_count does not match excluded_source_ids length"])
        if evidence.get("citation_valid_count", 0) > len(citations):
            errors.append([i, "citation_valid_count exceeds citations length"])
        if evidence.get("citation_invalid_count", 0) < 0:
            errors.append([i, "negative citation_invalid_count"])
        if evidence.get("context_passage_count", 0) < evidence.get("citation_valid_count", 0):
            errors.append([i, "context_passage_count < citation_valid_count"])
    return {"checked": len(items), "errors": errors, "decision": "pass" if not errors else "blocked"}


def latest_receipt(repo: pathlib.Path) -> pathlib.Path | None:
    current_run = None
    current_run_path = repo / "docs/codex-runs/CURRENT_RUN.md"
    if current_run_path.exists():
        for line in current_run_path.read_text(errors="replace").splitlines():
            if line.startswith("Current run:"):
                current_run = line.split(":", 1)[1].strip().strip("`")
                break

    candidates = [
        path for path in repo.rglob("*.json")
        if ".git" not in path.parts
        and "node_modules" not in path.parts
        and "fixtures" not in path.parts
        and "schemas" not in path.parts
        and (
            "docs/codex-runs" not in path.as_posix()
            or (current_run and f"docs/codex-runs/{current_run}/" in path.as_posix())
        )
        and ("evidence" in path.name or "retrieval" in path.name or "desktop_smoke" in path.parts)
    ]
    return max(candidates, key=lambda p: p.stat().st_mtime, default=None)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("payload", nargs="?", help="JSON payload or receipt to audit")
    parser.add_argument("--receipt", help="JSON payload or receipt to audit")
    parser.add_argument("--repo", default=".")
    parser.add_argument("--latest", action="store_true", help="audit the latest matching retrieval/evidence receipt")
    args = parser.parse_args()
    repo = pathlib.Path(args.repo).resolve()
    target = pathlib.Path(args.receipt or args.payload).resolve() if (args.receipt or args.payload) else None
    if args.latest:
        target = latest_receipt(repo)
    if target is None:
        print(json.dumps({"checked": 0, "errors": [], "decision": "no_receipts", "warnings": ["no retrieval disclosure receipt found"]}, indent=2))
        return 0
    if not target.exists():
        print(json.dumps({"checked": 0, "errors": [f"missing receipt: {target}"], "decision": "blocked"}, indent=2))
        return 1
    payload = json.loads(target.read_text())
    result = audit_payload(payload)
    result["receipt"] = str(target)
    print(json.dumps(result, indent=2))
    return 0 if not result["errors"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
