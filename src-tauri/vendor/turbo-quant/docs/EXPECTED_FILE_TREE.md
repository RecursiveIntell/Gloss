# Expected final file tree delta

Expected additions or modifications:

```text
Cargo.toml
README.md
CHANGELOG.md
RELEASE_NOTES.md
src/error.rs
src/lib.rs
src/bitpack.rs
src/profile.rs
src/codebook.rs
src/eval.rs
src/polar.rs
src/qjl.rs
src/rotation.rs
src/turbo.rs
src/kv.rs
examples/profile_receipt.rs
examples/bench_embeddings.rs
examples/kv_shadow.rs
tests/bitpack.rs
tests/encoded_size.rs
tests/profile_receipt.rs
tests/invalid_inputs.rs
tests/query_workspace.rs
tests/kv_policy.rs
tests/serialization.rs
tests/determinism.rs
tests/inner_product.rs
docs/RESEARCH_ALIGNMENT.md
docs/BENCHMARKING.md
docs/COMPATIBILITY.md
docs/FIB_QUANT_INTEROP.md
docs/SEMANTIC_MEMORY_GLOSS_SHADOW_MODE.md
docs/RELEASE_GATE.md
docs/codex-runs/P24/*
```

Forbidden leftovers:

```text
unqualified zero-loss claims
production KV-cache claims
silent compatibility shims
generated benchmark claims without JSON receipts
path dependency on fib-quant unless explicitly approved
```
