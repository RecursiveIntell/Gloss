# AGENTS.md — stack-ids

Read the root control plane before changing this crate:

1. `../CANONICAL_STACK_SPEC_V6.md`
2. `../CANONICAL_STACK_SPEC_V7_RECURSIVE_INFERENCE_KERNEL.md`
3. `../PACK_README.md`
4. `../SOURCE_BASIS.md`
5. `../STATUS_DASHBOARD.md`
6. `../MASTER_ISSUE_CHANGE_MATRIX.md`
7. `../CONFORMANCE_GATES.md`
8. `../PHASED_EXECUTION_PLAN.md`
9. `../IMPLEMENTATION_PLAYBOOK.md`
10. `../RISKS_AND_FORBIDDEN_SHORTCUTS.md`

## Scope

`stack-ids` owns only shared opaque primitives and helpers:

- IDs such as `AttemptId`, `TrialId`, `ClaimId`, `ClaimVersionId`, `EnvelopeId`, and
  `ImportBatchId`
- `Scope` and `ScopeKey`
- `TraceCtx` and bounded baggage helpers
- digests and format validation

## Do

- keep types opaque, parseable, and serialization-safe
- centralize cross-crate ID and trace primitives here instead of duplicating them elsewhere
- add invariant, round-trip, and trace-context tests when extending public surface
- keep new additions small and obviously reusable across crates

## Do not

- add business logic, storage rows, or semantic result types
- add retry policy or promotion policy here
- duplicate canonical primitives in downstream crates
- let convenience wrappers turn this crate into a generic contracts layer

## Finish-line focus

For the current closure pass, this crate mainly participates in:

- `TRACE-101` by preserving canonical ID and `TraceCtx` ownership
- `TRACE-101` through stable cross-crate retry/replay identifiers
- `TRACE-102` through stable queue-hop lineage identifiers
- `CONF-001` through root-visible proof that execution crates share one shipped release gate
