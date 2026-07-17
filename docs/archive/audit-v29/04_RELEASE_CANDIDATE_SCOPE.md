# Release Candidate Scope

## Required RC features

- Local notebook/source ingestion for text/markdown/code/paste/folder.
- Chunking and source lifecycle correctness.
- BM25/FTS5 retrieval.
- Native FastEmbed dense indexing with `indexed_chunks > 0` live receipt.
- Hybrid native retrieval with evidence/citations.
- semantic-memory preview with FastEmbed provider and strict live proof.
- TurboQuant proof or explicit demotion.
- Per-answer runtime truth and retrieval decision.
- Per-answer prompt/generation/decoding receipts.
- Provider-aware decoding settings UI and storage.
- Increased generation timeout, partial save, continuation.
- Inspector Dock minimum: Notes, Prompt, Evidence, Receipt, Sources.
- Source Health panel/card.
- Current-run/package scope cleanup.
- Fresh release validation and desktop smoke receipt.

## Explicit RC non-goals

- Full PDF/DOCX/XLSX/PPTX/EPUB/URL/YouTube/audio/video ingestion.
- Studio flashcards/quizzes/timelines/mind maps/audio overview.
- Notebook export/import beyond minimal receipt-safe path.
- DB doctor full UI.
- Model bake-off dashboard.
- AICC artifact viewer.
- Public demo mode.

These can be planned in broad-spec phases but must not start until RC gates pass, unless a broad feature is required to prove RC behavior.
