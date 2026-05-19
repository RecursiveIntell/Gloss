# AGENTS.md — Gloss Chat Runtime Fix

## Scope

This repo is Gloss. Current task scope is **chat runtime reliability**. The current user-facing defect is that sending a prompt to chat does not visibly produce a response for local Ollama models.

## Do not broaden scope

Do not perform broad redesign, product README rewrite, semantic-memory promotion, TurboQuant promotion, UI restyling, packaging overhaul, or general cleanup unless directly required to make chat respond or make the failure observable.

## Source-of-truth hierarchy

1. Current files in the repository.
2. Latest package/run evidence under `docs/codex-runs/`.
3. This pass bundle.
4. Prior docs/specs.
5. Memory or prior prose.

## Hard rules

- Every chat attempt must produce visible streamed tokens, visible error, visible timeout, or durable trace.
- No silent no-response path is allowed.
- Provider config must have one source of truth.
- Frontend chat events must be routed by stream identity, not only by current active notebook.
- Semantic-memory failures must not block provider streaming when fallback is enabled.
- TurboQuant must remain candidate-only and exact-reranked.
- Release readiness requires live desktop smoke.
- Done without receipts is forbidden.

## Required evidence

For each phase, write a phase report under:

```text
docs/codex-runs/CHAT_RUNTIME_FIX_20260518/
```

Include:

- files inspected;
- files changed;
- commands run;
- tests passed/failed/skipped;
- unresolved risks;
- exact blockers.

## Manual stop conditions

Stop and report if:

- provider URL source-of-truth cannot be resolved safely;
- chat stream events cannot be associated to message/conversation identity;
- provider-only smoke cannot be implemented;
- desktop smoke cannot run and no substitute proof exists;
- any fix would disable TurboQuant or weaken exact rerank;
- any change would hide semantic-memory failure instead of disclosing fallback.
