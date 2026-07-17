# Baseline gates

Audit tree: branch perf-slowdown-fix-20260610 at 93864835b21a52d4e504cb97c650d90abfc2f082

PASS:
- npm run build — Vite build completed; main JS chunk 654.39 kB with size warning.
- npm test — 12 static source-text contract checks.
- npm run test:unit — 3 discovered files, 16 tests.
- bash validation/run_all_gloss_repair_gates.sh . — five static gates.

FAIL:
- cargo fmt --all -- --check — malformed escaped Rust source at src-tauri/src/commands/studio.rs:612.
- cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant — 38 parser errors rooted at studio.rs:612.
- npm audit — one high and two low development-chain advisories.
- cargo deny check advisories — actionable Rust advisories.

NOT RUN / NOT PROVEN:
- full Rust tests (blocked by parser failure)
- Tauri package build
- desktop smoke
- clean-clone dependency resolution
