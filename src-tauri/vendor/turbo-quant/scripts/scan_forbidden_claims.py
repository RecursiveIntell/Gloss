#!/usr/bin/env python3
from __future__ import annotations
import re, sys, json
from pathlib import Path

ROOT = Path(sys.argv[1] if len(sys.argv) > 1 else ".").resolve()
FILES = ["Cargo.toml", "README.md", "src/lib.rs"]
PATTERNS = [
    ("zero_accuracy_loss", re.compile(r"\bzero\s+accuracy\s+loss\b", re.I)),
    ("zero_overhead", re.compile(r"\bzero[-\s]?overhead\b", re.I)),
    ("production_kv_ready", re.compile(r"\bproduction[-\s]+KV[-\s]+cache\b|\bproduction[-\s]+ready\b", re.I)),
    ("no_degradation", re.compile(r"\bno\s+degradation\b|\bno\s+quality\s+loss\b", re.I)),
    ("google_impl", re.compile(r"\bofficial\s+Google\b|\bGoogle\s+implementation\b", re.I)),
]
ALLOW_CONTEXT = re.compile(r"\b(do not|forbidden|avoid|remove|unqualified|paper claims?|not claim|must not)\b", re.I)

findings = []
for rel in FILES:
    p = ROOT / rel
    if not p.exists():
        continue
    for lineno, line in enumerate(p.read_text(encoding="utf-8", errors="replace").splitlines(), start=1):
        for name, pat in PATTERNS:
            if pat.search(line) and not ALLOW_CONTEXT.search(line):
                findings.append({"file": rel, "line": lineno, "pattern": name, "text": line.strip()})

print(json.dumps({"forbidden_claim_findings": findings, "count": len(findings)}, indent=2))
sys.exit(1 if findings else 0)
