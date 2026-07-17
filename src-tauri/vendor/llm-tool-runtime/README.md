# llm-tool-runtime

Provider-agnostic tool contracts, registry, dispatch, and receipt plumbing for llm-pipeline.

## Usage

```rust
use llm_tool_runtime::{ToolDefinition, ToolReceipt, ToolRegistry, ToolRuntime};
```

## What this crate is for

This crate defines the tool contract surface (definition, invocation, receipt),
a registry for tool lookup, dispatch logic, and runtime execution plumbing.
It is provider-agnostic and does not own domain truth.

## Non-goals

- do not let local convenience APIs become de facto truth surfaces
- prefer explicit versioned contracts over drift-by-example

## Ecosystem

**Depends on:**
- `stack-ids` -- identity primitives (`AttemptId`, `TrialId`, `TraceCtx`, `ContentDigest`, `ArtifactId`, `ScopeKey`)
- `semantic-memory-forge` -- export/evidence types

**Depended on by:**
- `verification-control`
- `verification-policy`
- `forge-engine` (living-memory)

## stack-ids integration

Uses `AttemptId`, `TrialId`, `TraceCtx`, `ContentDigest`, `DigestBuilder`,
`ArtifactId`, and `ScopeKey` from `stack-ids` for tool receipt identification,
retry lineage, and content-addressed result tracking.
