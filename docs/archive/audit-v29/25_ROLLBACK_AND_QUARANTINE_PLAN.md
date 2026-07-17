# Rollback and Quarantine Plan

## Rollback rules

- Every phase must record changed files before and after.
- Receipt/schema additions are append-only; do not destructively remove receipts during rollback.
- UI features may be disabled behind feature flags if they fail gates.
- Provider decoding settings must default to prior safe behavior if new settings fail.
- semantic-memory strict mode failure must block semantic-memory claims, not block local retrieval.
- TurboQuant proof failure demotes claims; it does not block local RAG release if all TQ claims are removed.

## Quarantine targets

- stale Codex run artifacts in active root;
- old P30/P33/P36 receipts treated as current;
- unrelated `/Coding` root docs in Gloss package;
- unsupported provider settings;
- broad spec phases attempted before RC gates pass;
- any reconstructed prompt not captured at generation time.

## Rollback receipt

Each phase must append:

```text
phase id
changed files
rollback command/path
quarantined files
remaining risks
```
