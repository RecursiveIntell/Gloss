# semantic-memory-forge

Forge verification truth: evidence bundles, export envelopes, execution
context, causal estimation metadata, and refutation artifacts for the local
AI systems stack.

`semantic-memory-forge` owns raw verification truth. It does not own queryable
memory storage, bridge transformation, retrieval ranking, or runtime planning.
Its job is to define the evidence and export contracts that downstream crates
can validate, transform, import, and audit without inventing missing truth.

## Contents

- [What it provides](#what-it-provides)
- [Current package status](#current-package-status)
- [Install](#install)
- [Quick start](#quick-start)
- [Authority boundary](#authority-boundary)
- [Evidence bundles](#evidence-bundles)
- [Export envelopes](#export-envelopes)
- [Execution context and tool receipts](#execution-context-and-tool-receipts)
- [Semantic and proof surfaces](#semantic-and-proof-surfaces)
- [Causal and experimental surfaces](#causal-and-experimental-surfaces)
- [Validation](#validation)
- [Integration path](#integration-path)
- [Operational notes](#operational-notes)
- [Minimum supported Rust version](#minimum-supported-rust-version)
- [License](#license)

## What it provides

- `EvidenceBundle` and related causal verification records.
- Export envelope contracts: `ExportEnvelopeV1`, `ExportEnvelopeV2`, and
  canonical `ExportEnvelopeV3`.
- Export records for claims, relations, episodes, entity aliases, and evidence
  references.
- Export metadata carrying authority, run ID, direct-write disclosure,
  comparability snapshot version, and export timestamp.
- Content-digest computation and validation for exported envelopes.
- Causal question, treatment, outcome, refutation, trial, comparability, and
  promotion-state structs.
- Execution context and episode bundle proof surfaces.
- Tool receipt contracts for operator-visible execution evidence.
- V11 semantic, witness, certificate, refutation, degradation, exactness, and
  oracle-slice surfaces.
- V13 support algebra, bilattice truth, contradiction witness, retraction, and
  claim-state surfaces.
- V14 intervention, outcome schema, cohort contract, and counterfactual slice
  surfaces.
- JSON Schema constants for supported artifact families.

## Current package status

| Property | Value |
| --- | --- |
| Crate | `semantic-memory-forge` |
| Current local version | `0.1.1` |
| Rust edition | 2021 |
| MSRV | Rust 1.75 |
| License | Apache-2.0 |
| Primary dependency | `stack-ids` |
| Storage | None |
| Network | None |
| Authority | Raw verification truth and export envelopes |

## Install

```toml
[dependencies]
semantic-memory-forge = "0.1.1"
```

## Quick start

```rust
use semantic_memory_forge::{ExportEnvelopeV3, ExportRecordV3};

fn inspect_export(envelope: &ExportEnvelopeV3) -> Result<usize, semantic_memory_forge::ExportEnvelopeError> {
    envelope.validate()?;
    Ok(envelope.records.len())
}

fn inspect_record(record: &ExportRecordV3) -> bool {
    record.semantics.is_some()
}
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Authority boundary

This crate is authoritative for:

- raw verification evidence schemas,
- export envelope schemas,
- causal estimation metadata,
- refutation and promotion state,
- semantic/proof/degradation artifact shapes,
- direct-write disclosure fields,
- digest validation for Forge-owned envelopes.

This crate is not authoritative for:

- projection import transformation,
- memory commit timestamps,
- SQLite storage,
- search ranking,
- query planning,
- runtime policy,
- kernel execution.

The intended authority split is:

| Crate | Responsibility |
| --- | --- |
| `semantic-memory-forge` | Raw verification truth and export envelopes |
| `forge-memory-bridge` | Transformation from export envelopes to import batches |
| `semantic-memory` | Queryable projected truth and importer-assigned `recorded_at` |
| `knowledge-runtime` | Query planning and result merge behavior |

## Evidence bundles

`EvidenceBundle` captures the methodological chain behind a verification result:

- causal question,
- treatment specification,
- outcome specification,
- estimator metadata,
- environment fingerprint,
- sidecar execution details,
- verification trials,
- comparability snapshot,
- refutation attempts,
- refutation artifact records,
- verification summary,
- lifecycle state,
- promotion state.

Useful methods:

- `EvidenceBundle::new(...)`
- `all_refutations_passed()`
- `has_failed_refutation()`

Evidence bundles are designed to be exported, bridged, imported, and audited
without becoming normal memory search payloads by default.

## Export envelopes

The canonical export contract is `ExportEnvelopeV3`.

V3 preserves:

- `EnvelopeId`,
- schema version,
- `ScopeKey`,
- optional `TraceCtx`,
- `ForgeExportMeta`,
- optional `EvidenceBundle`,
- typed export records,
- support sets,
- contradiction witnesses,
- retraction records,
- V13 claim states,
- V14 experiment/control surfaces,
- V15 exchange/admission surfaces,
- content digest.

Export records cover:

- `ExportClaim`
- `ExportRelation`
- `ExportEpisode`
- `ExportEntityAlias`
- `ExportEvidenceRef`

V1 and V2 envelope types remain available for migration compatibility. New
integrations should emit V3.

## Execution context and tool receipts

The crate includes execution evidence contracts used by local agent paths:

- `ExecutionContextV1`
- `EpisodeBundleV1`
- `ForgeToolReceiptV1`
- `ForgeToolReceiptV2`
- `ForgeToolBudgetContext`

These surfaces are meant to make execution context, operator contract, tool
budget, receipt, and degradation/proof state explicit instead of relying on
logs or prose.

## Semantic and proof surfaces

V11 types define semantic and proof disclosure structures:

- `SemanticsProfileV1`
- `ClaimStateV1`
- `WitnessArtifactV1`
- `CertificateArtifactV1`
- `RefutationArtifactV1`
- `SemanticDiffV1`
- `OracleSliceContractV1`
- `CausalAttributionBundleV1`
- `DegradationRecordV1`
- `ExactnessBudgetV1`

V13 extends the proof surface with:

- `SupportSetV1`
- `SupportExprV1`
- `SupportTokenV1`
- `QualityVectorV1`
- `ContradictionWitnessV1`
- `RetractionRecordV1`
- `ClaimStateV13`
- `BilatticeTruthV1`

These are data contracts. They do not execute policy or prove claims by
themselves.

## Causal and experimental surfaces

V14 types preserve richer causal and experimental artifacts:

- `InterventionBundleV1`
- `OutcomeSchemaV1`
- `CohortContractV1`
- `CounterfactualSliceV1`

The export envelope also preserves additional V14/V15 artifacts as JSON values
where the downstream crates need to carry the surface before tighter typed
contracts are introduced.

## Validation

Validation methods return explicit errors rather than silently filling missing
truth:

- `ExportEnvelopeV1::validate()`
- `ExportEnvelopeV2::validate()`
- `ExportEnvelopeV3::validate()`
- `SemanticsProfileV1::validate()`
- `ClaimStateV1::validate()`
- `WitnessArtifactV1::validate()`
- `CertificateArtifactV1::validate()`
- `RefutationArtifactV1::validate()`
- `SemanticDiffV1::validate()`
- `OracleSliceContractV1::validate()`
- `CausalAttributionBundleV1::validate()`
- `DegradationRecordV1::validate()`
- `ExactnessBudgetV1::validate()`

Envelope errors expose stable `kind()` strings for diagnostic and receipt
surfaces.

## Integration path

The normal pipeline is:

```text
Forge evidence and records
  -> ExportEnvelopeV3
  -> forge-memory-bridge::transform_envelope_v3()
  -> ProjectionImportBatchV3
  -> semantic-memory::MemoryStore::import_projection_batch()
```

Important ownership rule:

- Forge owns `source_exported_at`.
- Bridge owns `transformed_at`.
- Memory owns authoritative imported `recorded_at`.

## Operational notes

- The crate performs no I/O.
- The crate has no network behavior.
- Direct-write is an exceptional disclosed state, not the normal path.
- Thin exports must remain explicit; downstream crates must not invent missing
  lineage, grouping, or semantics.
- Evidence payloads should remain auditable and explicit rather than being
  silently materialized into normal search results.
- Prefer `ExportEnvelopeV3` for new integrations.

## Minimum supported Rust version

`semantic-memory-forge` declares `rust-version = "1.75.0"` and uses Rust 2021.

## License

Apache-2.0. See [LICENSE](LICENSE).
