# stack-ids

Shared identity, scope, trace, and digest primitives for the local-first AI
systems stack.

`stack-ids` is the identity foundation used by the stack's memory, bridge,
verification, runtime, kernel, governance, and evidence crates. It provides
opaque ID newtypes, scope partition keys, W3C-compatible trace context helpers,
canonical BLAKE3 content digests, and shared status/citation structs.

This crate intentionally owns type contracts only. It does not own the rows,
files, envelopes, policies, schemas, or runtime behavior that those identifiers
point to.

## Contents

- [What it provides](#what-it-provides)
- [Current package status](#current-package-status)
- [Install](#install)
- [Quick start](#quick-start)
- [Identity newtypes](#identity-newtypes)
- [Scope keys](#scope-keys)
- [Trace context](#trace-context)
- [Content digests](#content-digests)
- [Status and citations](#status-and-citations)
- [Authority boundary](#authority-boundary)
- [Serialization and schemas](#serialization-and-schemas)
- [Operational notes](#operational-notes)
- [Minimum supported Rust version](#minimum-supported-rust-version)
- [License](#license)

## What it provides

- Opaque string-backed ID newtypes for stack artifacts.
- UUID v4 generation for every ID type.
- Transparent serde serialization for wire and storage contracts.
- JSON Schema derivation through `schemars`.
- `Scope` and `ScopeKey` for namespace/domain/workspace/repository partitioning.
- Legacy namespace conversion helpers for migration paths.
- `TraceCtx` with W3C `traceparent` parsing/formatting.
- Bounded trace baggage with explicit validation errors.
- Deterministic conversion from legacy trace IDs to W3C trace IDs.
- `ContentDigest` for canonical BLAKE3 digests over bytes, strings, and JSON.
- `DigestBuilder` for incremental multi-field digest domains.
- Shared surface-status and constitution-citation types.

## Current package status

| Property | Value |
| --- | --- |
| Crate | `stack-ids` |
| Current local version | `0.1.1` |
| Rust edition | 2021 |
| MSRV | Rust 1.75 |
| License | Apache-2.0 |
| Storage | None |
| Network | None |
| Primary dependency role | Foundation crate |

## Install

```toml
[dependencies]
stack-ids = "0.1.1"
```

## Quick start

```rust
use stack_ids::{ContentDigest, EntityId, Scope, ScopeKey, TraceCtx};

let entity_id = EntityId::generate();
assert!(!entity_id.is_empty());

let scope = Scope::new("prod")
    .with_domain("code")
    .with_workspace("workspace-1")
    .with_repo("repo-1");

let scope_key: ScopeKey = scope.key();
assert_eq!(scope_key.to_string(), "prod/code@workspace-1#repo-1");

let trace = TraceCtx::generate();
let child = trace.child("abcdef0123456789");
let traceparent = child.to_traceparent()?;
let parsed = TraceCtx::from_traceparent(&traceparent)?;
assert_eq!(parsed.trace_id, child.trace_id);

let digest = ContentDigest::compute_str("stable input");
assert_eq!(digest.hex().len(), 64);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Identity newtypes

ID types are opaque wrappers around strings. They exist to prevent accidental
cross-use of semantically different identifiers that happen to have the same
runtime representation.

Each ID type supports:

- `new(value)`
- `generate()` using UUID v4
- `as_str()`
- `is_empty()`
- `Display`
- `FromStr`
- `From<String>`
- `From<&str>`
- `AsRef<str>`
- `Serialize` and `Deserialize`
- `JsonSchema`
- `Debug`, `Clone`, `PartialEq`, `Eq`, `Hash`, `PartialOrd`, and `Ord`

Representative ID families include:

| Family | Examples |
| --- | --- |
| Core truth | `EnvelopeId`, `ClaimId`, `ClaimVersionId`, `EntityId`, `EpisodeId` |
| Projection | `ProjectionId`, `RelationId`, `RelationVersionId`, `ImportBatchId` |
| Execution | `AttemptId`, `TrialId`, `KernelRunId`, `ExecutionPermitId` |
| Kernel | `ConstraintId`, `HyperedgeId`, `ResidualId`, `SyndromeId`, `WitnessId`, `CertificateId` |
| Verification | `VerificationCaseId`, `CheckPlanId`, `ControlReceiptId`, `PolicyDecisionId` |
| Governance | `ApprovalGrantId`, `PromotionDecisionId`, `RollbackPlanId`, `CalibrationSnapshotId` |
| Semantics | `SemanticsProfileId`, `ClaimStateId`, `SemanticDiffId`, `SupportSetId` |
| Experiment | `InterventionId`, `ExperimentCaseId`, `CohortContractId`, `DecisionTraceId` |
| Attestation | `AttestationEnvelopeId`, `TrustRootSetId`, `TransparencyReceiptId` |
| Effect | `EffectIntentId`, `EffectWindowId`, `EffectCommitDecisionId`, `CompensationPlanId` |
| Authority | `CapabilityClassId`, `AuthorityLeaseId`, `DelegationBundleId`, `BreakGlassGrantId` |
| Release and safety | `DeploymentProfileId`, `AssuranceCaseId`, `HazardRegisterId`, `CertificationBundleId` |

## Scope keys

`Scope` is the builder-style input shape. `ScopeKey` is the compact partition key
used by storage, import, and query contracts.

```rust
use stack_ids::{Scope, ScopeKey};

let scope = Scope::new("engineering")
    .with_domain("memory")
    .with_workspace("local")
    .with_repo("semantic-memory");

let key = scope.key();
assert_eq!(key.namespace, "engineering");
assert_eq!(key.domain.as_deref(), Some("memory"));
```

Display format:

```text
namespace[/domain][@workspace_id][#repo_id]
```

Examples:

- `prod`
- `prod/code`
- `prod/code@workspace-1`
- `prod/code@workspace-1#repo-1`

Legacy namespace helpers:

- `ScopeKey::namespace_only(ns)`
- `ScopeKey::from_legacy_namespace(ns)`
- `scope_key.to_legacy_namespace()`
- `scope_key.is_namespace_only()`

## Trace context

`TraceCtx` is the stack's shared trace-correlation shape. It is compatible with
W3C trace context, while still supporting deterministic migration from legacy
trace IDs.

```rust
use stack_ids::TraceCtx;

let trace = TraceCtx::generate()
    .with_parent("0123456789abcdef")
    .add_baggage("tenant", "local")?;

let header = trace.to_traceparent()?;
let parsed = TraceCtx::from_traceparent(&header)?;
assert_eq!(parsed.trace_id, trace.trace_id);
# Ok::<(), Box<dyn std::error::Error>>(())
```

Baggage limits are enforced:

- maximum 16 entries,
- maximum 256 bytes per key,
- maximum 256 bytes per value.

Legacy trace IDs can be converted without losing determinism:

```rust
let trace = TraceCtx::from_legacy_trace_id("old-run-id");
assert_eq!(trace.trace_id.len(), 32);
```

## Content digests

`ContentDigest` is a canonical BLAKE3 hex digest wrapper.

```rust
use stack_ids::{ContentDigest, DigestBuilder};
use serde_json::json;

let bytes_digest = ContentDigest::compute(b"bytes");
let text_digest = ContentDigest::compute_str("text");
let json_digest = ContentDigest::compute_json(&json!({
    "b": 2,
    "a": 1
}))?;

let composed = DigestBuilder::new()
    .update_str("domain")
    .separator()
    .update_str("value")
    .finalize();

assert_eq!(bytes_digest.hex().len(), 64);
assert_eq!(text_digest.hex().len(), 64);
assert_eq!(json_digest.hex().len(), 64);
assert_eq!(composed.hex().len(), 64);
# Ok::<(), Box<dyn std::error::Error>>(())
```

JSON digesting normalizes object keys recursively so equivalent JSON objects
hash consistently across map-order differences.

## Status and citations

The crate also exports small shared structs/enums used across stack contracts:

- `PhaseStatus`
- `SurfaceStatus`
- `V25ConstitutionCitation`

These are lightweight data contracts, not policy engines.

## Authority boundary

`stack-ids` is authoritative for:

- typed ID shapes,
- scope shape and display format,
- trace context shape,
- digest wrapper semantics.

`stack-ids` is not authoritative for:

- what an ID points to,
- whether an artifact exists,
- whether an artifact is valid,
- how rows are stored,
- how runtime policy is enforced,
- how memory retrieval is ranked.

Those responsibilities belong to the consuming domain crates.

## Serialization and schemas

All public ID wrappers are serde-transparent. This means a JSON field such as
`ClaimId` serializes as a string, not an object:

```json
{
  "claim_id": "6f00d9e5-1e1c-47aa-bf2c-a4fb5a3b7d57"
}
```

Types derive `JsonSchema` where appropriate so higher-level crates can generate
schema bundles without redefining identity primitives.

## Operational notes

- The crate performs no I/O.
- The crate has no network behavior.
- Generated IDs are UUID v4 strings.
- Empty strings are representable because some import and migration paths need
  to validate legacy data explicitly.
- Prefer typed IDs at API boundaries instead of raw `String`.
- Use `ScopeKey` instead of packing scope into ad hoc namespace strings for new
  integrations.

## Minimum supported Rust version

`stack-ids` declares `rust-version = "1.75.0"` and uses Rust 2021.

## License

Apache-2.0. See [LICENSE](LICENSE).
