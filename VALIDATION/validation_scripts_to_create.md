# Validation Scripts to Create or Update

## validation/gloss_current_run_truth_gate.py

- Purpose: Validate CURRENT_RUN and all sidecars/receipts use the same run id
- Inputs: CURRENT_RUN.md, codex archive sidecars, final receipts
- Exact checks: No P30/P36 drift; active run consistent
- Pass/fail: JSON {ok, failures, warnings}
- Output: JSON {ok, failures, warnings}

## validation/gloss_stale_pass_surface_gate.py

- Purpose: Detect active stale pass artifacts in root and docs
- Inputs: root files, docs/codex-runs, AGENTS/README
- Exact checks: Old pass artifacts archived/quarantined; no stale active instructions
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_package_scope_gate.py

- Purpose: Ensure package root is Gloss + explicit deps only
- Inputs: z.py report/manifest
- Exact checks: No unrelated /Coding root docs
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_semantic_memory_runtime_truth_gate.py

- Purpose: Require runtime truth schema and answer attachment
- Inputs: source code, fixtures, sample receipt
- Exact checks: SemanticMemoryRuntimeTruthV1 exists and is used
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_retrieval_decision_gate.py

- Purpose: Require canonical RetrievalCapabilityDecisionV1
- Inputs: chat/memory/ui/evidence code
- Exact checks: Decision object live, no dead parallel state
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_generation_receipt_gate.py

- Purpose: Require GenerationReceiptV1 fields and sample
- Inputs: schemas, db, chat code, receipt sample
- Exact checks: generation receipt captures provider/model/status
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_prompt_receipt_gate.py

- Purpose: Require PromptReceiptV1 with capture states
- Inputs: schemas, chat/context/ui
- Exact checks: prompt capture/redaction states honest
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_decoding_settings_gate.py

- Purpose: Require decoding settings provider map and receipts
- Inputs: providers/settings/ui/schemas
- Exact checks: temperature configurable; effective settings captured; unsupported not sent
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_timeout_partial_continuation_gate.py

- Purpose: Require timeout increase, partial save, continuation
- Inputs: chat/store/ui/tests
- Exact checks: 40% increase receipt; partial continuation works
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_inspector_dock_gate.py

- Purpose: Require Inspector Dock tabs and Notes preservation
- Inputs: frontend components/tests
- Exact checks: Notes, Prompt, Evidence, Receipt, Sources render
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_live_semantic_memory_smoke_gate.py

- Purpose: Strict live semantic-memory fixture
- Inputs: live fixture receipts
- Exact checks: backend_used semantic-memory-preview; fallback false
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_turboquant_runtime_gate.py

- Purpose: Prove/demote TurboQuant
- Inputs: TQ receipt/public diff
- Exact checks: exact rerank and artifact digest or claim removed
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_release_candidate_gate.py

- Purpose: Aggregate all RC gates
- Inputs: all receipts and command results
- Exact checks: release_ready legal only when all pass
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}

## validation/gloss_broad_spec_completion_gate.py

- Purpose: Track broad spec progress after RC
- Inputs: feature matrix/gap ledger
- Exact checks: broad feature complete only when all broad gates pass
- Pass/fail: JSON
- Output: JSON {ok, failures, warnings}
