# Zip Source Certifier Report

## Summary

- Script version: `2026.05.22-p31`
- Created UTC: `2026-05-29T00:48:08Z`
- Root: `/home/sikmindz/Coding/Gloss`
- Archive root: `/home/sikmindz/Coding`
- Output: `/home/sikmindz/Coding/Gloss/gloss-next-codex-context.zip`
- Include roots: `22`
- External Cargo path dependency roots: `21`
- Profile: `generic-rust` requested as `generic-rust`
- Mode: `next-codex-context`
- Package role: `next-codex-context`
- Strict: `True`
- Dry run: `False`
- Included files: `29774`
- Included bytes: `540221310`
- Excluded files: `8451`
- Pruned dirs: `33`
- Findings: `74` (`0` errors, `73` warnings)
- Archive zip-byte SHA-256: `faf308fe1d6fdceec210fc96a2c1429f1b1d31f5240f5bf967aaba562870f536`
- Archive hash semantics: `zip-byte-sha256-not-canonical-content-hash`
- Content manifest SHA-256: `a929c9295d208991fc2d8a7ac162bd0912d7b2cab91e7ce916cf69ca0cf77439`
- Ecosystems detected: `rust, node, git`
- Codex archive enabled: `True`
- Codex archive planned: `0`
- Codex archive moved: `0`
- Codex active stale after normalization: `0`
- Root Markdown archive enabled: `False`
- Root Markdown inspected: `6`
- Root Markdown protected: `3`
- Root Markdown candidates: `1`
- Root Markdown ambiguous: `2`
- Root Markdown moved: `0`
- Root Markdown collisions: `0`
- Root package archive enabled: `True`
- Root package inspected: `36`
- Root package protected: `8`
- Root package candidates: `2`
- Root package moved: `2`
- Root package skipped existing: `0`
- Root package collisions: `0`

## Ecosystem parity

| Ecosystem | Detected | Manifests | Missing expected | Dry-run status |
|---|---:|---:|---:|---|
| `rust` | `True` | 742 | 4 | `available-not-run` |
| `python` | `False` | 0 | 0 | `not-applicable` |
| `node` | `True` | 2 | 3 | `available-not-run` |
| `go` | `False` | 0 | 0 | `not-applicable` |
| `docker` | `False` | 0 | 0 | `not-applicable` |
| `git` | `True` | 1 | 1 | `available-not-run` |

## Decision provenance

- Decisions recorded: `38258`
- Includes: `29774`
- Excludes: `8451`
- Pruned dirs: `33`

## Validation findings

