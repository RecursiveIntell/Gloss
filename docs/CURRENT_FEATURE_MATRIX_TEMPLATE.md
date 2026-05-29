# Current Feature Matrix — Gloss

Status values: implemented, partial, degraded, deferred, removed.

| Feature | Status | Evidence | Validation | Notes |
|---|---|---|---|---|
| Text/Markdown/code folder import | partial | current source import path | runtime import smoke | fix stale notebook race first |
| PDF ingestion | degraded | bounded pure-Rust PDF text extraction | document extractor gate | OCR/forms/layout fidelity remain unsupported |
| DOC/DOCX ingestion | degraded | bounded DOCX OOXML text extraction plus legacy .doc antiword CLI extraction | document and legacy Office extractor gates | layout/rendering fidelity remains unsupported |
| XLS/XLSX ingestion | degraded | bounded XLSX OOXML shared strings/worksheet values plus legacy .xls xls2csv CLI extraction | document and legacy Office extractor gates | formulas/charts/layout fidelity remains unsupported |
| PPT/PPTX ingestion | degraded | bounded PPTX OOXML slide text plus legacy .ppt catppt CLI extraction | document and legacy Office extractor gates | speaker notes/layout fidelity remains unsupported |
| EPUB ingestion | degraded | bounded EPUB spine XHTML extraction | document extractor gate | DRM/readability fidelity remain unsupported |
| URL import | degraded | one-shot user-consented HTTP(S) text fetch with strict network and byte limits | URL import gate | no crawling/authenticated fetch/readability claim |
| YouTube transcript import | deferred | no pipeline proven | feature matrix gate | requires URL/transcript policy |
| Audio transcription | partial | ffprobe metadata route and optional cached Whisper CLI transcription proven | audio metadata/transcription gates | diarization, speaker labels, automatic model download, and long-audio certification remain future |
| Audio overview/TTS | deferred | no piper pipeline proven | feature matrix gate | future |
| Local BM25 RAG | partial | runtime BM25-only evidence | citation anchor tests | fix citation anchors |
| Native dense retrieval | degraded | native indexing disabled in runtime log | retrieval capability tests | require explicit setting/status |
| semantic-memory preview | degraded | projection context-length failure | projection batching tests | fix batching |
| TurboQuant | unproven | no runtime proof in screenshot/log | TQ proof tests | do not overclaim |
| Studio reports | partial | deterministic source-cited backend artifacts plus Inspector Dock Studio panel/export | studio artifacts gate | model-generated workflow remains future |
| Studio flashcards/quizzes | partial | deterministic source-cited flashcard/quiz artifacts plus Inspector Dock Studio panel/export | studio artifacts gate | model-generated workflow remains future |
| Notebook export/import | deferred | not proven | export/import tests | future |
| Desktop smoke | partial | current harness emits capability-detected non-release receipt; no active automated live GUI driver | npm run desktop-smoke; python3 validation/gloss_desktop_smoke_gate.py --repo . | add live GUI driver and release-grade receipt |
| Installer package smoke | partial | deb/rpm payload and isolated installed-launch receipt; post-launch workflow and AppImage unproven | npm run installer-smoke | add installed workflow smoke and AppImage if tooling is available |
| DB doctor | missing | stale notebook/source risk | db doctor tests | implement P6 |
