# Rollback and Quarantine Plan

## Branching

Create a new branch before edits:

```bash
git checkout -b gloss-total-completion-hardening-20260526
```

## Rollback

- Use git commits at phase boundaries.
- If a phase fails guardrail, revert only that phase or quarantine changed files.
- Do not delete prior truth-bearing state; supersede or quarantine with receipt.

## Quarantine targets

- stale Codex run docs/logs
- old package manifests
- unused vendor crates if not wired into deterministic build
- compatibility shims without proof
- speculative research-derived claims without promotion packet
- unsupported extractor outputs
- failed import batches
- corrupted DB rows detected by DB doctor

## Quarantine receipt minimum

Each quarantine record must include target, reason, source evidence, recorded time, owner, review path, rollback path, and whether public claims are affected.
