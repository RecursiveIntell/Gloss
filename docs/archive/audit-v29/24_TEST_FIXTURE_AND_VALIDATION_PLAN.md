# Test Fixture and Validation Plan

## Required fixture corpus

- single text source with known unique facts;
- markdown/code source with anchors;
- folder with one valid, one unsupported, one failing file;
- source requiring semantic-memory projection;
- source producing citation anchors;
- long-generation provider stub that times out after partial tokens;
- provider settings matrix fixture;
- old-answer fixture with missing receipts;
- legacy notebook fixture requiring reconciliation.

## Required command families

```bash
npm run build
npm test
npm run check:sm-tq-profile
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant --all-targets -- -D warnings
python3 validation/gloss_release_candidate_gate.py --repo . --run-id CURRENT
```

## New validation scripts required

See `VALIDATION/validation_scripts_to_create.md`.
