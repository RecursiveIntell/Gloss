# Dense indexing and TurboQuant release policy

The user explicitly wants dense indexing enabled and TurboQuant enabled in the build. Treat that as a release requirement, not an optional enhancement.

## Required build policy

`src-tauri/Cargo.toml` must end with one of these acceptable states:

Preferred:

```toml
[features]
default = ["semantic-memory-turbo-quant"]
semantic-memory-backend = ["dep:semantic-memory"]
semantic-memory-turbo-quant = [
    "semantic-memory-backend",
    "semantic-memory/turbo-quant-codec",
]
```

Also acceptable if default cannot change for packaging reasons:

```json
package.json scripts:
  "tauri:build:release": "tauri build --features semantic-memory-turbo-quant"
  "tauri:dev:release": "tauri dev --features semantic-memory-turbo-quant"
```

But final release commands must use `tauri:build:release` or `tauri:build:sm-tq`, not `tauri:build:sm`.

## Required runtime policy

- semantic-memory backend must compile: `cfg!(feature = "semantic-memory-backend") == true`.
- TurboQuant must compile: `cfg!(feature = "semantic-memory-turbo-quant") == true`.
- TurboQuant runtime flag must be enableable without hidden dependencies.
- semantic-memory Preview and TurboQuant Candidates can still be shown as guarded, but the release build must not say `Compiled TQ no`.
- If TQ is selected, retrieval must require exact rerank. Candidate-only acceleration is not answer authority.

## Dense indexing policy

Current code disables native dense indexing with `NATIVE_SEMANTIC_INDEXING_ENABLED=false`. P36 must remove that as a release default.

Acceptable final state:

```rust
pub const NATIVE_SEMANTIC_INDEXING_ENABLED: bool = true;
```

Better final state:

```rust
pub fn native_dense_indexing_enabled(app_db: &AppDb) -> Result<bool, GlossError> {
    Ok(cfg!(feature = "semantic-memory-backend") && setting_bool(app_db, "native_dense_indexing_enabled", true)?)
}
```

But the release default must be enabled. Emergency disable may exist as an explicit user/diagnostic setting, not a hidden build default.

## Folder import policy

Do not keep this final state:

```rust
IngestionOpts { embed_chunks: false, queue_summary: false, ... }
```

For large folder imports, dense indexing may be deferred to a bounded queue, but it must be queued and receipt-bearing:

```text
folder import → extract/chunk/FTS → dense-index queue → semantic-memory projection queue → summary queue
```

## Required proof commands

```bash
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets -- -D warnings
npm run tauri:build:sm-tq
python3 scripts/gloss_p36_static_gate.py --repo .
python3 scripts/gloss_dense_tq_release_gate.py --repo . --require-live-evidence
```
