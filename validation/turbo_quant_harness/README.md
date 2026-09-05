# TurboQuant runtime acceptance gate

Run from the Gloss repository root:

```bash
cargo test --locked --manifest-path validation/turbo_quant_harness/Cargo.toml
```

This compiles registry `semantic-memory =0.5.15` with its default vector backend and `turbo-quant-codec`, including locked `turbo-quant 0.2.3`. The build fails if any registry package in the gate differs in version, source or checksum from the root application lock. The gate imports the actual Gloss proof owner and canonical notebook DB modules. It does not run the historical vendored codec benchmark.

Disposable `MockEmbedder` vectors exercise manifest ingestion, actual artifact build, restart/reopen, namespace and chunk filtering, missing generations, stale source snapshots, corrupt artifacts, explicit raw-f32 fallback, strict receipt rejection and rebuild recovery. The build/reopen/filter canary uses Gloss's 768-dimensional native profile. Receipt fault tests use 64 dimensions to keep repeated debug codec construction bounded. Generation binding and canonical mutation invalidation cover Gloss's acceptance boundary.

This is a runtime correctness canary, not a performance or retrieval-quality benchmark. It does not prove model quality, actual Ollama/Candle embedding execution, Tauri IPC, desktop usability, large-corpus recall, latency, memory savings, or superiority over exact search. Those remain separate measured acceptance gates.
