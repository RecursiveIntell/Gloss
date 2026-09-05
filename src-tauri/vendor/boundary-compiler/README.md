# boundary-compiler

<!-- last-verified: 2026-08-20 -->

RFC 8785 JSON Canonicalization Scheme (JCS) for Rust, with strict duplicate-key rejection, executable resource ceilings, and content-digest metadata.

![JSON input flows through duplicate-key checks and resource budgets into RFC 8785 canonical bytes, a digest, and an enforcement receipt.](https://raw.githubusercontent.com/RecursiveIntell/boundary-compiler/main/docs/architecture.svg)

`boundary-compiler` is a small boundary-admission layer. It turns JSON input into deterministic RFC 8785 bytes only after duplicate-key and resource-budget checks pass. The result can carry a `ContentDigest` and a receipt describing which rules were enforced.

## What it provides

- **RFC 8785 canonicalization** through `Canonicalizer`.
- **Strict parsing** through `parse_with_dup_check` and `parse_and_validate`; duplicate object keys fail instead of using “last value wins” behavior.
- **Content binding** through `ContentDigest`, computed from canonical JSON bytes.
- **Executable admission profiles** through `BoundaryProfile`, with input-byte, node, depth, object-key, string-byte, and array-length ceilings.
- **Enforcement receipts** through `BoundaryAdmission::receipt`, including the canonicalization profile and applied rules.
- **Fail-closed schema behavior** through `SchemaValidator` when no schema engine is configured in this base crate.

## Scope and limits

This crate supports one canonicalization contract: RFC 8785. It does **not** bundle a JSON Schema engine, choose an unknown-field policy, or claim that canonicalization alone validates a domain schema. Applications that need schema identity or field admission should run a configured schema layer before treating a value as domain-valid.

The resource profile is a boundary guard, not a security certification. Select ceilings for the workload, keep normal error handling, and retain the receipt if downstream systems need to explain an admission decision.

## Installation

```toml
[dependencies]
boundary-compiler = "0.1.1"
```

Rust 1.75 or newer is required by the package manifest.

## Quick start

```rust
use boundary_compiler::{BoundaryProfile, ResourceCeilings};

fn main() -> Result<(), boundary_compiler::JcsError> {
    let profile = BoundaryProfile::new(ResourceCeilings {
        max_input_bytes: 1 << 20,
        max_nodes: 100_000,
        max_depth: 32,
        max_object_keys: 128,
        max_string_bytes: 1 << 20,
        max_array_len: 1024,
    });

    let admitted = profile.parse(r#"{"b":2,"a":1}"#)?;
    assert_eq!(admitted.canonical_bytes, br#"{"a":1,"b":2}"#);
    assert_eq!(admitted.receipt.canonicalization_profile, "rfc8785");
    Ok(())
}
```

For an already parsed `serde_json::Value`, use `Canonicalizer` or `canonicalize_flexible` directly. Use `ContentDigest::compute` when you need a digest for canonicalized content.

## API map

| API | Purpose |
|---|---|
| `Canonicalizer` | Serialize a `serde_json::Value` into deterministic RFC 8785 JSON. |
| `parse_with_dup_check` | Parse JSON while rejecting decoded duplicate object keys. |
| `parse_and_validate` | Convenience parse-and-check entry point. |
| `canonicalize_flexible` | Canonicalize an in-memory JSON value. |
| `ContentDigest` | Bind a value to its canonical representation with a content digest. |
| `BoundaryProfile::new` / `rfc8785` | Construct an explicit or default admission profile. |
| `BoundaryProfile::parse` | Check input budgets, reject duplicates, canonicalize, and return an admission receipt. |
| `BoundaryProfile::check_resources` | Check a parsed value against structural budgets. |
| `BoundaryEnforcementReceipt` | Record the canonicalization profile and enforced rules. |
| `SchemaValidator` | Fail-closed schema boundary when no validator is configured. |
| `JcsError` | Typed errors for parse, duplicate-key, digest, schema, profile, and resource failures. |

## Resource ceilings

`ResourceCeilings` defaults to the following limits:

| Rule | Default |
|---|---:|
| `max_input_bytes` | 1 MiB |
| `max_nodes` | 100,000 |
| `max_depth` | 32 |
| `max_object_keys` | 128 |
| `max_string_bytes` | 1 MiB |
| `max_array_len` | 1,024 |

Provide a tighter or wider profile explicitly when those defaults do not match the surrounding service. A limit violation returns `JcsError::ResourceCeilingExceeded` before the admission succeeds.

## Error behavior

Fallible operations return `Result<_, JcsError>`. Important variants include:

- `DuplicateKey { key }` — a decoded object key appeared more than once.
- `ParseError(_)` — the input is not valid JSON.
- `InvalidJson { reason }` — a value cannot be canonicalized under the supported contract.
- `ResourceCeilingExceeded { resource, used, limit }` — an admission budget was exceeded.
- `SchemaValidation(_)` / `SchemaError(_)` — schema admission failed or is not configured.
- `DigestError(_)` — content-digest construction failed.

## Verification

The release-source checkout was verified on 2026-08-20 with:

```bash
cargo fmt --check
cargo check --all-targets
cargo test --all-targets
cargo clippy --all-targets -- -D warnings
```

The test suite covers RFC 8785 vectors, duplicate keys, number and string handling, profile enforcement, resource ceilings, digest behavior, and fail-closed schema admission. Run the commands above in your checkout for current evidence; this README does not substitute for a local gate.

## Integration path

`boundary-compiler` can sit between an untrusted JSON boundary and a receipt, persistence, or signing layer:

1. Parse with duplicate-key rejection.
2. Apply a workload-specific `BoundaryProfile`.
3. Persist or transmit `canonical_bytes` rather than an arbitrary serialization.
4. Carry `ContentDigest` and the enforcement receipt with the downstream record.
5. Apply domain schema validation separately when the application has a schema engine.

The crate is used as a foundational boundary primitive in the RecursiveIntell stack, including [`semantic-memory`](https://github.com/RecursiveIntell/semantic-memory). The links are absolute so they remain usable on crates.io as well as GitHub.

## Repository and release artifacts

- Source: <https://github.com/RecursiveIntell/boundary-compiler>
- API documentation: <https://docs.rs/boundary-compiler>
- Changelog: [`CHANGELOG.md`](https://github.com/RecursiveIntell/boundary-compiler/blob/main/CHANGELOG.md)
- License: [`LICENSE-APACHE`](https://github.com/RecursiveIntell/boundary-compiler/blob/main/LICENSE-APACHE) and [`LICENSE-MIT`](https://github.com/RecursiveIntell/boundary-compiler/blob/main/LICENSE-MIT)

## License

Apache-2.0. The repository also carries the MIT text for downstream compatibility; consult the checked-in license files for the governing terms.
