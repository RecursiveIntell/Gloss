---
name: gloss-runtime-smoke
description: "Use for manual or scripted live Gloss/Ollama runtime smoke validation after semantic-memory and TurboQuant changes."
---


# Gloss runtime smoke

Use for live app validation.

Minimum smoke:

1. Launch Gloss.
2. Import text and code sources.
3. Sync to semantic-memory.
4. Confirm synced links have exact sm document/chunk ids.
5. Rebuild TurboQuant artifacts.
6. Ask scoped question.
7. Verify evidence drawer shows source/chunk ids, sm document/chunk ids, generation id, exact rerank, and fallback state.
8. Delete source and verify it cannot answer.
9. Restart app and verify status persists.

Skipped live smoke blocks release readiness.
