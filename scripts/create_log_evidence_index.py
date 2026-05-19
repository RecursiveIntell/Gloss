#!/usr/bin/env python3
import hashlib, json, sys, pathlib, datetime
run_dir = pathlib.Path(sys.argv[1])
log_dir = run_dir / "logs"
logs = []
for path in sorted(log_dir.rglob("*")):
    if path.is_file():
        data = path.read_bytes()
        logs.append({
            "path": str(path.relative_to(run_dir)),
            "sha256": hashlib.sha256(data).hexdigest(),
            "bytes": len(data),
            "phase": path.name.split('_')[0] if '_' in path.name else None
        })
index = {
    "run_id": "GLOSS_SEMANTIC_MEMORY_P2_PARITY_AND_UX_PROMOTION_20260513",
    "created_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "logs": logs
}
(run_dir / "LOG_EVIDENCE_INDEX.json").write_text(json.dumps(index, indent=2))
md = ["# Log Evidence Index", "", f"Created UTC: `{index['created_utc']}`", "", "| Path | Bytes | SHA-256 |", "|---|---:|---|"]
for log in logs:
    md.append(f"| `{log['path']}` | {log['bytes']} | `{log['sha256']}` |")
(run_dir / "LOG_EVIDENCE_INDEX.md").write_text("\n".join(md)+"\n")
print(f"indexed {len(logs)} logs")
