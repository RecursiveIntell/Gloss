---
name: turbo-quant-gatekeeper
description: "Use for semantic-memory TurboQuant candidate acceleration, exact rerank, vector artifact lifecycle, and fallback receipts."
---


# TurboQuant gatekeeper

Use when enabling or reviewing TurboQuant.

Rules:

1. TurboQuant is candidate generation only.
2. Exact f32 rerank is mandatory for promoted results.
3. Search receipts must disclose candidate backend, generation id, approximate count, exact rerank count, and fallback reason.
4. Missing/stale/corrupt vector artifacts must not answer silently.
5. Rebuild/invalidate vector artifacts on ingest, delete, and reindex.

Commands:

```bash
(cd ../Libraries/semantic-memory && cargo test --features turbo-quant-codec)
(cd src-tauri && cargo test --features semantic-memory-turbo-quant)
```
