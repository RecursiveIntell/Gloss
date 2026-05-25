# Current Feature Matrix

Status terms: `implemented`, `partial`, `degraded`, `deferred`, `blocked`.

| Feature | Status | Runtime truth |
| --- | --- | --- |
| Text/Markdown/code file import | partial | Folder/file import supports text-like files and code; P34 adds batch receipts and stale-notebook cancellation. |
| PDF ingestion | deferred | No PDF extractor is release-proven in current source. |
| DOCX ingestion | deferred | No DOCX extractor is release-proven in current source. |
| XLSX ingestion | deferred | No XLSX extractor is release-proven in current source. |
| URL import | deferred | No web fetch/import pipeline is release-proven in current source. |
| YouTube transcript import | deferred | No YouTube transcript pipeline is release-proven in current source. |
| Audio transcription | deferred | No audio transcription pipeline is release-proven in current source. |
| Audio overview/TTS | deferred | No audio generation pipeline is release-proven in current source. |
| Image/video import | partial | Queue paths exist, but end-to-end media smoke is not release-proven. |
| Semantic-memory preview | degraded | Opt-in; P34 adds chunk-budgeted projection and per-chunk failure records, but runtime smoke is still required. |
| TurboQuant acceleration | partial | Candidate acceleration only; exact rerank remains required. Release proof is pending full feature validation. |
| BM25/local retrieval | implemented | Stable fallback path, with fallback/degradation disclosed per answer. |
| Citation evidence | partial | `CitationAnchorV1` and `CitationFilterReasonV1` are emitted; runtime answer quality still needs smoke proof. |
| Studio reports | deferred | Studio report generation is not release-proven. |
| Studio flashcards/quizzes | deferred | Structured study outputs are not release-proven. |
| Mind maps/timelines | deferred | Structured visualization outputs are not release-proven. |
| Notebook export/import | deferred | Portable notebook package round-trip is not release-proven. |
| Desktop smoke | partial | `scripts/gloss_desktop_smoke_harness.py` now validates scripted runtime contracts and emits `live_desktop_exercised=false` without a live receipt; release-grade GUI smoke remains blocked. |
| DB doctor | deferred | No database repair/check command is release-proven. |
| Release replay | partial | `scripts/gloss_release_replay_gate.py --repo .` validates active gate presence; P35 strengthens fresh-unzip command replay, but release readiness still requires live desktop proof. |

Validation:

```bash
python3 scripts/gloss_feature_matrix_gate.py --repo .
```
