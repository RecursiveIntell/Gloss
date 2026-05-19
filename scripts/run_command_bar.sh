#!/usr/bin/env bash
set -uo pipefail
ROOT="${1:-$(pwd)}"
RUN_ID="GLOSS_COMPLETION_AND_UX_RELEASE_CANDIDATE_P3_20260513"
OUT="$ROOT/docs/codex-runs/$RUN_ID"
LOG_DIR="$OUT/logs"
mkdir -p "$LOG_DIR" "$OUT/receipts" "$OUT/reports"
STATUS="$LOG_DIR/commands.status.tsv"
: > "$STATUS"
printf 'name\tcwd\tstatus\texit_code\tlog\tsha256\tcommand\n' >> "$STATUS"
sha_file() { if [ -f "$1" ]; then sha256sum "$1" | awk '{print $1}'; else printf ''; fi; }
run_cmd() {
  local name="$1"
  local cwd="$2"
  local cmd="$3"
  local log="$LOG_DIR/${name}.log"
  echo "== $name ==" | tee "$log"
  echo "cwd=$cwd" | tee -a "$log"
  echo "cmd=$cmd" | tee -a "$log"
  (cd "$cwd" && bash -lc "$cmd") >> "$log" 2>&1
  local ec=$?
  local status="pass"; if [ "$ec" -ne 0 ]; then status="fail"; fi
  local hash; hash=$(sha_file "$log")
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "$name" "$cwd" "$status" "$ec" "$log" "$hash" "$cmd" >> "$STATUS"
}
{
  echo "pwd=$(pwd)"; echo "root=$ROOT"; date -u +%Y-%m-%dT%H:%M:%SZ
  node --version 2>/dev/null || true; npm --version 2>/dev/null || true
  rustc --version 2>/dev/null || true; cargo --version 2>/dev/null || true
  df -h /tmp "$ROOT" 2>/dev/null || true
} > "$LOG_DIR/environment.log" 2>&1
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-/tmp/gloss-target}"
gloss="$ROOT"; tauri="$ROOT/src-tauri"; semantic="$ROOT/src-tauri/vendor/semantic-memory"
if [ ! -d "$semantic" ] && [ -d "$ROOT/../Libraries/semantic-memory" ]; then semantic="$ROOT/../Libraries/semantic-memory"; fi
run_cmd npm_ci "$gloss" "npm ci"
run_cmd npm_run_build "$gloss" "npm run build"
run_cmd npm_run_tauri_build "$gloss" "npm run tauri build"
run_cmd cargo_fmt_check "$tauri" "cargo fmt --all -- --check"
run_cmd cargo_test_default "$tauri" "cargo test --locked"
run_cmd cargo_test_semantic_memory_backend "$tauri" "cargo test --locked --features semantic-memory-backend"
run_cmd cargo_test_semantic_memory_turbo_quant "$tauri" "cargo test --locked --features semantic-memory-turbo-quant"
run_cmd cargo_clippy_default "$tauri" "cargo clippy --locked --all-targets -- -D warnings"
run_cmd cargo_clippy_semantic_memory_backend "$tauri" "cargo clippy --locked --all-targets --features semantic-memory-backend -- -D warnings"
run_cmd cargo_clippy_semantic_memory_turbo_quant "$tauri" "cargo clippy --locked --all-targets --features semantic-memory-turbo-quant -- -D warnings"
if [ -d "$semantic" ]; then run_cmd semantic_memory_cargo_test "$semantic" "cargo test --locked"; else
  log="$LOG_DIR/semantic_memory_missing.log"; echo "semantic-memory root not found" > "$log"
  printf '%s\t%s\t%s\t%s\t%s\t%s\t%s\n' "semantic_memory_cargo_test" "$semantic" "blocked" "" "$log" "$(sha_file "$log")" "cargo test --locked" >> "$STATUS"
fi
python3 - <<'PYIN' "$STATUS" "$OUT/receipts/PHASE_01_ENVIRONMENT_AND_COMMAND_BAR_UNBLOCK.json"
import csv, json, sys, pathlib
status_path=pathlib.Path(sys.argv[1]); out=pathlib.Path(sys.argv[2])
rows=list(csv.DictReader(status_path.open(), delimiter='\t'))
decision='pass' if all(r['status']=='pass' for r in rows) else 'blocked'
out.write_text(json.dumps({'run_id':'GLOSS_COMPLETION_AND_UX_RELEASE_CANDIDATE_P3_20260513','phase':'PHASE_01_ENVIRONMENT_AND_COMMAND_BAR_UNBLOCK','created_utc':None,'source_package_sha256':'56a037332351491ba103b832a543fb0dc36d9ea575b50f4ee11f76302a1d2560','content_manifest_sha256':'65a131ee403bf49d1d35deb019d718a9c9183901f88f561828b58fc94868c27b','files_changed':[],'commands_run':[{'cmd':r['command'],'cwd':r['cwd'],'status':r['status'],'log_path':r['log'],'log_sha256':r['sha256'],'exit_code': int(r['exit_code']) if r['exit_code'].isdigit() else None} for r in rows],'tests_added':[],'tests_passed':[r['name'] for r in rows if r['status']=='pass'],'tests_failed':[r['name'] for r in rows if r['status']!='pass'],'issues_closed':[],'issues_deferred':[],'residual_risk':[],'decision':decision}, indent=2)+'\n')
PYIN
echo "Command status: $STATUS"
