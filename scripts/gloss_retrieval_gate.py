#!/usr/bin/env python3
"""Static retrieval truth gate for GLOSS_RETRIEVAL_TRUTH_AND_HYBRID_REPAIR_20260519."""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

RUN_ID = "GLOSS_RETRIEVAL_TRUTH_AND_HYBRID_REPAIR_20260519"

REQUIRED_REASON_CODES = [
    "native_indexing_disabled",
    "dense_engine_unavailable",
    "embedder_unavailable",
    "index_missing",
    "no_embedded_chunks",
    "partial_embedding_coverage",
    "scope_has_missing_embeddings",
    "semantic_memory_feature_disabled",
    "semantic_memory_build_feature_missing",
    "semantic_memory_links_missing",
    "semantic_memory_links_degraded",
    "semantic_memory_timeout",
    "bm25_query_sanitized_empty",
    "bm25_no_matches",
    "source_order_fallback",
    "raw_content_fallback",
    "no_retrieval_context",
]


def read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""


def finding(findings: list[dict[str, str]], code: str, detail: str, severity: str = "error") -> None:
    findings.append({"severity": severity, "code": code, "detail": detail})


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    findings: list[dict[str, str]] = []

    memory_types = read(repo / "src-tauri/src/memory/types.rs")
    hybrid = read(repo / "src-tauri/src/retrieval/hybrid_search.rs")
    state = read(repo / "src-tauri/src/state.rs")
    chat = read(repo / "src-tauri/src/commands/chat/mod.rs")
    sources = read(repo / "src-tauri/src/commands/sources/mod.rs")
    features = read(repo / "src-tauri/src/features.rs")
    settings_dialog = read(repo / "src/components/settings/SettingsDialog/index.tsx")
    chat_panel = read(repo / "src/components/chat/ChatPanel.tsx")
    frontend_types = read(repo / "src/lib/types.ts")
    tauri_api = read(repo / "src/lib/tauri.ts")

    for symbol in [
        "pub enum RetrievalMode",
        "pub enum RetrievalReasonCode",
        "pub struct RetrievalEngineStatus",
        "pub struct RetrievalCoverage",
        "pub struct RetrievalOutcome",
    ]:
        if symbol not in memory_types:
            finding(findings, "missing-retrieval-outcome-symbol", symbol)

    if "pub enum RetrievalReasonCode" not in memory_types:
        finding(findings, "generic-fallback-reasons-only", "RetrievalReasonCode enum is absent")
    for reason in REQUIRED_REASON_CODES:
        if reason not in memory_types:
            finding(findings, "missing-retrieval-reason-code", reason)

    for mode in [
        "bm25_only",
        "dense_only",
        "hybrid_rrf",
        "semantic_memory",
        "source_order_fallback",
        "raw_content_fallback",
        "unavailable",
    ]:
        if mode not in memory_types:
            finding(findings, "missing-retrieval-mode", mode)

    if "local_retrieval_outcome" not in hybrid or "rrf_fuse" not in hybrid:
        finding(findings, "missing-local-retrieval-coordinator", "hybrid_search.rs lacks local_retrieval_outcome or RRF")
    if "coverage.embedded_chunks == 0" not in hybrid or "coverage.missing_embeddings > 0" not in hybrid:
        finding(findings, "partial-coverage-not-modeled", "partial and zero embedding coverage branches not found")
    if "RetrievalReasonCode::PartialEmbeddingCoverage" not in hybrid:
        finding(findings, "partial-coverage-reason-absent", "partial_embedding_coverage is not emitted")
    if "can_run_hybrid_search" in chat or "try_hybrid_search" in chat:
        finding(findings, "chat-still-uses-all-or-nothing-hybrid", "chat.rs references stale all-or-nothing hybrid gate")
    if "can_run_hybrid_search" in state or "try_hybrid_search" in state:
        finding(findings, "state-still-uses-all-or-nothing-hybrid", "state.rs references stale all-or-nothing hybrid gate")

    if "semantic_memory_preview_active" not in features or "require_semantic_memory_preview_enabled" not in features:
        finding(findings, "missing-semantic-memory-feature-gate", "feature registry lacks semantic-memory active/require helpers")
    if "semantic_memory_preview_active" not in chat:
        finding(findings, "chat-lacks-semantic-memory-gate", "chat.rs does not check active semantic-memory gate")
    chat_has_link_reasons = (
        ("semantic_memory_links_missing" in chat and "semantic_memory_links_degraded" in chat)
        or (
            "RetrievalReasonCode::SemanticMemoryLinksMissing" in chat
            and "RetrievalReasonCode::SemanticMemoryLinksDegraded" in chat
        )
    )
    if not chat_has_link_reasons:
        finding(findings, "semantic-link-health-not-surfaced", "chat.rs lacks link-health reason codes")
    if "SemanticMemoryLinkStatus" not in sources or "reason_codes" not in sources:
        finding(findings, "semantic-link-health-command-incomplete", "sources.rs does not expose link health reasons")

    if "retrieval_outcome" not in chat or "retrieval_trace_ref" not in chat:
        finding(findings, "chat-trace-lacks-retrieval-outcome", "chat trace/evidence lacks retrieval outcome/ref")
    if "ChatEvidenceDisclosure" in chat and "retrieval_outcome" not in chat:
        finding(findings, "evidence-lacks-retrieval-outcome", "ChatEvidenceDisclosure lacks retrieval outcome")
    if "RetrievalOutcome" not in frontend_types or "RetrievalCoverage" not in frontend_types:
        finding(findings, "frontend-retrieval-types-absent", "src/lib/types.ts lacks backend-owned retrieval type mirror")
    if "diagnose_retrieval_coverage" not in sources or "diagnoseRetrievalCoverage" not in tauri_api:
        finding(findings, "coverage-diagnostic-command-absent", "coverage command/API wrapper missing")
    if "Copy retrieval diagnostics JSON" not in chat_panel or "fallback_chain" not in chat_panel:
        finding(findings, "ui-cannot-show-retrieval-reason", "ChatPanel evidence drawer lacks retrieval diagnostics JSON/reasons")

    semantic_option_match = re.search(r'<option[^>]+value="semantic-memory-preview"[^>]*>', settings_dialog)
    if semantic_option_match and "disabled={!semanticPreviewSelectable}" not in semantic_option_match.group(0):
        finding(findings, "ungated-semantic-memory-selection", "SettingsDialog contains raw semantic-memory-preview option")
    if (
        "feature_semantic_memory_preview_enabled" not in settings_dialog
        and "FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED" not in settings_dialog
    ):
        finding(findings, "settings-does-not-render-semantic-feature", "SettingsDialog does not reference semantic-memory feature flag")

    run_dir = repo / "docs/codex-runs" / RUN_ID
    required_artifacts = [
        "STARTUP_PREFLIGHT.md",
        "CURRENT_RETRIEVAL_STATE.md",
        "RETRIEVAL_SOURCE_OF_TRUTH_MAP.md",
        "RETRIEVAL_OUTCOME_SCHEMA.md",
        "PHASE_REPORTS.md",
        "COMMANDS_RUN.md",
        "CHANGED_FILES.txt",
        "VALIDATION_RESULTS.md",
        "RETRIEVAL_SMOKE_RESULTS.md",
        "FINAL_RECEIPT.json",
        "FINAL_AUDITOR_HANDOFF.md",
        "ROLLBACK_PLAN.md",
    ]
    for artifact in required_artifacts:
        if not (run_dir / artifact).exists():
            finding(findings, "missing-final-artifact", artifact)

    receipt_path = run_dir / "FINAL_RECEIPT.json"
    if receipt_path.exists():
        try:
            receipt = json.loads(read(receipt_path))
        except json.JSONDecodeError as exc:
            finding(findings, "invalid-final-receipt-json", str(exc))
        else:
            validation = receipt.get("validation", {})
            for key, value in validation.items():
                if value == "skipped":
                    reason = receipt.get("skipped_reasons", {}).get(key)
                    if not reason:
                        finding(findings, "skipped-validation-without-reason", key)
            if receipt.get("hybrid_truth") not in {"active", "partially_active", "unavailable"}:
                finding(findings, "receipt-missing-hybrid-truth", "hybrid_truth must be active/partially_active/unavailable")
            if not receipt.get("retrieval_outcome_schema_present"):
                finding(findings, "receipt-contradicts-schema", "receipt does not affirm retrieval outcome schema")
            if not receipt.get("chat_trace_has_retrieval_outcome"):
                finding(findings, "receipt-contradicts-chat-trace", "receipt does not affirm chat trace retrieval evidence")

    errors = [item for item in findings if item["severity"] == "error"]
    result = {
        "ok": not errors,
        "run_id": RUN_ID,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
