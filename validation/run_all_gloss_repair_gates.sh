#!/usr/bin/env bash
set -euo pipefail
ROOT="${1:-.}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
python3 "$SCRIPT_DIR/verify_tauri_contract.py" "$ROOT"
python3 "$SCRIPT_DIR/gloss_rust_source_integrity_gate.py" "$ROOT"
python3 "$SCRIPT_DIR/gloss_runtime_static_gate.py" "$ROOT"
python3 "$SCRIPT_DIR/gloss_provider_cancellation_static_gate.py" "$ROOT"
python3 "$SCRIPT_DIR/validate_chat_terminal_contract.py" "$ROOT"
python3 "$SCRIPT_DIR/gloss_semantic_memory_contract_gate.py" "$ROOT"
python3 "$SCRIPT_DIR/gloss_settings_contract_gate.py" "$ROOT"
python3 "$SCRIPT_DIR/gloss_receipt_consistency_gate.py" "$ROOT"
echo "run_all_gloss_repair_gates: PASS"
