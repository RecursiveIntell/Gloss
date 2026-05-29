# Executive Decision

## Verdict

Gloss is promising but not release-ready. The next Codex run must be a **release-candidate proof pass**, not a broad feature blast.

## Release-candidate target

Gloss may be considered release-candidate complete only after:

- runtime truth is backend-authored and attached to every answer;
- `RetrievalCapabilityDecisionV1` is canonical for answer routing;
- semantic-memory serves at least one strict live fixture answer with `fallback_used=false`;
- dense indexing receipt proves `indexed_chunks > 0`;
- semantic-memory projection receipt proves `live_projection_sources > 0`;
- TurboQuant exact rerank/artifact contribution is proven or all TQ contribution claims are demoted;
- effective decoding settings are configurable, provider-aware, and captured per answer;
- prompt/generation receipts are attached to answers without fake capture;
- long local generation timeout is increased by 40%, partial output persists, and continuation works;
- Inspector Dock shows Notes, Prompt, Evidence, Receipt, and Sources without regressing Notes;
- package/current-run truth is repaired;
- final release receipt sets `release_ready=true` only after all gates pass.

## Broad-spec target

Full NotebookLM-style feature completion is a later track. It includes PDF/DOCX/XLSX/PPTX/EPUB/HTML/URL/YouTube/audio/video ingestion, Studio outputs, export/import, DB doctor, model comparison, and packaging polish. These are blocked behind the RC core unless a broad feature is directly required to prove the RC core.

## Hard decision

Do not let Codex implement broad features until the RC proof gate passes. If Codex reaches broad phases before RC proof receipts are green, the run is invalid.
