# FibQuant Interop

FibQuant is a separate crate and algorithm family. P24 does not merge FibQuant source into `turbo-quant`.

Shared interop names:

- `CodecProfileV1`
- `CompressionPolicyV1`
- `CompressionReceiptV1`
- `CompressionEvalV1`
- `BenchmarkReceiptV1`

Compatibility plan:

- Compare profile digest rules across crates.
- Keep benchmark receipt fields stable enough for ClaimLedger-style attachment.
- Define future governor traits outside both algorithm crates.
- Preserve canonical vectors and exact fallback in caller systems.

Known gaps:

- No sibling `fib-quant` source was modified.
- No cross-crate test was added because P24 is scoped to this crate.
- Digest compatibility is deterministic within this crate but not yet standardized as a cryptographic profile hash.
