# Gloss

Gloss is a local-first desktop notebook for importing source files, indexing them locally, asking scoped questions, and saving cited answers into notes.

The app is built with Tauri, React, TypeScript, and Rust. Its release-proof contract is conservative: public claims in this repository must match source, validation gates, and live receipts under `docs/codex-runs/`.

## Current Scope

- Local notebook and source management.
- File import with local indexing.
- Source-grounded chat with cited evidence.
- Local dense retrieval as the default retrieval path.
- Semantic-memory and TurboQuant paths only when enabled and proven by receipts for the active run.

## Release Proof

The active Codex repair run is:

```text
GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525
```

Run receipts and validation evidence belong under:

```text
docs/codex-runs/GLOSS_RELEASE_PROOF_EMBEDDING_UNIFICATION_CLEANUP_20260525/
```

`README.md` is product-facing documentation, not release proof. Use the active run receipt and validation gates for release decisions.

## Development

```bash
npm install
npm run build
npm run tauri:dev
```

Release-profile checks are defined by the active run instructions in `AGENTS.md` and the current run receipts.
