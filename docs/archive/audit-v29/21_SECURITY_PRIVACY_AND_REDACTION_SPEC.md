# Security, Privacy, and Redaction Spec

## Required RC controls

- local-first default;
- cloud providers opt-in;
- API keys in secret store, not logs/receipts;
- prompt receipts redacted by policy;
- file access limited to user-selected sources;
- Tauri CSP and permissions audited;
- provider URLs validated/safely displayed;
- package warnings reviewed;
- no raw prompt export unless redaction policy passes.

## Prompt Inspector privacy

- show redacted previews by default;
- allow copy only with explicit user action;
- receipt records redaction state;
- old answers with missing prompt data show `not_captured`.

## Package hygiene

- command receipts included as `.jsonl`/`.md`, not excluded `.log` only;
- no unrelated root docs in Gloss package;
- generated sidecars excluded or intentionally included with role.
