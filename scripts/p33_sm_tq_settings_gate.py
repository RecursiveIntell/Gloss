#!/usr/bin/env python3
from __future__ import annotations

import argparse
import json
from pathlib import Path


def read(path: Path) -> str:
    return path.read_text(encoding="utf-8", errors="replace") if path.exists() else ""


def require(findings: list[dict[str, str]], ok: bool, code: str, path: str, detail: str) -> None:
    if not ok:
        findings.append({"severity": "error", "code": code, "path": path, "detail": detail})


def main() -> int:
    parser = argparse.ArgumentParser(description="P33 semantic-memory/TurboQuant settings gate.")
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    findings: list[dict[str, str]] = []

    features = read(repo / "src-tauri/src/features.rs")
    adapter = read(repo / "src-tauri/src/memory/semantic_memory_adapter.rs")
    settings_rs = read(repo / "src-tauri/src/commands/settings.rs")
    settings_ui = read(repo / "src/components/settings/SettingsDialog.tsx")
    types = read(repo / "src/lib/types.ts")
    tauri = read(repo / "src/lib/tauri.ts")
    sources = read(repo / "src-tauri/src/commands/sources.rs")

    require(
        findings,
        "FEATURE_SEMANTIC_MEMORY_PREVIEW_ENABLED" in features
        and "default_enabled: false" in features,
        "semantic-memory-preview-not-default-off",
        "src-tauri/src/features.rs",
        "semantic-memory preview must be present and default off",
    )
    require(
        findings,
        "FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED" in features
        and "turbo_quant_active" in features,
        "turboquant-runtime-gate-missing",
        "src-tauri/src/features.rs",
        "TurboQuant must be governed by runtime setting, not build feature alone",
    )
    require(
        findings,
        "require_semantic_memory_preview_enabled" in features
        and "validate_setting_update" in features,
        "memory-backend-setting-not-guarded",
        "src-tauri/src/features.rs",
        "semantic-memory backend selection must require runtime consent",
    )
    require(
        findings,
        "SemanticMemoryRuntimeConfig" in adapter
        and "turbo_quant_enabled: false" in adapter
        and "runtime_config_from_settings" in adapter,
        "semantic-memory-runtime-config-missing",
        "src-tauri/src/memory/semantic_memory_adapter.rs",
        "runtime config must default TurboQuant off and be settings-derived",
    )
    require(
        findings,
        "turbo_quant_require_exact_rerank" in adapter
        and "DerivedVectorBackendPolicy::TurboQuantCandidateOnly" in adapter,
        "turboquant-exact-rerank-policy-missing",
        "src-tauri/src/memory/semantic_memory_adapter.rs",
        "TurboQuant must remain candidate-only with exact rerank required",
    )
    require(
        findings,
        "get_feature_flags" in settings_rs and "update_feature_flag" in settings_rs,
        "settings-feature-commands-missing",
        "src-tauri/src/commands/settings.rs",
        "settings commands must expose feature status and updates",
    )
    require(
        findings,
        "semantic-memory" in settings_ui
        and "memoryBackendStatus" in settings_ui
        and "linkStatus" in settings_ui
        and "FEATURE_SEMANTIC_MEMORY_TURBO_QUANT_ENABLED" in settings_ui,
        "settings-ui-disclosure-missing",
        "src/components/settings/SettingsDialog.tsx",
        "Settings UI must expose semantic-memory status, link health, and TurboQuant controls",
    )
    require(
        findings,
        "FeatureFlagStatus" in types
        and "SemanticMemoryLinkStatus" in types
        and "MemoryBackendStatus" in types,
        "frontend-types-missing",
        "src/lib/types.ts",
        "frontend must mirror backend settings/status types",
    )
    require(
        findings,
        "getFeatureFlags" in tauri
        and "updateFeatureFlag" in tauri
        and "memoryBackendStatus" in tauri,
        "frontend-tauri-api-missing",
        "src/lib/tauri.ts",
        "frontend Tauri wrappers must expose feature and backend status commands",
    )
    require(
        findings,
        "memory_backend_status" in sources and "semantic_memory_link_status" in sources,
        "backend-status-command-missing",
        "src-tauri/src/commands/sources.rs",
        "backend must expose memory backend and semantic link status",
    )

    errors = [finding for finding in findings if finding.get("severity") == "error"]
    result = {
        "ok": not errors,
        "error_count": len(errors),
        "finding_count": len(findings),
        "findings": findings,
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
