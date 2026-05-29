# Phase Plan

## Phase 00 — Preflight and baseline

Record git/tool/package state. Identify actual current run, package manifest locations, build/test commands, and tool availability. No edits before report.

## Phase 01 — Chat stream termination

Fix provider done-frame handling. Add mock provider test for done=true without EOF. Frontend must finalize deterministically.

## Phase 02 — Partial persistence and cancellation

Persist partial outputs and terminal states for timeout/error/cancel. Make stop backend-authoritative.

## Phase 03 — Runtime gates and background preemption

Ensure foreground chat preempts/degrades background summary work. Add visible phase state and gate-owner receipts.

## Phase 04 — Validation gate hardening

Fix package scope gate, missing secret-store gate, release gate hang, false-positive static gates, and gate schema consistency.

## Phase 05 — Package/run truth and transferability

Separate Codex context from release package. Add RunTruthV1, PackManifestV1, GitStateReceiptV1, ToolAvailabilityReceiptV1, evidence index.

## Phase 06 — Release proof

Run/prove Rust checks, frontend checks, live/mock Ollama smoke, live desktop smoke, semantic-memory smoke, TurboQuant runtime proof, installer smoke or explicit release demotion.

## Phase 07 — Final audit and claim boundary

Generate release proof packet, public claim diff, remaining delta, rollback plan, and hostile auditor handoff.
