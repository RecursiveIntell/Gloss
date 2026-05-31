# Gloss P35 — Implementation Specification

**Date:** 2026-05-30
**Based on:** SPEC-gloss.md v1.0.0, hostile-audit findings, re-audit, feature completeness analysis
**Target:** Production release candidate

---

## 0. Architecture Constraints (Non-Negotiable)

| Constraint | Why | Design Impact |
|---|---|---|
| 8GB VRAM, single GPU model | GTX 1070, Ollama on LAN | No concurrent GPU inference. Chat/studio/embedding all sequential on GPU. CPU work (embedding, search) parallel. |
| Per-notebook SQLite + HNSW | Portability, no server process | `~/.local/share/gloss/notebooks/{uuid}/` isolation |
| Feature flags as kill switches | Runtime stability over premature rollout | Every new feature gated behind `features.rs` flag, opt-in until proven |
| Chat MUST work without retrieval | User never blocked by source state | `SourceScope::None` is always valid, never an error path |
| All outputs include receipts | Provenance is the product | Every LLM call, every chunk, every export produces a receipt |
| Tauri IPC boundary | Backend ↔ frontend is an API contract | No direct DB access from frontend. Every mutation goes through a Tauri command. |

---

## 1. Feature Completion — What's Actually Needed

### 1.1 ALREADY DONE (ship as-is)

```
✅ Chat streaming with RAG (embed→HNSW→FTS5→RRF→stuff→generate→cite)
✅ 4 LLM providers (Ollama, OpenAI, Anthropic, llama.cpp) + model registry
✅ 6 source input types (file, URL, paste, folder, YouTube transcript, batch)
✅ Per-notebook isolation (SQLite FTS5, HNSW index, dual memory backends)
✅ Studio: 10 output types via unified pipeline (LLM→structured output→render)
✅ Export/import with SHA256 manifests, receipts, validation
✅ Inspectors: evidence, prompt, receipt, notes, source content
✅ Feature flag system (14 flags, runtime toggle, build-time gating)
✅ Dual memory backends (gloss-local + semantic-memory-preview)
✅ TurboQuant acceleration (candidate phase)
✅ Background auto-summarization (GPU/LLM gate coordination)
✅ Diagnostics panel (provider health, model refresh, DB doctor, retrieval probe)
✅ Notes system with citation backlinks
✅ DB doctor (check + repair)
✅ External tool detection (ffmpeg, yt-dlp)
✅ YouTube transcript import (pure Rust, no download)
✅ PDF, DOCX, XLSX, PPTX, EPUB text extraction
✅ Audio file metadata extraction (ffprobe receipts)
✅ Path traversal hardening
✅ Provider URL validation with allowlist
✅ Redaction system (API keys, LAN topology)
✅ React ErrorBoundary wrapping
✅ Toast notifications with aria roles
✅ Responsive resize with cleanup
```

### 1.2 MUST FIX FOR RELEASE (P35 blockers)

| # | Item | Effort | Approach |
|---|---|---|---|
| **M-1** | Live desktop smoke test | 2h | Headed environment: launch app, add sources, chat with retrieval, chat without retrieval, verify all 10 studio types generate, verify settings persist, verify export/import roundtrip |
| **M-2** | AppImage packaging | 3h | `tauri-bundler` → AppImage. Verify: runs on fresh Nobara, finds Ollama, loads models, reads/writes to `~/.local/share/gloss/`. Test on clean VM or second machine. |
| **M-3** | Performance certification | 1h | Run retrieval probe gate, verify RRF merge latency < 200ms, verify first-token latency < 2s on local 7B model, verify no unbounded memory growth over 1h idle |
| **M-4** | Embedding URL LAN policy parity | 1h | Backend: mirror provider LAN validation for embedding URLs. Reject LAN/cloud embedding URLs unless `allow_lan_local_providers` is set. Frontend: warning is already shown (F-10). This is backend enforcement. |
| **M-5** | Final receipt regeneration | 30m | After all fixes, regenerate `FINAL_RECEIPT.json` with all gate results, commit SHAs, and release candidate gate pass. |

### 1.3 SHOULD FIX FOR RELEASE (quality, not blocking)

