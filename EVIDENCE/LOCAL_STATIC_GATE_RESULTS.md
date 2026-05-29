Observed local static gate results from extracted latest package (`Gloss-generic-rust-next-codex-context-20260525T215913Z.zip`):

PASS:
- node scripts/run_frontend_contract_tests.mjs
- python3 scripts/check_gloss_sm_tq_fix.py .
- python3 scripts/gloss_current_run_truth_gate.py --repo .
- python3 scripts/gloss_receipt_integrity_gate.py --repo .
- python3 scripts/gloss_feature_matrix_gate.py --repo .
- python3 scripts/gloss_issue_ledger_gate.py --ledger ISSUE_LEDGER.csv
- python3 scripts/gloss_static_runtime_truth_gate.py --repo .
- python3 scripts/audit_tauri_security.py .
- python3 scripts/audit_ui_disclosure_a11y.py .
- python3 scripts/gloss_evidence_ui_gate.py --repo .
- python3 scripts/gloss_retrieval_gate.py --repo .
- python3 scripts/gloss_validator_path_gate.py --repo .
- python3 validation/gloss_stale_pass_surface_gate.py --repo .
- python3 validation/gloss_embedding_provider_gate.py --repo .

FAIL:
- python3 scripts/check_release_eligibility_current.py --repo .
  - current-run mismatch: script still expects P36 while CURRENT_RUN is GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525
  - FINAL_RECEIPT release_ready false/missing
- python3 validation/gloss_live_receipt_gate.py --repo . --run-id GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525
  - dense indexed_chunks must be > 0
  - dense live_dense_ingestion_exercised must be true
  - semantic live_projection_sources must be > 0
  - semantic projection passed must be true
  - TurboQuant exact_rerank must be true
  - TurboQuant exact_rerank_count must be > 0
  - TurboQuant/vector artifact manifest digest missing
  - live desktop smoke not exercised
  - live desktop smoke not release_grade

UNKNOWN / NOT REPRODUCED HERE:
- cargo fmt/check/test/clippy because this sandbox did not have the Rust/Cargo toolchain.
- tauri release build and installer smoke.
