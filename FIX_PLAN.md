# Gloss Fix Plan

## Priority 1: Stub Jobs (High Risk - Breaks Background Processing)

### Fix IndexChunks Job (jobs/mod.rs) - DONE
**File**: `/home/sikmindz/Coding/Gloss/src-tauri/src/jobs/mod.rs`
**Status**: IMPLEMENTED - all 185 tests pass, all validation gates pass

---

## Priority 2: Audio Generation Pipeline (Phase 4) - BLOCKED

### Add TTS Dependencies (Cargo.toml)
**Blocked**: piper-rs requires ort 2.0.0-rc.12, fastembed requires 2.0.0-rc.9 - incompatible versions

**Would Add**:
```toml
piper-rs = "0.2"  # or later when ort conflict resolved
hound = "3.5"
```

### Create Audio Generation Command (src-tauri/src/commands/audio.rs - NEW FILE - BLOCKED)
Requires TTS dependencies first.

---

## Priority 3: Missing Frontend Components - DONE

### TimelineView - DONE
Created `/home/sikmindz/Coding/Gloss/src/components/studio/TimelineView.tsx`
- Parses timeline JSON entries with sequence/label/event
- Renders vertical timeline with markers

### DataTableView - DONE
Created `/home/sikmindz/Coding/Gloss/src/components/studio/DataTableView.tsx`
- Parses StudioArtifact JSON table format
- Renders sortable table

---

## Priority 4: QuizWidget Explain Button - NOT STARTED

- Backend lacks `explain_quiz_question` command
- Frontend shows explanations if present in initial generation
- Would add value but low priority

---

## Priority 5: Multi-angle Query Rewriting - NOT STARTED

- SPEC requires LLM to generate 2 rephrased queries (SPEC §7.1)
- Requires settings toggle to enable/disable
- Must be implemented in retrieval layer before hybrid search

---

## Priority 6: Template System - NOT STARTED

- Templates are hardcoded in studio/mod.rs (not TOML config as SPEC requires)
- Would need to create prompts/studio_templates.toml
- Low priority: current hardcoded templates work functionally

---

## Priority 7: Candle Embedder Replacement (Unblocks TTS)

### Replace fastembed with candle in EmbeddingService
**File**: `/home/sikmindz/Coding/Gloss/src-tauri/Cargo.toml`

**Current**: `fastembed = "4"` (depends on ort 2.0.0-rc.9)

**Replace with**:
```toml
# Remove fastembed
candle-core = "0.10"
candle-nn = "0.10"
candle-transformers = "0.10"
hf-hub = { version = "1.0.0-rc.1", default-features = false, features = ["blocking"] }
tokenizers = { version = "0.23", default-features = false, features = ["onig"] }
```

**Update**: `/home/sikmindz/Coding/Gloss/src-tauri/src/ingestion/embed.rs`
- Remove `fastembed::{...}` import
- Remove `EmbeddingBackend::FastEmbed` variant (and the reranker field)
- Create `EmbeddingBackend::Candle` variant matching semantic-memory's CandleEmbedder
- Download `nomic-ai/nomic-embed-text-v1.5` from HuggingFace (768 dimensions)

**Alternative**: Use semantic-memory's CandleEmbedder directly
- semantic-memory already exports `CandleEmbedder` when `candle-embedder` feature enabled
- Would need to align APIs (EmbeddingService vs Embedder trait)

### Add TTS with candle: any-tts
**File**: `/home/sikmindz/Coding/Gloss/src-tauri/Cargo.toml`
```toml
any-tts = { version = "0.1.2", default-features = false, features = ["download"] }
hound = "3.5"
```

This uses candle-core 0.10.2 which matches semantic-memory. No ort conflict.

---

## Verification Summary

- Build: `cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant` - PASS
- Tests: `cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant` - 185 passed
- Validation gates: `bash validation/run_all_gloss_repair_gates.sh` - ALL PASS

---

## Remaining Gaps (SPEC Required But Not Implemented)

### Phase 4 Output Types Missing (No StudioOutputKind Variant)
- `audio_deep_dive` - backend missing
- `audio_brief` - backend missing
- `audio_critique` - backend missing
- `audio_debate` - backend missing
- `slide_deck` - backend missing
- `infographic` - backend missing

### Studio Output Types Without Backend
- briefing_doc - listed in SPEC §6.1 but no StudioOutputKind variant
- study_guide - listed in SPEC §6.1 but no StudioOutputKind variant
- custom_report - listed in SPEC §6.1 but no StudioOutputKind variant

### Blocked UI Components
- AudioPlayer - needs backend audio outputs first
- SlideViewer - needs slide_deck backend
- InfographicView - needs infographic backend
- TTS blocked by ort crate conflict with fastembed