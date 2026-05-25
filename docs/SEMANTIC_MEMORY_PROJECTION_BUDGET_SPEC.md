# semantic-memory projection budget and Ollama context repair

## Problem

The live UI shows an Ollama HTTP 400 projection failure:

```text
semantic-memory manifest ingest failed: Ollama returned HTTP 400 Bad Request:
{"error":"the input length exceeds the context length"}
```

Current code only retries single chunks when a batch overflows. It does not split a single over-context chunk.

## External behavior to account for

Ollama's embed endpoint documents a `truncate` boolean and says false returns an error when context is exceeded. Current/observed behavior may still error for over-context embeddings in some versions/models. Therefore Gloss must not rely on server-side truncation. It must keep inputs under a local budget and preserve backpointers.

## Required implementation

Add deterministic projection subchunks:

```rust
struct ProjectionSubchunk {
    projection_chunk_id: String, // e.g. {gloss_chunk_id}::p{index}
    parent_gloss_chunk_id: String,
    subchunk_index: usize,
    byte_start: usize,
    byte_end: usize,
    content: String,
    content_digest: String,
}
```

Required budget defaults:

```text
max_projection_chars_per_item = 3000
max_projection_estimated_tokens_per_item = 768
max_projection_chars_per_batch = 8000
max_projection_chunks_per_batch = 4
```

Runtime settings may expose these values, but release defaults must be conservative.

## Required receipts

Each projection run emits:

```json
{
  "schema": "GlossSemanticMemoryProjectionReceiptV1",
  "receipt_id": "...",
  "notebook_id": "...",
  "source_id": "...",
  "embedding_model": "nomic-embed-text",
  "embedding_url": "http://...",
  "budget": {
    "max_chars_per_item": 3000,
    "max_estimated_tokens_per_item": 768,
    "max_chars_per_batch": 8000,
    "max_chunks_per_batch": 4
  },
  "gloss_chunks": 0,
  "projection_subchunks": 0,
  "projected_subchunks": 0,
  "failed_subchunks": 0,
  "context_length_failures": 0,
  "silent_truncation": false,
  "status": "synced|partial|failed"
}
```

## Acceptance

- No context-length failures in strict mode.
- If any chunk exceeds budget, it is split and backpointered.
- If split fails, source semantic projection status becomes failed/partial with exact error.
- No answer evidence claims semantic-memory contributed unless projection is synced/partial with actual candidates.
