---
name: gloss-semantic-memory-promoter
description: "Use for Gloss semantic-memory adapter, exact backpointer, source scope, and promotion work."
---


# Gloss semantic-memory promoter

Use this skill when modifying or reviewing Gloss semantic-memory integration.

Checklist:

1. Find canonical owner before editing. Gloss must not implement semantic-memory internals.
2. Preserve exact `source_id`, `chunk_id`, `content_digest`, `sm_document_id`, `sm_chunk_id` mapping.
3. `sync_status='synced'` requires exact non-null backpointers.
4. Unmapped/degraded/stale candidates cannot become citations.
5. Explicit source scope must not widen silently.
6. Search/evidence payloads must disclose backend, fallback, degradation, and receipt id.

Run:

```bash
python3 scripts/sm_tq_static_validator.py --repo .
```
