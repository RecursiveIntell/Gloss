# Observed evidence basis

This pass was designed from:

- current uploaded Gloss package sidecars;
- extracted current Gloss source package;
- screenshots showing live Ollama chat, source counts, semantic-memory failure, dense/TurboQuant status, and projection/context-length errors;
- current code inspection of `src-tauri/Cargo.toml`, `src-tauri/src/state.rs`, `src-tauri/src/features.rs`, `src-tauri/src/commands/sources/mod.rs`, `src-tauri/src/memory/semantic_memory_adapter.rs`, `src/components/chat/ChatPanel.tsx`, `src/components/layout/StatusBar.tsx`, `src/components/sources/SourcesPanel.tsx`, and DB migrations.

## Critical observed current-state facts from source inspection

```text
src-tauri/Cargo.toml:
  default = ["semantic-memory-backend"]
  semantic-memory-turbo-quant exists but is not default.

src-tauri/src/state.rs:
  NATIVE_SEMANTIC_INDEXING_ENABLED: bool = false.

src-tauri/src/commands/sources/mod.rs:
  IngestionOpts::default().embed_chunks = NATIVE_SEMANTIC_INDEXING_ENABLED.
  Folder import overrides embed_chunks=false and queue_summary=false.
  semantic-memory projection emits source status values such as semantic_memory_projecting/synced/error.

src/components/chat/ChatPanel.tsx:
  readiness uses source.status !== "ready".
  unindexed count uses source.status === "pending".

src/components/sources/SourcesPanel.tsx:
  one JSX text node renders literal · no summary.

src-tauri/src/memory/semantic_memory_adapter.rs:
  projection batches allow 12 chunks / 24k chars / 6k estimated tokens and only retry single chunks on batch context failure.
  no deterministic projection subchunk path is present for a single over-context chunk.
```

## Screenshot-derived current live failures

```text
570 sources loaded.
570/570 selected.
570 not ready.
410 unindexed.
semantic_memory_synced appears on source rows.
Memory: semantic-memory-preview (failed).
semantic-memory manifest ingest failed: Ollama returned HTTP 400 Bad Request: input length exceeds context length.
Embedding/index failed in Settings despite 3712/3731 semantic links synced.
```
