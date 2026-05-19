---
name: p32r3-ingestion-performance
description: Use for Gloss file/folder ingestion, NotebookDb transaction, batch creation/delete, deterministic traversal.
---

Prefer batch commands and transactions over per-source/per-chunk writes. Folder traversal must sort entries before applying caps. UI must receive explicit events for scan started/failed/empty/truncated/completed.
