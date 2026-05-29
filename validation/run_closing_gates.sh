#!/usr/bin/env bash
set -u
ROOT="${1:-.}"
cd "$ROOT" || exit 2
fail=0
run() {
  echo "\n==> $*"
  "$@" || fail=1
}
run npm run build
run npm test
run python3 validation/validate_source_send_gate.py .
run python3 validation/validate_frontend_event_routing.py .
run python3 validation/validate_chat_terminal_contract.py .
run python3 validation/validate_provider_lan_policy.py .
run python3 validation/validate_release_receipt_consistency.py .
if command -v cargo >/dev/null 2>&1; then
  run cargo fmt --all -- --check
  run cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets
  run cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant commands::chat::tests
  run cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant providers::tests
else
  echo "WARN: cargo unavailable; release remains blocked until Rust checks run locally"
  fail=1
fi
exit $fail
