# AGENTS.md — Gloss Provenance Repair Governance

Gloss is a local-first Tauri + React + Rust notebook/RAG app. Work must preserve truthful source scope, citation proof, local-first behavior, and receipt-backed execution.

## Current truth rule

Current source files, current validation output, and fresh package replay outrank old release receipts, old issue matrices, README claims, and generated reports. P33 receipts are quarantined until `scripts/gloss_receipt_integrity_gate.py` and fresh-unzip replay pass.

## Canonical semantic owners

- Source scope: `src-tauri/src/retrieval/source_scope.rs` plus generated TS schema.
- Retrieval proof: retrieval outcome + RetrievalReceiptV1; fallback class is not ranked proof.
- Message evidence: MessageEvidenceEnvelopeV1, not `citations` string/union storage.
- Chat attempt identity: ConversationTurnV1 links client_request_id, user_message_id, assistant_message_id, attempt_id, trace_id.
- Source lifecycle: SourceLifecycleEventV1 and SourceImportReceiptV1.
- Provider/tool calls: LlmInvocationReceiptV1 and ToolInvocationReceiptV1.
- Settings: SettingsContractV1 with typed generated Rust/TS bindings.
- Release truth: fresh-unzip replay receipt for the exact archive being shipped.

## Forbidden

- No silent source-scope widening.
- No fallback answer marketed as indexed retrieval.
- No source content with system-message authority.
- No polymorphic `citations`/evidence fields.
- No mutable projection as canonical truth.
- No global `#![allow(dead_code)]` in release state.
- No missing scripts referenced by package.json, AGENTS.md, receipts, or validation manifests.
- No old root docs or Codex artifacts as active truth.
- No release-ready claim while any S0/S1 issue remains open.

## Required gates

Run these before final handoff:

```bash
python3 scripts/gloss_preflight_gate.py --repo .
python3 scripts/gloss_receipt_integrity_gate.py --repo .
python3 scripts/gloss_semantic_naming_gate.py --repo .
python3 scripts/gloss_source_scope_fixture_gate.py --repo .
python3 scripts/gloss_validation_manifest.py --repo . --run-release-blocking
npm ci
npm run build
npm run test
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-backend
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
```

## Final report shape

Changed files; commands run; pass/fail/skipped checks; S0/S1 closure table; receipt/schema additions; source-of-truth decisions; remaining blockers/risks; rollback path; release decision.
