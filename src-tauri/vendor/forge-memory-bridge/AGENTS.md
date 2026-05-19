# AGENTS.md — forge-memory-bridge

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

This crate transforms Forge export payloads into typed memory import batches. It does not own
source truth, import authority, promotion policy, or authoritative store time.

## What this crate must do in the finish-line pass

1. preserve export provenance exactly,
2. validate the exported shape deterministically,
3. never invent missing version lineage,
4. carry only the fields memory needs for a correct import,
5. support the V2 transform path while keeping legacy helpers visibly fenced.

The current snapshot still ships V1 surfaces in code. Do not teach those compatibility paths as the
long-term architecture once the V2 seam exists.

## Do not do

- do not stamp authoritative imported `recorded_at`
- do not synthesize `supersedes_claim_version_id`
- do not decide promotion, comparability, or truth absent from the export
- do not query live memory to invent semantics
- do not let legacy upgrade helpers look canonical

## Primary files

- `src/batch.rs`
- `src/transform.rs`
- `src/legacy.rs`
- `tests/forge_bridge_memory_proof.rs`
