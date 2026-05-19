# FibQuant interop plan

## Standing decision

`fib-quant` is already its own crate. Do not merge it into `turbo-quant`.

## Shared semantic layer

The crates should converge on compatible names and schema shapes:

- `CodecProfileV1`
- `CompressionPolicyV1`
- `CompressionReceiptV1`
- `CompressionEvalV1`
- `BenchmarkReceiptV1`
- `ProfileDigest`
- `source_digest`
- `codebook_digest`
- `storage_layout`
- `score_semantics`

## Ownership split

### turbo-quant owns

- TurboQuant_mse/prod-style codecs
- PolarQuant-style angle/radius encoding
- QJL residual sketching
- TurboQuant KV policy experiments
- bitpacked scalar/code paths

### fib-quant owns

- FibQuant radial/angular vector codebooks
- Beta-quantile radii
- Fibonacci/Roberts-Kronecker directions
- FibQuant-specific Lloyd-Max refinement
- fractional/sub-one-bit codebook semantics

### future quant-governor owns

- codec selection
- drift thresholds
- exact fallback
- corpus-specific promotion
- cross-codec benchmark comparison
- policy decisions

## P24 action

- Inspect `fib-quant` read-only if available.
- Align naming in docs and schemas.
- Do not add a path dependency on `fib-quant`.
- Add a future interop note for a common `quant-codec-core` crate only if justified.
