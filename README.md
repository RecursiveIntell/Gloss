# Gloss Closing / Hardening Final Repair Pack

Generated: `2026-05-27T23:29:04Z`
Active run: `GLOSS_TOTAL_COMPLETION_AND_HARDENING_SUPERPASS_20260526`

This pack converts the prior hostile audits plus current post-run code inspection into a **verification-first Codex closure bundle**. It is designed to fix the real chat/Ollama failure path and every broken proof/tool/addition surfaced by the audits, without guessing.

## Controlling evidence

- Current package: `Gloss-generic-rust-next-codex-context-20260527T064920Z.*`
- Prior five-angle hostile audit addendum: `gloss_5_hostile_audits_20260527`
- Current source snippets inspected in extracted package:
  - `src/components/chat/ChatPanel.tsx`
  - `src/stores/chatStore.ts`
  - `src/stores/sourceStore.ts`
  - `src/App.tsx`
  - `src/lib/tauri.ts`
  - `src-tauri/src/commands/chat/mod.rs`
  - `src-tauri/src/providers/mod.rs`
  - `src-tauri/src/providers/ollama.rs`
  - `src-tauri/src/commands/settings.rs`

## How to use

1. Copy or attach this entire pack to the next Codex run.
2. Start Codex with `MASTER_PROMPT.md`.
3. Use `AGENTS.md` as the repo-level guidance candidate.
4. Execute phase prompts in `phase_prompts/` in order.
5. Copy `validation/` scripts into the Gloss repo only after review, then run `validation/run_closing_gates.sh`.
6. Refuse completion unless `FINAL_HANDOFF_TEMPLATE.md` is fully populated.

## Hard rule

No release claim, no public claim, no “fixed” claim until the live Ollama smoke + ChatAttemptTraceV1 proves the failing branch is gone.
