# Packaging, Release, and Public Claim Spec

## Package requirements

- archive root scoped to Gloss or explicit dependency snapshot;
- current-run sidecar matches active `CURRENT_RUN.md`;
- command receipts included;
- generated sidecars identified;
- old pass artifacts archived/quarantined;
- package report has 0 errors and reviewed warnings.

## Release requirements

- npm build/test pass;
- cargo fmt/check/test/clippy pass or failures are blockers;
- Tauri build release pass;
- fresh unzip replay pass;
- live desktop smoke pass;
- semantic-memory strict fixture pass;
- final receipt `release_ready=true` only after gates.

## Public claims allowed before RC

- local-first desktop notebook/RAG app under active validation;
- supports local/Ollama chat and source ingestion paths;
- includes evidence-oriented UI under development.

## Claims blocked until proof

- release-ready;
- semantic-memory-powered answers;
- TurboQuant-contributing retrieval;
- NotebookLM parity;
- production-ready;
- benchmark superiority.
