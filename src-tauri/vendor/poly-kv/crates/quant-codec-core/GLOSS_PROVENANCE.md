# Dependency restoration

Restored unchanged from RecursiveIntell/Libraries
`011027f77fc7a53c6ecb05300d5c34144370fa3b`, path
`poly-kv/crates/quant-codec-core`, on 2026-09-05 (package 0.1.0-alpha.1).
The vendored poly-kv workspace already declared this missing member and
fib-quant's optional compat feature already referenced its path and traits.
This does not enable PolyKV/FibQuant in Gloss or certify their compatibility.
The upstream manifest declares MIT OR Apache-2.0.
