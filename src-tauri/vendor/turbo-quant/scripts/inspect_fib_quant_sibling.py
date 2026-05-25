#!/usr/bin/env python3
from __future__ import annotations
import json, os, sys
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()

candidates = []
if os.environ.get("FIB_QUANT_PATH"):
    candidates.append(Path(os.environ["FIB_QUANT_PATH"]).expanduser())
for rel in ["../fib-quant", "../fib_quant", "../../fib-quant", "../../Libraries/fib-quant"]:
    candidates.append((ROOT / rel).resolve())
for abs_path in [
    "~/Coding/Libraries/fib-quant",
    "~/Coding/fib-quant",
    "~/Documents/fib-quant",
]:
    candidates.append(Path(abs_path).expanduser())

seen = set()
found = []
for c in candidates:
    if str(c) in seen:
        continue
    seen.add(str(c))
    cargo = c / "Cargo.toml"
    if cargo.exists():
        info = {"path": str(c), "cargo_toml": str(cargo)}
        readme = c / "README.md"
        if readme.exists():
            info["readme"] = str(readme)
        src_lib = c / "src" / "lib.rs"
        if src_lib.exists():
            info["src_lib"] = str(src_lib)
        found.append(info)

summary = {
    "root": str(ROOT),
    "found_fib_quant": found,
    "rule": "read-only inspection only; do not modify sibling fib-quant in this pass unless explicitly authorized"
}
print(json.dumps(summary, indent=2))
