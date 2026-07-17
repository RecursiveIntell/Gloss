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
    store = text(repo / "src-tauri/src/provider_config_store.rs")
    receipt = (
        repo
        / "docs/codex-runs"
        / (current_run(repo) or "__missing__")
        / "SECRET_STORE_PERMISSION_RECEIPT.json"
    )

    checks = {
        "owner-only dir permissions": "set_owner_only_dir_permissions" in store and "0o700" in store,
        "owner-only file permissions": "set_owner_only_permissions" in store and "0o600" in store,
        "tmp/data owner-only writer": "write_owner_only_file" in store and "open_owner_only_truncate" in store,
        "key create mode": "open_owner_only_create_new" in store,
        "existing key repair": "set_owner_only_permissions(&key_path)" in store,
        "existing data repair": "set_owner_only_permissions(&data_path)" in store,
        "permission test": "secret_store_repairs_owner_only_permissions" in store,
    }
    for name, ok in checks.items():
        if not ok:
            failures.append(f"missing {name}")

    if not receipt.exists():
        failures.append(f"missing secret-store permission receipt: {receipt.relative_to(repo)}")
    else:
        try:
            data = json.loads(receipt.read_text())
        except Exception as exc:
            failures.append(f"invalid secret-store permission receipt JSON: {exc}")
            data = {}
        if data.get("schema") != "SecretStorePermissionReceiptV1":
            failures.append("secret-store permission receipt schema mismatch")
        if not data.get("unix_owner_only_permissions_active"):
            failures.append("secret-store receipt does not mark Unix owner-only permissions active")

    print(json.dumps({"ok": not failures, "failures": failures}, indent=2))
    return 0 if not failures else 1


if __name__ == "__main__":
    raise SystemExit(main())
