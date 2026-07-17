# Dependency and Provider Decision Ledger

## Provider/runtime settings

| Provider | Current recommendation | Supported settings policy | Notes |
|---|---|---|---|
| Ollama | Primary local chat provider | Use `options` for supported runtime settings; capture exact effective values | Docs show `/api/generate`/chat streaming, `options`, `temperature`, `stream`, `keep_alive`; verify `top_k/top_p/min_p/typical_p` against current Modelfile/API docs before shipping. |
| OpenAI | Optional cloud provider | Expose only officially supported fields; likely `temperature`, `top_p`, `max_output_tokens`; mark others unsupported/opaque | Recheck current Responses API docs before implementation. |
| Anthropic | Optional cloud provider | Expose only current supported fields; do not guess top-k/top-p behavior | Recheck current Messages API docs before implementation. |
| llama.cpp/OpenAI-compatible | Optional future provider | Capability map must be endpoint-specific | Do not assume OpenAI-compatible means identical settings. |

## Retrieval/storage

| Need | Recommendation | Reason | Gate |
|---|---|---|---|
| Full-text search | SQLite FTS5 via rusqlite bundled/vtab | Already current design; local-first and portable | FTS fixture |
| Dense embeddings | FastEmbed default | Avoid Ollama embedding/network split-brain | FastEmbed init/embed-one + semantic-memory store smoke |
| HNSW | usearch current path | Existing dependency; portable vector index | reload/search fixture |
| semantic-memory | canonical `Libraries/semantic-memory` | Avoid Gloss-local semantics fork | strict semantic-memory answer fixture |
| TurboQuant | only where measured | Prevent compile-only claim | exact rerank/artifact receipt |

## Broad ingestion dependencies — defer until RC gate passes

| Format | Candidate | Policy |
|---|---|---|
| PDF | `pdf-extract`/`lopdf`/Poppler wrapper after fixture evaluation | Need text spans and failure receipts. |
| DOCX | ZIP + XML parser or vetted crate | Preserve paragraphs/tables metadata. |
| XLSX | `calamine` | Turn sheets/ranges into table-aware chunks. |
| CSV | Rust `csv` crate | Table-aware chunking; dialect receipt. |
| PPTX | ZIP + XML parser | Slide/text order and speaker notes receipt. |
| EPUB | EPUB crate + HTML normalizer | Chapter/spine metadata. |
| HTML/URL | reqwest + scraper/readability candidate | Network opt-in + fetch receipt. |
| YouTube transcript | ytextract/API/manual transcript TBD | Network policy and source provenance required. |
| Audio transcription | whisper-rs + ffmpeg | Background job + transcript receipt. |
| TTS/audio overview | Piper + ffmpeg | Deferred Studio feature, output artifact receipt. |
