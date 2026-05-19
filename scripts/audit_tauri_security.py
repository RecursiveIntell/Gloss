#!/usr/bin/env python3
import json
import pathlib
import sys


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    conf = json.loads((root / "src-tauri/tauri.conf.json").read_text())
    cap = json.loads((root / "src-tauri/capabilities/default.json").read_text())
    chat = (root / "src/components/chat/ChatPanel.tsx").read_text()
    security = (root / "SECURITY_AND_PRIVACY.md").read_text()
    errors = []
    warnings = []

    csp = conf.get("app", {}).get("security", {}).get("csp")
    if not isinstance(csp, str) or not csp.strip():
        errors.append("CSP must be non-null")
    else:
        for directive in ["default-src 'self'", "script-src 'self'", "object-src 'none'", "frame-ancestors 'none'"]:
            if directive not in csp:
                errors.append(f"CSP missing directive: {directive}")

    perms = set(cap.get("permissions", []))
    broad_defaults = [
        "core:default",
        "core:event:default",
        "dialog:default",
        "opener:default",
        "fs:default",
        "shell:default",
        "clipboard-manager:default",
    ]
    for permission in broad_defaults:
        if permission in perms:
            errors.append(f"{permission} must not be granted")

    unused_permissions = [
        "core:event:allow-emit",
        "core:event:allow-emit-to",
        "dialog:allow-save",
        "fs:allow-read-text-file",
        "fs:allow-write-text-file",
        "clipboard-manager:allow-read-text",
        "clipboard-manager:allow-write-text",
    ]
    for permission in unused_permissions:
        if permission in perms:
            errors.append(f"{permission} is not justified by current frontend usage")

    required_permissions = {"core:event:allow-listen", "core:event:allow-unlisten", "dialog:allow-open"}
    missing = sorted(required_permissions - perms)
    for permission in missing:
        errors.append(f"missing required least-privilege permission: {permission}")

    if "least-privilege permissions" not in security:
        errors.append("SECURITY_AND_PRIVACY must document the explicit capability permissions")

    if "dangerouslySetInnerHTML" in chat or "innerHTML" in chat:
        errors.append("ChatPanel must not render model output through innerHTML")
    if "ReactMarkdown" not in chat:
        errors.append("ChatPanel markdown rendering path not found")
    if "does not use `dangerouslySetInnerHTML`" not in security:
        errors.append("SECURITY_AND_PRIVACY must document model-output rendering safety")

    result = {
        "permissions": sorted(perms),
        "errors": errors,
        "warnings": warnings,
        "status": "pass" if not errors else "fail",
    }
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
