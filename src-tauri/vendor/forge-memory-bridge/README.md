# forge-memory-bridge

Transformation layer from Forge export envelopes to `semantic-memory`
projection import batches.

`forge-memory-bridge` sits between raw verification truth and queryable
projected memory. It validates source export envelopes, maps Forge-owned record
types into memory import records, preserves provenance and semantic disclosure,
and emits batch contracts that `semantic-memory` can commit atomically.

The bridge does not persist state, query memory, evaluate comparability, or
invent missing lineage. It owns transformation only.

## Contents

- [What it provides](#what-it-provides)
- [Current package status](#current-package-status)
- [Install](#install)
- [Quick start](#quick-start)
- [Authority boundary](#authority-boundary)
- [Canonical pipeline](#canonical-pipeline)
- [Projection import batches](#projection-import-batches)
- [Record mapping](#record-mapping)
- [Provenance and timestamps](#provenance-and-timestamps)
- [Errors and failure artifacts](#errors-and-failure-artifacts)
- [Compatibility surfaces](#compatibility-surfaces)
- [Operational notes](#operational-notes)
- [Minimum supported Rust version](#minimum-supported-rust-version)
- [License](#license)

## What it provides

- `transform_envelope_v3()` for the canonical export-to-import path.
- `ProjectionImportBatchV3`, the canonical kernel-oriented import batch.
- `ProjectionImportBatchV2` for normalized compatibility import flows.
- Deprecated V1 compatibility types and helpers for migration windows.
- Import record shapes for claims, relations, episodes, entity aliases, and
  evidence references.
- V3 record semantics preservation from Forge exports.
- Preservation of evidence bundles, execution context, support sets,
  contradiction witnesses, retraction records, claim states, and V14/V15
  additive artifacts.
- Stable bridge errors with diagnostic `kind()` strings.
- `BridgeImportFailureArtifact` for serializable failure reporting.
- Trace context bridging when a source envelope lacks one.

## Current package status

| Property | Value |
| --- | --- |
| Crate | `forge-memory-bridge` |
| Current local version | `0.1.1` |
| Rust edition | 2021 |
| MSRV | Rust 1.75 |
| License | Apache-2.0 |
| Depends on | `stack-ids`, `semantic-memory-forge` |
| Storage | None |
| Network | None |
| Authority | Export-to-import transformation |

## Install

```toml
[dependencies]
forge-memory-bridge = "0.1.1"
```

## Quick start

```rust
use forge_memory_bridge::{transform_envelope_v3, ProjectionImportBatchV3};
use semantic_memory_forge::ExportEnvelopeV3;

fn bridge(envelope: &ExportEnvelopeV3) -> Result<ProjectionImportBatchV3, forge_memory_bridge::BridgeError> {
    transform_envelope_v3(envelope)
}
```

The returned batch is ready for:

```rust
let receipt = memory_store.import_projection_batch(batch).await?;
```

## Authority boundary

This crate is authoritative for:

- transformation from Forge export vocabulary to memory import vocabulary,
- projection import batch shape,
- bridge-assigned `transformed_at`,
- preservation of source provenance,
- serializable bridge failure artifacts.

This crate is not authoritative for:

- source evidence truth,
- source export timestamps,
- memory commit timestamps,
- SQLite storage,
- query ranking,
- proof acceptance,
- comparability decisions,
- runtime planning.

Authority split:

| Crate | Responsibility |
| --- | --- |
| `semantic-memory-forge` | Raw verification truth and `ExportEnvelopeV3` |
| `forge-memory-bridge` | Transformation into `ProjectionImportBatchV3` |
| `semantic-memory` | Atomic import, query storage, and authoritative `recorded_at` |
| `knowledge-runtime` | Planning and merge behavior |

## Canonical pipeline

```text
ExportEnvelopeV3
  -> transform_envelope_v3()
  -> ProjectionImportBatchV3
  -> semantic-memory import_projection_batch()
```

V3 is the preferred path for new integrations because it preserves:

- record-level semantics,
- evidence bundle payloads,
- execution context,
- episode bundle proof surface,
- support sets,
- contradiction witnesses,
- retraction lineage,
- claim states,
- experimental/control artifacts,
- source and bridge timestamps.

## Projection import batches

Batch versions:

| Type | Status | Notes |
| --- | --- | --- |
| `ProjectionImportBatchV1` | Deprecated compatibility | Retained for migration only. |
| `ProjectionImportBatchV2` | Compatibility/current storage bridge | Preserves evidence bundle and execution context. |
| `ProjectionImportBatchV3` | Canonical | Rich kernel-oriented batch with V13/V14/V15 surfaces. |

`ProjectionImportBatchV3` contains:

- source envelope ID,
- import schema version,
- source export schema version,
- content digest,
- source authority,
- `ScopeKey`,
- optional `TraceCtx`,
- `source_exported_at`,
- `transformed_at`,
- optional export metadata,
- optional evidence bundle,
- optional episode bundle,
- optional execution context,
- additive proof/semantic/experiment artifacts,
- transformed import records.

## Record mapping

The bridge maps Forge export records into import records:

| Forge record | Import record |
| --- | --- |
| `ExportClaim` | `ImportClaimVersion` |
| `ExportRelation` | `ImportRelationVersion` |
| `ExportEpisode` | `ImportEpisodeRecord` |
| `ExportEntityAlias` | `ImportEntityAlias` |
| `ExportEvidenceRef` | `ImportEvidenceRef` |

Mapping rules are conservative:

- preserve IDs supplied by Forge,
- preserve source envelope provenance,
- preserve scope,
- preserve trace context,
- preserve source confidence/freshness/contradiction state,
- do not invent supersession lineage,
- do not infer missing entity aliases,
- do not materialize evidence payloads into normal claim text.

## Provenance and timestamps

Temporal ownership is intentionally split:

| Field | Owner | Meaning |
| --- | --- | --- |
| `source_exported_at` | `semantic-memory-forge` | When the source envelope was emitted. |
| `transformed_at` | `forge-memory-bridge` | When the bridge produced the import batch. |
| `recorded_at` | `semantic-memory` | When the importing store committed projected rows. |

The bridge must not stamp importer-owned `recorded_at`; that is assigned by
`semantic-memory` during the atomic import transaction.

## Errors and failure artifacts

`BridgeError` covers:

- invalid source envelopes,
- unsupported schema versions,
- missing required source fields,
- transformation failures,
- serialization failures.

Errors expose `kind()` for stable diagnostics.

`BridgeImportFailureArtifact` turns a bridge error into a serializable failure
record with:

- source envelope ID,
- scope,
- error kind,
- message,
- failure timestamp.

## Compatibility surfaces

Deprecated compatibility helpers remain hidden for migration windows:

- `transform_legacy_envelope`
- `upgrade_legacy_envelope`
- `LegacyImportEnvelopeV1`
- `LegacyImportRecord`
- `LegacyEpisodeMeta`

New integrations should use `ExportEnvelopeV3`, `transform_envelope_v3()`, and
`ProjectionImportBatchV3`.

## Operational notes

- The crate performs no I/O.
- The crate has no network behavior.
- The crate does not store state.
- Transformation is deterministic for a given source envelope except for the
  bridge-owned `transformed_at` timestamp and generated trace context when the
  source omits one.
- Source digest validation belongs to the Forge envelope validation path.
- Import atomicity belongs to `semantic-memory`.

## Minimum supported Rust version

`forge-memory-bridge` declares `rust-version = "1.75.0"` and uses Rust 2021.

## License

Apache-2.0. See [LICENSE](LICENSE).
