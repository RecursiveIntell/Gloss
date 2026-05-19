# turbo-quant

`turbo-quant` is an experimental Rust codec substrate for TurboQuant-family vector-compression work. It provides deterministic PolarQuant, optional QJL residual sketches, packed storage, profiles, and receipts for benchmarkable sidecar use.

This crate does not publish or validate deployed KV-cache runtime behavior. It does not replace canonical `f32` vectors, and lossy compressed retrieval should be used only as a derived sidecar with exact fallback and workload-specific benchmark gates.

## What This Crate Verifies

- Polar angle indices are bitpacked in `PolarCode::packed_angle_indices`.
- QJL signs are bitpacked in `QjlSketch::packed_signs` using `0 => -1`, `1 => +1`.
- `encoded_bytes()` reports the packed serialized payload bytes for the code object.
- `CodecProfileV1`, `CompressionPolicyV1`, and `CompressionReceiptV1` are exported.
- `TurboMode::PolarOnly` can disable QJL; QJL use is explicit.
- `RotationKind::Auto` uses FastHadamard for power-of-two dimensions and Stored QR for other supported dimensions.
- `RotationKind::StoredQr` remains available when the dense reference backend is needed.
- `KvQuantPolicy` supports asymmetric key/value policies plus exact shadow comparison.
- Benchmark receipts can be generated with `examples/bench_embeddings.rs`.

## Research Context

TurboQuant-style designs combine data-oblivious rotations, polar/scalar quantization, and residual sketching for approximate inner-product scoring. Those are algorithm-family ideas, not claims that this crate has proven deployment quality for any production retrieval or attention stack.

FibQuant is a separate crate and algorithm family. Interop should happen through shared profile, receipt, policy, and benchmark schemas rather than source merging.

## Quick Start

```rust
use turbo_quant::{TurboMode, TurboQuantizer};

# fn main() -> Result<(), turbo_quant::TurboQuantError> {
let dim = 64;
let quantizer = TurboQuantizer::new_with_mode(
    dim,
    8,
    32,
    42,
    TurboMode::PolarWithQjl,
)?;

let vector = vec![0.1; dim];
let query = vec![0.2; dim];
let (code, receipt) = quantizer.encode_with_receipt(&vector, Some("source:example".into()))?;
let score = quantizer.inner_product_estimate(&code, &query)?;

assert!(score.is_finite());
assert_eq!(receipt.encoded_bytes, code.encoded_bytes());
# Ok(())
# }
```

For QJL-free scoring:

```rust
use turbo_quant::{TurboMode, TurboQuantizer};

# fn main() -> Result<(), turbo_quant::TurboQuantError> {
let quantizer = TurboQuantizer::new_with_mode(64, 8, 0, 42, TurboMode::PolarOnly)?;
# Ok(())
# }
```

For an explicit dense QR reference rotation:

```rust
use turbo_quant::TurboQuantizer;

# fn main() -> Result<(), turbo_quant::TurboQuantError> {
let quantizer = TurboQuantizer::new_with_stored_rotation(64, 8, 32, 42)?;
assert_eq!(quantizer.profile().rotation_kind, "stored_qr_reference");
# Ok(())
# }
```

## Examples

```bash
cargo run --example profile_receipt --all-features
cargo run --example bench_embeddings --all-features -- \
  --dim 128 --db-size 512 --queries 16 --bits 4 \
  --projections 64 --seed 42 --top-k 10 \
  --out target/turbo-quant/p24-bench.json
cargo run --example kv_shadow --all-features
```

`bench_embeddings` writes a `BenchmarkReceiptV1` JSON file with synthetic recall and error metrics. Treat it as a reproducibility receipt, not as deployment evidence.

## Storage Notes

Packed code payloads are derived artifacts:

- Polar radii remain `f32`.
- Polar angle indices are packed at the configured bit width.
- QJL residual signs are packed one bit per projection.
- Wire artifacts include headers in addition to the code payload.

Canonical vectors and source evidence remain owned by the caller.

## Release Status

Current target: `0.2.0-alpha.1`.

Recommended use: local experiments, benchmarks, receipts, and sidecar/shadow-mode integration work. Do not publish or promote a production claim without the validation receipts required by your downstream system.
