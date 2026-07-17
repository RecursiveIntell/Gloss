#!/usr/bin/env python3
import argparse
import json
from pathlib import Path

def text(path: Path) -> str:
    return path.read_text(errors="ignore") if path.exists() else ""

def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", default=".")
    repo = Path(parser.parse_args().repo)
    failures = []
    ui = "\n".join(text(p) for p in (repo / "src").rglob("*.tsx"))
    for token in ["Notes", "Prompt", "Evidence", "Receipt", "Sources"]:
        if token not in ui:
            failures.append(f"Inspector Dock required tab/surface missing: {token}")
    if "InspectorDock" not in ui and "Inspector Dock" not in ui:
        failures.append("Inspector Dock shell not found")
    print(json.dumps({"ok": not failures, "failures": failures, "warnings": []}, indent=2))
    return 0 if not failures else 1

if __name__ == "__main__":
    raise SystemExit(main())
