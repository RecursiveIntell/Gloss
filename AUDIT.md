# Gloss Codebase Audit

**Status Summary**: 185 tests pass, validation gates pass. IndexChunks implemented, Timeline/DataTable renderers created. Phase 4 outputs blocked by ort crate conflict.

## MISSING - Phase 2 (Spec §6.1)

- [x] `TimelineView` renderer component - created TimelineView.tsx
- [x] `DataTableView` renderer component - created DataTableView.tsx

## MISSING - Phase 4 (Spec §6.1, §6.4)

- [ ] `SlideViewer` renderer - slide_deck output type has no backend or UI
- [ ] `InfographicView` renderer - infographic output type has no backend or UI
- [ ] `AudioPlayer` component - audio overview output types missing from backend
- [ ] TTS dependencies blocked by ort crate conflict with fastembed

## MISSING - Studio Features

- [ ] Briefing Doc generation UI
- [ ] Study Guide generation UI
- [ ] Custom Report generation UI
- [ ] QuizWidget "Explain" button (extra LLM call per question)

## STUB/NOP - Job Queue

- [x] `IndexChunks` job - IMPLEMENTED (embed chunks + add to HNSW)
- [ ] Template directory missing (`/home/sikmindz/Coding/Gloss/templates/`)
- [ ] `src-tauri/src/studio/mod.rs` uses hardcoded templates (SPEC wants TOML)

## MISSING - Retrieval Features

- [ ] Multi-angle query rewriting (SPEC §7.1: "LLM generates 2 rephrased queries")

---

## VERIFIED - SPEC Requirements

### FlashcardWidget
- [x] Difficulty levels (easy/medium/hard) - FlashcardWidget.tsx:49-59
- [x] "Known/Review" buttons - FlashcardWidget.tsx:225-244

### Database Schema
- [x] Migrations: app=3, notebook=7
- [x] Core tables: sources, chunks, conversations, messages, chat_attempts, notes, studio_outputs
- [x] Semantic memory: semantic_memory_links, projection_status, embedding_index_metadata
- [x] FTS5 indexes with triggers

### Job Queue
- [x] SummarizeSource - implemented
- [x] DescribeImage - implemented
- [x] DescribeVideo - implemented
- [x] ExtractAudioMetadata - implemented (whisper CLI, ffprobe)

### Import/Export
- [x] export_notebook_package/archive - implemented
- [x] import_notebook_package/archive - implemented
- [x] validate_notebook_package/archive - implemented

### Receipt Types
- [x] ChatAttemptTraceV1, BatchReceiptV1 - chat/receipts.rs
- [x] StudioOutputView, StudioExportReceipt - studio.rs
- [x] NotebookExportReceipt, NotebookImportReceipt - db/portable.rs

### Key Features
- [x] Provider registry: Ollama, OpenAI, Anthropic, LlamaCpp
- [x] Chunking/embedding pipeline
- [x] HNSW vector search + RRF fusion
- [x] Chat streaming with event traces
- [x] Studio outputs: Report, Summary, Outline, FAQ, Flashcards, Quiz, MindMap, CompareTable, ActionPlan
- [x] All 185 tests pass

## Cargo Build Warnings (semantic-memory library)

- FALSE POSITIVE: `DigestBuilder` is used in graph_edges.rs:382
- `mut rows` warning at graph.rs:1026 - clippy suggestion ignored (semantic would change)
- dead_code: internal library functions not used by Gloss
- unused field: `late_interaction_score` in search.rs:227 - internal to semantic-memory

## Cargo.toml Patch Applied

Removed `semantic-memory/poly-kv-pool` from semantic-memory-turbo-quant feature - was causing feature mismatch with turbo-quant-codec.

## Fixes Applied This Session

### IndexChunks Job - DONE
Added `get_chunks_without_embedding()` to NotebookDb (notebook_db/mod.rs)
Implemented `execute_index_chunks()` (jobs/mod.rs) - embeds chunks via Ollama, adds to HNSW, updates DB

### Frontend Renderers - DONE
Created TimelineView.tsx - renders vertical timeline from JSON entries
Created DataTableView.tsx - renders table from StudioArtifact JSON format
Updated StudioPanel.tsx to use TimelineView and DataTableView renderers

### Blocked/Won't Fix
- piper-rs TTS: blocked by ort crate version conflict with fastembed (2.0.0-rc.9 vs 2.0.0-rc.12)
- hound WAV I/O: blocked by same ort conflict
- Audio overview, Slide deck, Infographic output types: no backend implementation in StudioOutputKind