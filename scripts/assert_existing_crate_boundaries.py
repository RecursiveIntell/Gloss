#!/usr/bin/env python3
"""Heuristic guard against reinventing existing library-stack owner concepts."""
import json
import re
import sys
from pathlib import Path

ROOT = Path.cwd()
LIB_ROOTS = [Path("/home/sikmindz/Coding/Libraries")]
KNOWN_OWNERS = {
    "stack-ids": ["id", "digest", "trace", "replay"],
    "contract-schema-gen": ["schema"],
    "verification-policy": ["policy", "permit"],
    "verification-control": ["control receipt", "review", "adjudication"],
    "authority-delegation": ["authority", "delegation"],
    "effect-runtime": ["effect", "action lifecycle"],
    "attestation-exchange": ["attestation", "provenance", "trust"],
    "semantic-memory-forge": ["evidence", "export"],
    "knowledge-runtime": ["runtime query provenance", "temporal view"],
    "llm-tool-runtime": ["tool receipt", "dispatch"],
}
DUPLICATE_TYPE_PATTERNS = [
    r"\b(struct|enum)\s+ContentDigest\b",
    r"\b(struct|enum)\s+ArtifactId\b",
    r"\b(struct|enum)\s+ReceiptId\b",
    r"\b(struct|enum)\s+PolicyId\b",
    r"\b(struct|enum)\s+PermitId\b",
    r"\b(struct|enum)\s+ExecutionContextEnvelope\b",
    r"\b(struct|enum)\s+ControlReceipt\b",
    r"\b(struct|enum)\s+AttestationEnvelope\b",
]


def package_name(cargo_toml: Path):
    text = cargo_toml.read_text(encoding="utf-8", errors="replace")
    in_pkg = False
    for line in text.splitlines():
        stripped = line.strip()
        if stripped == "[package]":
            in_pkg = True
            continue
        if stripped.startswith("[") and stripped != "[package]":
            in_pkg = False
        if in_pkg and stripped.startswith("name"):
            m = re.search(r'name\s*=\s*"([^"]+)"', stripped)
            if m:
                return m.group(1)
    return None


def main() -> int:
    discovered = {}
    for root in LIB_ROOTS:
        if not root.exists():
            continue
        for cargo in root.rglob("Cargo.toml"):
            name = package_name(cargo)
            if name in KNOWN_OWNERS:
                discovered[name] = str(cargo.parent)
    out = ROOT / "docs" / "EXTERNAL_CRATE_BOUNDARY_SCAN.json"
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text(json.dumps({"discovered": discovered, "known_owners": KNOWN_OWNERS}, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    errors = []
    boundary_map = ROOT / "docs" / "EXTERNAL_CRATE_BOUNDARY_MAP.md"
    if not boundary_map.exists():
        errors.append("docs/EXTERNAL_CRATE_BOUNDARY_MAP.md missing")
    else:
        text = boundary_map.read_text(encoding="utf-8", errors="replace")
        for name in discovered:
            if name not in text:
                errors.append(f"boundary map does not mention discovered owner crate: {name} at {discovered[name]}")
    for path in (ROOT / "crates").rglob("*.rs") if (ROOT / "crates").exists() else []:
        text = path.read_text(encoding="utf-8", errors="replace")
        for pattern in DUPLICATE_TYPE_PATTERNS:
            if re.search(pattern, text):
                errors.append(f"possible duplicate owner-owned type in {path.relative_to(ROOT)}: {pattern}")
    if errors:
        print("existing crate boundary violations:", file=sys.stderr)
        for err in errors[:300]:
            print(f"  {err}", file=sys.stderr)
        print(f"scan written: {out}", file=sys.stderr)
        return 1
    print(f"ok existing_crate_boundaries discovered={len(discovered)} scan={out}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