| Severity | Code | Path | Detail |
|---|---|---|---|
| warning | `script-ref-not-archived` | `Gloss/scripts/run_all_checks.sh` | Script reference exists but is not included: validation/gloss_secret_store_permissions_gate.py |
| warning | `secret-content-named-secret-assignment` | `AGENT-SYSTEM.md` | Potential secret-like content detected at line 1309; value intentionally not printed. |
| warning | `secret-content-named-secret-assignment` | `Gloss/.hermes/plans/2026-05-28-release-grade-hardening.md` | Potential secret-like content detected at line 26; value intentionally not printed. |
| warning | `secret-content-named-secret-assignment` | `Gloss/ollama_relevant_lines.txt` | Potential secret-like content detected at line 107; value intentionally not printed. |
| warning | `secret-content-named-secret-assignment` | `Gloss/src-tauri/src/providers/ollama.rs` | Potential secret-like content detected at line 327; value intentionally not printed. |
| warning | `secret-like-filename` | `Gloss/docs/codex-runs/GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526/SECRET_STORE_PERMISSION_RECEIPT.json` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/gio/src/auto/credentials.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/gio/src/auto/unix_credentials_message.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/hyper-rustls/examples/sample.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/aia_test_cert.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/alt_name_cert.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/authority_key_identifier.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/cert.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/certs.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/certv3.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/cms.p12` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/corrupted-rsa.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/csr.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/dhparams.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/dsa.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/dsaparam.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/identity.p12` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/intermediate-ca.key` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/intermediate-ca.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/key.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/keystore-empty-chain.p12` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/leaf.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/nid_test_cert.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/nid_uid_test_cert.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/root-ca.key` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/root-ca.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/rsa-encrypted.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/openssl/test/rsa.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/ring/src/ec/suite_b/private_key.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/ring/src/rsa/signature_rsa_example_private_key.der` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/ring/tests/ecdsa_test_private_key_p256.p8` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/ring/tests/ed25519_test_private_key.bin` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/ring/tests/ed25519_test_private_key.p8` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/ring/tests/rsa_test_private_key_2048.p8` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/cert.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/identity.p12` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/key.key` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/key.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/key_invalid_header.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/key_no_end_header.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/key_no_headers.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/schannel/test/key_wrong_header.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/tokio-native-tls/examples/identity.p12` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/tokio-native-tls/tests/identity.p12` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/tokio-rustls/tests/certs/chain.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/tokio-rustls/tests/certs/end.key` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/tokio-rustls/tests/certs/root.pem` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/tower-http/src/cors/allow_credentials.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/tower-http/src/follow_redirect/policy/filter_credentials.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/untrusted/mk/llvm-snapshot.gpg.key` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/web-sys/src/features/gen_Credential.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/webkit2gtk/src/auto/credential.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/webkit2gtk/src/credential.rs` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/winapi-i686-pc-windows-gnu/lib/libwinapi_mincore-api-ms-win-security-credentials-l1-1-0.a` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/winapi-i686-pc-windows-gnu/lib/libwinapi_onecore-api-ms-win-security-credentials-l1-1-0.a` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/winapi-i686-pc-windows-gnu/lib/libwinapi_onecoreuap-api-ms-win-security-credentials-l1-1-0.a` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/winapi-x86_64-pc-windows-gnu/lib/libwinapi_mincore-api-ms-win-security-credentials-l1-1-0.a` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/winapi-x86_64-pc-windows-gnu/lib/libwinapi_onecore-api-ms-win-security-credentials-l1-1-0.a` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/src-tauri/vendor/crates/winapi-x86_64-pc-windows-gnu/lib/libwinapi_onecoreuap-api-ms-win-security-credentials-l1-1-0.a` | File excluded because of secret-like-filename. |
| warning | `secret-like-filename` | `Gloss/validation/gloss_secret_store_permissions_gate.py` | File excluded because of secret-like-filename. |
| warning | `rust-expected-file-not-packaged` | `Cargo.lock` | rust adapter expected this existing file to be included. |
| warning | `rust-expected-file-not-packaged` | `Cargo.toml` | rust adapter expected this existing file to be included. |
| warning | `rust-expected-file-not-packaged` | `LICENSE` | rust adapter expected this existing file to be included. |
| warning | `rust-expected-file-not-packaged` | `README.md` | rust adapter expected this existing file to be included. |
| warning | `node-expected-file-not-packaged` | `LICENSE` | node adapter expected this existing file to be included. |
| warning | `node-expected-file-not-packaged` | `README.md` | node adapter expected this existing file to be included. |
| warning | `node-expected-file-not-packaged` | `package.json` | node adapter expected this existing file to be included. |
| warning | `git-expected-file-not-packaged` | `.gitignore` | git adapter expected this existing file to be included. |
| info | `git-metadata-excluded` | `.git/` | Git metadata detected and intentionally excluded from transferable package contents. |

## Included files by extension

| Extension | Count |
|---|---:|
| `.rs` | 21579 |
| `.json` | 2309 |
| `.md` | 2045 |
| `.toml` | 1100 |
| `<no-extension>` | 1090 |
| `.lock` | 511 |
| `.yml` | 381 |
| `.txt` | 213 |
| `.py` | 173 |
| `.sh` | 93 |
| `.0` | 57 |
| `.js` | 40 |
| `.yaml` | 29 |
| `.ts` | 27 |
| `.html` | 19 |
| `.tsx` | 19 |
| `.csv` | 16 |
| `.tpl` | 12 |
| `.spdx` | 10 |
| `.0_with_llvm-exception` | 8 |
| `.css` | 7 |
| `.jsx` | 7 |
| `.rst` | 6 |
| `.jsonl` | 4 |
| `.tsv` | 3 |
| `.conf` | 2 |
| `.mjs` | 2 |
| `.ndjson` | 2 |
| `.ps1` | 2 |
| `.template` | 2 |
| `.cfg` | 1 |
| `.log` | 1 |
| `.mit` | 1 |
| `.mkd` | 1 |
| `.patch` | 1 |
| `.sql` | 1 |

## Included files by top-level path

