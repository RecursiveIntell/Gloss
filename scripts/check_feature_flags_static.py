#!/usr/bin/env python3
"""Static gate for Gloss feature flag governance."""
from __future__ import annotations
import argparse, json, re, sys
from pathlib import Path

REQUIRED_KEYS = [
    "experimental_features_enabled",
    "feature_semantic_memory_preview_enabled",
    "feature_semantic_memory_turbo_quant_enabled",
    "feature_chat_diagnostics_enabled",
    "feature_provider_smoke_tools_enabled",
    "feature_advanced_retrieval_controls_enabled",
    "feature_index_replay_tools_enabled",
    "feature_package_release_panel_enabled",
    "feature_vision_jobs_enabled",
    "feature_video_import_enabled",
    "feature_background_summaries_enabled",
    "feature_external_tools_enabled",
    "feature_local_rag_enabled",
    "feature_source_scope_enabled",
]

def read(path: Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except FileNotFoundError:
        return ""

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=".")
    args = ap.parse_args()
    repo = Path(args.repo).resolve()
    findings = []

    rust_blob = "\n".join(read(p) for p in [
        repo / "src-tauri/src/features.rs",
        repo / "src-tauri/src/commands/settings.rs",
        repo / "src-tauri/src/state.rs",
        repo / "src-tauri/src/lib.rs",
    ])
    frontend_blob = "\n".join(read(p) for p in [
        repo / "src/lib/types.ts",
        repo / "src/lib/tauri.ts",
        repo / "src/lib/features.ts",
        repo / "src/stores/settingsStore.ts",
        repo / "src/components/settings/SettingsDialog.tsx",
    ])

    for key in REQUIRED_KEYS:
        if key not in rust_blob and key not in frontend_blob:
            findings.append({"severity":"error","code":"missing-feature-key","detail":key})

    if "get_feature_flags" not in rust_blob:
        findings.append({"severity":"error","code":"missing-backend-command","detail":"get_feature_flags not found in backend"})
    if "update_feature_flag" not in rust_blob:
        findings.append({"severity":"error","code":"missing-backend-command","detail":"update_feature_flag not found in backend"})
    if "cfg!(feature = \"semantic-memory-backend\")" not in rust_blob:
        findings.append({"severity":"error","code":"missing-build-feature-gate","detail":"semantic-memory-backend cfg gate not found"})
    if "cfg!(feature = \"semantic-memory-turbo-quant\")" not in rust_blob:
        findings.append({"severity":"error","code":"missing-build-feature-gate","detail":"semantic-memory-turbo-quant cfg gate not found"})

    settings_dialog = read(repo / "src/components/settings/SettingsDialog.tsx")
    # Fail only the old exact unconditional option pattern; Codex may replace it with a mapped/gated option.
    old_option = '<option value="semantic-memory-preview">semantic-memory preview</option>'
    if old_option in settings_dialog:
        findings.append({"severity":"error","code":"ungated-semantic-memory-option","detail":"SettingsDialog still contains old unconditional semantic-memory-preview option"})

    if "FeatureFlagStatus" not in frontend_blob:
        findings.append({"severity":"error","code":"missing-frontend-type","detail":"FeatureFlagStatus not found"})

    errors = [f for f in findings if f["severity"] == "error"]
    result = {"ok": not errors, "error_count": len(errors), "finding_count": len(findings), "findings": findings}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1

if __name__ == "__main__":
    raise SystemExit(main())
