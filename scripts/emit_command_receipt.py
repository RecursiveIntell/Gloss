#!/usr/bin/env python3
import csv, json, sys, datetime, pathlib
run_dir = pathlib.Path(sys.argv[1])
source_sha = sys.argv[2]
status_tsv = run_dir / "logs" / "commands.status.tsv"
rows = []
if status_tsv.exists():
    with status_tsv.open() as f:
        for parts in csv.reader(f, delimiter='\t'):
            if len(parts) >= 5:
                rows.append({"name": parts[0], "status": parts[1], "exit_code": parts[2], "log_path": parts[3], "log_sha256": parts[4]})
receipt = {
    "run_id": "GLOSS_SEMANTIC_MEMORY_P2_PARITY_AND_UX_PROMOTION_20260513",
    "phase": "PHASE_08_COMMAND_BAR",
    "created_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "source_package_sha256": source_sha,
    "files_changed": [],
    "commands_run": [{"cmd": r["name"], "cwd": "see log", "status": r["status"], "log_path": r["log_path"], "log_sha256": r["log_sha256"]} for r in rows],
    "tests_added": [],
    "tests_passed": [r["name"] for r in rows if r["status"] == "pass"],
    "tests_failed": [r["name"] for r in rows if r["status"] == "fail"],
    "issues_closed": ["P2-011"] if rows and all(r["status"] == "pass" for r in rows) else [],
    "issues_deferred": [],
    "residual_risk": [] if rows and all(r["status"] == "pass" for r in rows) else ["one or more command-bar entries did not pass"],
    "decision": "pass" if rows and all(r["status"] == "pass" for r in rows) else "blocked"
}
out = run_dir / "receipts" / "PHASE_08_COMMAND_BAR.json"
out.write_text(json.dumps(receipt, indent=2))
