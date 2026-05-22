#!/usr/bin/env bash
set -euo pipefail

npm run build
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml

python3 scripts/check_gloss_active_validation_scope.py --repo .
python3 scripts/check_feature_flags_static.py --repo .
python3 scripts/chat_runtime_static_audit.py --repo .

echo "all Gloss active checks passed"
