# Current Feature Matrix — Gloss

Status values: implemented, partial, degraded, deferred, removed.

| Feature | Status | Evidence | Validation | Notes |
|---|---|---|---|---|
| Text/Markdown/code folder import | partial | current source import path | runtime import smoke | fix stale notebook race first |
| PDF ingestion | deferred | BINARY_EXTENSIONS excludes pdf | feature matrix gate | implement later or keep disabled |
| DOCX ingestion | deferred | BINARY_EXTENSIONS excludes docx | feature matrix gate | implement later |
| XLSX ingestion | deferred | BINARY_EXTENSIONS excludes xlsx | feature matrix gate | implement later |
| URL import | deferred | no URL pipeline proven | feature matrix gate | requires network policy |
| YouTube transcript import | deferred | no pipeline proven | feature matrix gate | requires URL/transcript policy |
| Audio transcription | deferred | no whisper pipeline proven | feature matrix gate | future |
| Audio overview/TTS | deferred | no piper pipeline proven | feature matrix gate | future |
| Local BM25 RAG | partial | runtime BM25-only evidence | citation anchor tests | fix citation anchors |
| Native dense retrieval | degraded | native indexing disabled in runtime log | retrieval capability tests | require explicit setting/status |
| semantic-memory preview | degraded | projection context-length failure | projection batching tests | fix batching |
| TurboQuant | unproven | no runtime proof in screenshot/log | TQ proof tests | do not overclaim |
| Studio reports | deferred | studio stub | feature matrix gate | future or hide |
| Studio flashcards/quizzes | deferred | studio stub | feature matrix gate | future or hide |
| Notebook export/import | deferred | not proven | export/import tests | future |
| Desktop smoke | missing | desktop-smoke points to archived script | npm run desktop-smoke | implement current harness |
| DB doctor | missing | stale notebook/source risk | db doctor tests | implement P6 |
