#!/usr/bin/env python3
import json
import pathlib
import sys


def main() -> int:
    root = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else ".")
    state = (root / "src-tauri/src/state.rs").read_text()
    sources = (root / "src-tauri/src/commands/sources.rs").read_text()
    executor = (root / "src-tauri/vendor/job-queue/src/executor.rs").read_text()
    errors = []
    warnings = []

    if "pub struct ActiveCounterGuard" not in state or "impl Drop for ActiveCounterGuard" not in state:
        errors.append("ActiveCounterGuard with Drop finalizer is missing")
    if "current.checked_add(1)" not in state:
        errors.append("ActiveCounterGuard must use checked increment")
    if "current.checked_sub(1)" not in state:
        errors.append("ActiveCounterGuard must use checked decrement")
    if "active: bool" not in state:
        errors.append("ActiveCounterGuard must track whether increment succeeded")
    if sources.count("ActiveCounterGuard::new(&state.ingestion_active") < 2:
        errors.append("run and batch ingestion paths must use ActiveCounterGuard")
    if ".ingestion_active.fetch_sub" in sources:
        errors.append("manual ingestion_active fetch_sub remains in sources.rs")
    if "active_counter_guard_finalizes_on_drop_and_saturates" not in state:
        errors.append("ActiveCounterGuard finalizer test is missing")
    if "u32::MAX" not in state:
        errors.append("ActiveCounterGuard overflow/saturation regression test is missing")

    if "struct HeartbeatStopGuard" not in executor or "impl Drop for HeartbeatStopGuard" not in executor:
        errors.append("HeartbeatStopGuard with Drop finalizer is missing")
    if "let _heartbeat_guard = HeartbeatStopGuard::new(&heartbeat_stop)" not in executor:
        errors.append("Queue executor heartbeat task is not guarded by HeartbeatStopGuard")
    if "heartbeat_stop.store(true, Ordering::Relaxed);" in executor:
        warnings.append("explicit heartbeat stop store remains; verify RAII guard is still authoritative")
    if "heartbeat_stop_guard_finalizes_on_drop" not in executor:
        errors.append("HeartbeatStopGuard finalizer test is missing")
    if "db::reclaim_stale(conn, stale_after_secs)" not in executor:
        errors.append("Foreground process_one must reclaim stale jobs before claiming")
    if "foreground_process_one_reclaims_stale_processing_job" not in executor:
        errors.append("Foreground stale recovery behavioral test is missing")

    result = {"errors": errors, "warnings": warnings, "status": "pass" if not errors else "fail"}
    print(json.dumps(result, indent=2))
    return 0 if not errors else 1


if __name__ == "__main__":
    raise SystemExit(main())