| # | Item | Effort | Approach |
|---|---|---|---|
| **S-1** | Conversational styles | 2h | Three styles defined: `default`, `learning_guide`, `custom`. Backend: `format_system_prompt()` switches on style, injects instruction. `default` = current behavior. `learning_guide` = socratic tutor persona. `custom` = loads `custom_goal` from notebook_config. Frontend: dropdown in chat header. All styles emit the same receipts — only system prompt differs. |
| **S-2** | Response length control | 30m | `default` (current), `short` (halve max_tokens), `long` (double max_tokens). A setting, not a style — applies across all conversations. Store in notebook_config. Frontend: dropdown or toggle in chat header. |
| **S-3** | Flashcard widget | 3h | Interactive card flip UI. Frontend: new `FlashcardWidget.tsx`. State: index, flipped[], known[], review[]. Backend: no changes needed — studio already generates `flashcards` JSON. Renderer: card face with front/back, flip animation (CSS 3D transform), Know/Review buttons, progress bar, citation link back to source. |
| **S-4** | Quiz widget | 2h | Frontend: `QuizWidget.tsx`. State: currentQuestion, selectedOption, score, answered[]. Render: question, 4 options (clickable), highlight correct/incorrect, explanation reveal, Next button, final score with retry. |
| **S-5** | Mind map graph | 4h | Frontend: `MindMapGraph.tsx` using d3-force. Read `nodes[]` + `edges[]` from studio JSON. Render: force-directed layout, zoom/pan, node hover (shows summary tooltip), click (navigates to source). SVG-based, dark theme. |
| **S-6** | Image/video import flow | 4h | Already has backend scaffolding (`vision_jobs`, `video_import` feature flags). Wire frontend: "Add Image" button in SourcesPanel, trigger Tauri command `add_source_file` with image mime type detection. Backend: `ExtractSource` dispatches to vision model description when `source_type = image`. For video: extract audio → whisper transcription (already scaffolded). Gate behind feature flags. |

### 1.4 DEFER TO P36+ (post-release)

```
❌ Audio overviews / TTS (Piper integration, script generation, WAV assembly)
❌ Slide deck generation + viewer
❌ Infographic generation + PNG export
❌ Audio podcast (Deep Dive, Brief, Critique, Debate — all TTS-based)
❌ Multi-angle query rewriting (reranker exists but query rewriting not wired)
❌ Conversational styles beyond three basic ones
❌ Custom goals per notebook (schema exists, not wired)
❌ Timeline interactive view (currently renders as Markdown, not TimelineView)
❌ DataTable interactive view (currently renders as Markdown)
```

---

## 2. Implementation Patterns

### 2.1 Feature Flag Gating

Every new feature follows this pattern:

```rust
// features.rs — add constant
pub const FEATURE_FLASHCARD_WIDGET_ENABLED: &str = "feature_flashcard_widget_enabled";

// Then in FeatureDefinition array
FeatureDefinition {
    id: FEATURE_FLASHCARD_WIDGET_ENABLED,
    label: "Flashcard Widget",
    section: "studio",
    description: "Interactive card-flip study widget for studio flashcard outputs",
    default_enabled: false,
    stable: false,
    requires_experimental: true,
    build_feature: None,
}
```

Frontend check:
```typescript
// In FlashcardWidget.tsx
const enabled = useSettingsStore(s => s.flags?.["feature_flashcard_widget_enabled"]);
if (!enabled) return <MarkdownViewer content={output.raw_content} />;
```

### 2.2 Studio Output → Renderer Dispatch

Current architecture: all 10 outputs render through `renderValue()` (generic JSON → text). New pattern:

```typescript
// StudioOutputBody.tsx
const RENDERERS: Record<string, React.FC<{output: StudioOutput}>> = {
  flashcards: FlashcardWidget,
  quiz: QuizWidget,
  mind_map: MindMapGraph,
  timeline: MarkdownViewer,  // no interactive renderer yet
  data_table: MarkdownViewer, // same
  // ... others default to MarkdownViewer
};

function StudioOutputBody({ output }: { output: StudioOutput }) {
  const Renderer = RENDERERS[output.output_type] || MarkdownViewer;
  return <Renderer output={output} />;
}
```

### 2.3 Backend Command Registration

