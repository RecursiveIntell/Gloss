# Benchmarking

Use `examples/bench_embeddings.rs` to generate a machine-readable `BenchmarkReceiptV1`:

```bash
cargo run --example bench_embeddings --all-features -- \
  --dim 128 --db-size 512 --queries 16 --bits 4 \
  --projections 64 --seed 42 --top-k 10 \
  --out target/turbo-quant/p24-bench.json
```

By default the example uses `RotationKind::Auto`, which resolves to FastHadamard for power-of-two dimensions and adds a Stored QR comparison in the `comparisons` array. Use `--rotation stored` to benchmark the dense reference path directly, or `--rotation fast` to require FastHadamard and fail on unsupported dimensions.

The example uses synthetic standard-normal vectors and reports recall@k plus mean absolute score error against exact inner products. Treat the output as a reproducibility receipt only. Real promotion requires representative corpora, workload-specific recall/rank gates, and exact fallback.
