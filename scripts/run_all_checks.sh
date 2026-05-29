#!/usr/bin/env bash
set -euo pipefail

npm run build
npm test
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant

python3 scripts/check_gloss_active_validation_scope.py --repo .
python3 scripts/check_feature_flags_static.py --repo .
python3 scripts/chat_runtime_static_audit.py --repo .
python3 validation/gloss_current_run_truth_gate.py --repo .
python3 validation/gloss_stale_pass_surface_gate.py --repo .
python3 validation/gloss_package_scope_gate.py --repo .
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT
python3 validation/gloss_security_egress_gate.py --repo .
python3 validation/gloss_fastembed_download_consent_gate.py --repo .
python3 validation/gloss_secret_store_permissions_gate.py --repo .
python3 validation/gloss_tool_invocation_receipt_gate.py --repo .
python3 validation/gloss_path_redaction_gate.py --repo .
python3 validation/gloss_import_capability_gate.py --repo .
python3 validation/gloss_document_extractors_gate.py --repo .
python3 validation/gloss_legacy_office_extractors_gate.py --repo .
python3 validation/gloss_audio_metadata_gate.py --repo .
python3 validation/gloss_audio_transcription_gate.py --repo .
python3 validation/gloss_url_import_gate.py --repo .
python3 validation/gloss_youtube_transcript_gate.py --repo .
python3 validation/gloss_studio_artifacts_gate.py --repo .
python3 validation/gloss_db_doctor_gate.py --repo .
python3 validation/gloss_failed_import_quarantine_gate.py --repo .
python3 validation/gloss_import_performance_gate.py --repo .
python3 validation/gloss_notebook_portability_gate.py --repo .
npm run desktop-smoke
python3 validation/gloss_desktop_smoke_gate.py --repo .
python3 validation/gloss_fresh_unzip_replay_gate.py --repo .

echo "all Gloss active checks passed"
