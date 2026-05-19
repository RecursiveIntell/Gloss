#!/usr/bin/env python3
import json
import pathlib
import sys


def main() -> int:
    path = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else "fixtures/backend_parity_fixture.json")
    data = json.loads(path.read_text())
    sample = data.get("sample_comparison")
    required = data.get("required_report_fields", [])
    errors = []

    if not isinstance(sample, dict):
        errors.append("sample_comparison must be an object")
    else:
        missing = [field for field in required if field not in sample]
        if missing:
            errors.append(f"sample_comparison missing required fields: {missing}")
        if sample.get("semantic_memory_result") is None:
            errors.append("semantic_memory_result must be explicit, not null")
        if sample.get("decision") not in data.get("allowed_decisions", []):
            errors.append("decision is not in allowed_decisions")
        receipt_ids = sample.get("receipt_ids", [])
        if len(receipt_ids) < 2:
            errors.append("receipt_ids must include local and semantic receipts")
        semantic = sample.get("semantic_memory_result") or {}
        if semantic.get("backend_requested") != "semantic-memory-preview":
            errors.append("semantic_memory_result.backend_requested must be semantic-memory-preview")
        local = sample.get("local_backend_result") or {}
        if local.get("backend_requested") != "gloss-local":
            errors.append("local_backend_result.backend_requested must be gloss-local")
        if sample.get("source_scope_violations") and sample.get("decision") != "blocked-source-scope-violation":
            errors.append("source scope violations must produce blocked-source-scope-violation decision")
        if semantic.get("degraded") and not sample.get("unmapped_semantic_candidates"):
            errors.append("degraded semantic result must include unmapped/degradation evidence")

    result = {"fixture": str(path), "errors": errors, "status": "pass" if not errors else "fail"}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
