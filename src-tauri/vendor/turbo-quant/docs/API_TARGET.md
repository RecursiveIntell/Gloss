# P24 API target

## New public types

- `CodecKind`
- `RotationKind`
- `StorageLayout`
- `ScoreSemantics`
- `CodebookKind`
- `CodecProfileV1`
- `CompressionPolicyV1`
- `CompressionReceiptV1`
- `CompressionEvalV1`
- `BenchmarkReceiptV1`
- `TurboMode`
- `KvQuantPolicy`
- `KvCompressionReceipt`
- `KvShadowReport`

## New modules

- `bitpack`
- `profile` or `codec`
- `codebook`
- `eval`

## Compatibility

Old constructors may remain as deprecated wrappers if feasible:

- `PolarQuantizer::new(dim, bits, seed)`
- `QjlQuantizer::new(dim, projections, seed)`
- `TurboQuantizer::new(dim, bits, projections, seed)`

If behavior changes, document in `CHANGELOG.md`.

## Default-off advanced behavior

- QJL residual correction should be explicit.
- KV-cache compression policies should be explicit.
- semantic-memory/Gloss integration should be via examples/docs only unless crates are present.
