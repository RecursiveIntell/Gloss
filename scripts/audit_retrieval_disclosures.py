#!/usr/bin/env python3
import json, pathlib, sys

required = [
    "backend_requested", "backend_used", "retrieval_mode", "fallback_used",
    "fallback_reason", "degradation_markers", "source_scope_mode",
    "requested_source_ids", "selected_source_ids", "effective_source_ids", "invalid_source_ids", "excluded_source_ids",
    "invalid_source_count", "effective_source_count", "excluded_source_count",
    "context_passage_count", "citation_valid_count", "citation_invalid_count",
    "omitted_candidate_count", "source_scope_preserved", "index_status",
    "link_status", "receipt_id",
]

payload = json.loads(pathlib.Path(sys.argv[1]).read_text())
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
print(json.dumps({"checked": len(items), "errors": errors, "decision": "pass" if not errors else "blocked"}, indent=2))
sys.exit(0 if not errors else 1)