```rust
// lib.rs — add to invoke_handler
.manage(tauri::generate_handler![
    // ... existing commands
    commands::sources::add_source_media,  // new: unified image/video/audio add
])
```

Command pattern:
```rust
#[tauri::command]
pub async fn add_source_media(
    state: State<'_, AppState>,
    notebook_id: String,
    file_path: String,  // already in sources/ dir from file dialog
) -> Result<Source, GlossError> {
    // 1. Detect mime type from file extension
    // 2. Create source row (status: 'pending')
    // 3. Queue ExtractSource job
    // 4. Return source with id so frontend can track
}
```

### 2.4 Conversational Styles

Minimal backend change — only the system prompt construction differs:

```rust
fn format_system_prompt(style: &str, custom_goal: Option<&str>, source_count: usize) -> String {
    let base = format!("You are Gloss, a local-first research assistant...");
    match style {
        "learning_guide" => format!(
            "{}\nYou are acting as a Socratic tutor. Ask questions to guide understanding. \
            When the user is stuck, provide hints rather than direct answers. \
            Always cite sources.", base
        ),
        "custom" => format!(
            "{}\nAdditional instruction from the user: {}",
            base,
            custom_goal.unwrap_or("")
        ),
        _ => base, // "default"
    }
}
```

Frontend: dropdown in ChatPanel header, persisted in `chatStore.sendMessage()` as a parameter.

---

## 3. Task Breakdown

### Phase 3A: Release Blockers (must complete)

| Task | ID | Deps | Est. | Owner |
|---|---|---|---|---|
| Embedding URL LAN policy backend enforcement | M-4 | none | 1h | Backend |
| Live desktop smoke test — full feature walkthrough | M-1 | M-4 | 2h | QA |
| AppImage packaging + clean install test | M-2 | M-1 | 3h | DevOps |
| Performance certification (gate run) | M-3 | M-1 | 1h | Backend |
| Regenerate FINAL_RECEIPT.json | M-5 | M-1..M-4 | 30m | Gate |

### Phase 3B: Quality Improvements (should complete)

| Task | ID | Deps | Est. | Owner |
|---|---|---|---|---|
| Conversational styles (3 styles) | S-1 | none | 2h | Full stack |
| Response length control | S-2 | none | 30m | Full stack |
| Flashcard widget | S-3 | none (studio already generates) | 3h | Frontend |
| Quiz widget | S-4 | none (studio already generates) | 2h | Frontend |
| Mind map graph (d3-force) | S-5 | none | 4h | Frontend |
| Image/video import frontend wiring | S-6 | none | 4h | Full stack |

### Phase 3C: Verification

| Task | ID | Deps | Est. | Owner |
|---|---|---|---|---|
| Re-run hostile audit | V-1 | all above | 2h | Auditor |
| Full gate pass (35 gates) | V-2 | all above | 30m | Gate |
| Commit + push to release branch | V-3 | V-1, V-2 | 15m | Git |
| Tag release candidate | V-4 | V-3 | 5m | Git |

---

## 4. Verification Gates (must all pass)

```bash
# Standard gates
cargo check --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo test --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo clippy --manifest-path src-tauri/Cargo.toml --features semantic-memory-turbo-quant
cargo fmt --all -- --check
npm run build  # tsc + vite

# Project gates
python3 validation/gloss_release_candidate_gate.py --repo .
python3 validation/gloss_embedding_provider_gate.py --repo .
python3 validation/validate_source_send_gate.py .
python3 validation/validate_frontend_event_routing.py .
python3 validation/validate_chat_terminal_contract.py .
python3 validation/validate_provider_lan_policy.py .
python3 validation/validate_release_receipt_consistency.py .
```

---

## 5. Ship Criteria

- [ ] All 35 gates pass
- [ ] 147+ backend tests pass
- [ ] TypeScript build clean (0 errors, 0 warnings)
- [ ] AppImage builds and runs on clean Nobara
- [ ] Live desktop smoke: add source → chat with retrieval → chat without → all 10 studio types → export → import → settings persist
- [ ] Hostile audit finds 0 CRITICAL, 0 HIGH
- [ ] FINAL_RECEIPT.json regenerated with all pass results
- [ ] No `unwrap()` in production code
- [ ] No silent error swallowing
- [ ] All new features behind feature flags (opt-in, not on by default)
