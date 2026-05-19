# P24 test matrix

| Test file | Required checks |
|---|---|
| `tests/bitpack.rs` | 1..=16 roundtrip, crossing byte boundaries, invalid values, truncation |
| `tests/encoded_size.rs` | packed bytes match formulas, lower bits use fewer bytes |
| `tests/profile_receipt.rs` | digest stability, serde roundtrip, compression ratio math |
| `tests/invalid_inputs.rs` | NaN/Inf, zero dimensions, odd dims, bad bits, bad indexes |
| `tests/query_workspace.rs` | batch vs per-code score parity, workspace reuse |
| `tests/kv_policy.rs` | asymmetric K/V policy, exact fallback, index errors, shadow report |
| `tests/serialization.rs` | packed code serde roundtrip, profile/receipt schema stability |
| `tests/determinism.rs` | update for packed storage and digest determinism |
| `tests/inner_product.rs` | real accuracy trends, not just non-panic smoke |
