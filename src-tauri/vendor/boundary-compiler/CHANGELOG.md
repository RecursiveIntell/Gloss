# Changelog

All notable changes to `boundary-compiler` are documented here.

## [Unreleased]

## [0.1.0] — 2026-06-02

Initial crates.io release.

### Added

- `Canonicalizer::canonicalize(&Value) -> Result<String, JcsError>` — full
  RFC 8785 JCS canonicalization: sorted object keys, RFC 8785 number
  formatting, UTF-8 escape rules, no insignificant whitespace.
- `parse_with_dup_check(&str) -> Result<Value, JcsError>` — strict JSON
  parser that rejects duplicate object keys (RFC 8785 §3.2.2.2).
- `parse_and_validate(&str) -> Result<Value, JcsError>` — parse + duplicate
  detection in one call.
- `canonicalize_flexible(&Value) -> Result<String, JcsError>` — convenience
  for "value already in memory, just canonicalize."
- `ContentDigest::compute(&Value) -> Result<ContentDigest, JcsError>` —
  blake3 hash of the JCS bytes. Wrap a `Hash` for downstream receipt
  construction.
- `BoundaryProfile` — a struct that bundles:
  - `Dialect` (JSON, StrictJSON, custom)
  - `CanonicalizationProfile` (RFC8785, custom)
  - `UnknownFieldPolicy` (Reject, Strip, Accept)
  - `ResourceCeilings` (max depth, max key count, max value size)
  - schema id + version strings
- `SchemaValidator` — JSON Schema validation hook (currently a pass-through
  with the validation result struct in place; real validator wires in via
  a future feature flag).

### Doctests

- 5 doctests in the lib.rs doc-comment (one per public API).
- All pass under `cargo test --doc`.

### Lints

- Inherits workspace `[lints]` policy: `unsafe_code = "deny"`, plus the
  workspace `clippy::expect_used = "warn"` / `clippy::unwrap_used = "warn"`.
- `cargo clippy --all-targets -- -D warnings` clean.

### Test coverage

- 27 integration tests in `tests/` covering: canonical key sorting,
  duplicate-key rejection, RFC 8785 number formatting edge cases
  (integers, negative, scientific notation, large), string escaping,
  unicode handling, and round-trip determinism.
- `cargo test` clean.

### Notes

- This crate was extracted from `semantic_memory::graph::canonical_json_string`
  in the V30 audit. The `semantic-memory` crate depends on this one for
  JCS canonicalization across the entire memory store.

[Unreleased]: https://github.com/recursiveintell/boundary-compiler/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/recursiveintell/boundary-compiler/releases/tag/v0.1.0
