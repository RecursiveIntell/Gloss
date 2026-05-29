# Broad Spec Scope and Non-Goals

## Broad scope after RC

Gloss full feature completion may include:

- multi-format ingestion: PDF, DOCX, XLSX/CSV, PPTX, EPUB, HTML, URL, YouTube transcript, audio/video metadata, audio transcription, optional OCR/vision;
- Studio outputs: summaries, reports, flashcards, quizzes, timelines, mind maps, saved outputs, templates;
- evidence export, notebook export/import, DB doctor/repair, migration smoke;
- packaging/installers and first-run model/cache workflows;
- performance hardening for 2k+ sources and 10k+ chunks;
- public README/demo assets once proof packets exist.

## Non-goals unless explicitly re-scoped

- Reimplementing canonical semantic-memory or TurboQuant semantics inside Gloss.
- Treating vector index as durable truth.
- Treating a generated summary as source truth.
- Claiming NotebookLM parity without feature and receipt proof.
- Shipping cloud provider behavior without opt-in and provider-route receipts.
- Adding comparison dashboards without fair benchmark receipts.