| Top-level path | Count |
|---|---:|
| `Gloss` | 29133 |
| `Libraries` | 588 |
| `00_README.md` | 1 |
| `01_OPERATOR_DECISION_BRIEF.md` | 1 |
| `02_SCOPE_AND_ASSUMPTIONS.md` | 1 |
| `03_REQUIRED_INPUTS.md` | 1 |
| `04_FORBIDDEN_CHANGES.md` | 1 |
| `05_RUN_ORDER.md` | 1 |
| `ACCEPTANCE_GATES.md` | 1 |
| `AGENT-SYSTEM.md` | 1 |
| `AGENTS-TEMPLATE.md` | 1 |
| `AGENTS.md` | 1 |
| `AGENT_LOG.md` | 1 |
| `Agent.md` | 1 |
| `Cat Info App.md` | 1 |
| `Coding-research-next-codex-context-20260525T185045Z.codex-archive.json` | 1 |
| `Coding.md` | 1 |
| `Director.md` | 1 |
| `FINAL_REPORT_TEMPLATE.md` | 1 |
| `GENERATED_FILE_TREE.txt` | 1 |
| `MANUAL_PHASE_INJECTIONS.md` | 1 |
| `MASTER_CODEBASE_REFERENCE2.md` | 1 |
| `MASTER_ISSUE_TENSOR.md` | 1 |
| `Medicine.md` | 1 |
| `PACK_METADATA.json` | 1 |
| `PHASE_00_PREFLIGHT.md` | 1 |
| `PHASE_01_LIBRARIES_CANONICAL_CLOSURE.md` | 1 |
| `PHASE_02_SALVAGE_TERMINAL_DECISIONS.md` | 1 |
| `PHASE_03_RESIDUAL_LIBRARIES2_REFS.md` | 1 |
| `PHASE_04_DOWNSTREAM_DEPENDENCY_REPAIR.md` | 1 |
| `PHASE_05_SEMANTIC_MEMORY_AND_GLOSS_BOUNDARY.md` | 1 |
| `PHASE_06_CLAIMLEDGER_FORGE_BOUNDARY.md` | 1 |
| `PHASE_07_GENERATED_ARTIFACT_HYGIENE.md` | 1 |
| `PHASE_08_VALIDATION_AND_RECEIPTS.md` | 1 |
| `PHASE_09_FINAL_AUDITOR_HANDOFF.md` | 1 |
| `PLANa.md` | 1 |
| `Phone.md` | 1 |
| `Pictures.md` | 1 |
| `Playground.md` | 1 |
| `Portal Doctor.md` | 1 |
| `ROLLBACK_PLAN.md` | 1 |
| `RecursiveOps.md` | 1 |
| `Research.md` | 1 |
| `STATEa.md` | 1 |
| `TRANSFER.md` | 1 |
| `VALIDATION_COMMANDS.md` | 1 |
| `WORKSPACE_MAP.md` | 1 |
| `backup.py` | 1 |
| `codex.md` | 1 |
| `gitdb.md` | 1 |
| `recall-codex.md` | 1 |
| `research-architectural-next-steps-2026-04-15.md` | 1 |
| `website.md` | 1 |
| `z.py` | 1 |
| `zip.py` | 1 |

## Exclusion reasons

| Reason | Count |
|---|---:|
| `unsupported-extension-or-basename` | 4769 |
| `binary-build-artifact` | 3210 |
| `log-disabled` | 230 |
| `image-disabled` | 72 |
| `generated-sidecar` | 60 |
| `secret-like-filename` | 60 |
| `max-file-size-exceeded` | 19 |
| `archive-file` | 14 |
| `non-utf8-text-file` | 12 |
| `database-file` | 2 |
| `doc-binary-disabled` | 2 |
| `generated-output` | 1 |

## Sidecar files

- Manifest: `/home/sikmindz/Coding/Gloss/gloss-next-codex-context.manifest.json`
- Markdown report: `/home/sikmindz/Coding/Gloss/gloss-next-codex-context.report.md`
- Excluded file list: `/home/sikmindz/Coding/Gloss/gloss-next-codex-context.excluded.json`
- Findings: `/home/sikmindz/Coding/Gloss/gloss-next-codex-context.findings.json`

## Interpretation

This package has warnings. It is probably usable, but the warnings should be reviewed before using it as a Codex or audit handoff.
