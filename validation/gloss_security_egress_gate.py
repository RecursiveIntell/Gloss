#!/usr/bin/env python3
import argparse
import json
import re
from pathlib import Path


def text(path: Path) -> str:
    return path.read_text(errors="ignore") if path.exists() else ""


def current_run(repo: Path) -> str | None:
    match = re.search(
        r"Current run:\s*`?([^`\n]+)`?",
        text(repo / "docs/codex-runs/CURRENT_RUN.md"),
    )
    return match.group(1).strip() if match else None


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    args = parser.parse_args()
    repo = Path(args.repo).resolve()
    failures: list[str] = []
    warnings: list[str] = []

    providers = text(repo / "src-tauri/src/providers/mod.rs")
    settings = text(repo / "src-tauri/src/commands/settings.rs")
    provider_modules = "\n".join(
        text(repo / "src-tauri/src/providers" / name)
        for name in ["openai.rs", "anthropic.rs", "ollama.rs", "llamacpp.rs"]
    )
    receipt = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "SECURITY_EGRESS_RECEIPT.json"
    )

    required_tokens = [
        "NetworkScopeReceiptV1",
        "validate_provider_base_url",
        "LOCAL_EGRESS_HOSTS",
        "OPENAI_EGRESS_HOST",
        "ANTHROPIC_EGRESS_HOST",
        "sanitize_provider_error_body",
        "provider_http_error",
    ]
    for token in required_tokens:
        if token not in providers:
            failures.append(f"providers/mod.rs missing {token}")
    validation_call = re.search(
        r"validate_provider_base_url\s*\(\s*provider_type\s*,\s*&?candidate_url\s*,\s*allow_lan\s*,\s*allow_custom_cloud_endpoints",
        settings,
        re.DOTALL,
    )
    if validation_call is None:
        failures.append("update_provider does not validate provider base URL before persistence")
    if "HTTP {}: {}" in provider_modules:
        failures.append("provider module still formats raw HTTP error body")
    if "provider_http_error(" not in provider_modules:
        failures.append("provider modules do not use sanitized provider_http_error helper")
    if not receipt.exists():
        failures.append(f"missing security egress receipt: {receipt.relative_to(repo)}")
    else:
        try:
            data = json.loads(receipt.read_text())
        except Exception as exc:
            failures.append(f"invalid security egress receipt JSON: {exc}")
            data = {}
        if data.get("schema") != "SecurityEgressReceiptV1":
            failures.append("security egress receipt schema mismatch")
        if not data.get("network_scope_policy_active"):
            failures.append("security egress receipt does not mark network scope policy active")
        if not data.get("provider_error_redaction_active"):
            failures.append("security egress receipt does not mark provider redaction active")

    print(json.dumps({"ok": not failures, "failures": failures, "warnings": warnings}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
