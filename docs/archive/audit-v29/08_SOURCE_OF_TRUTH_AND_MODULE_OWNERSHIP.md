# Source of Truth and Module Ownership

| Concept | Owner | Forbidden duplicate/substitute | Required proof |
|---|---|---|---|
| Source lifecycle | `src-tauri/src/db/notebook_db`, source commands | Frontend local guesses; semantic projection overwriting lifecycle | source lifecycle receipt / DB row |
| Source processing health | DB processing/projection tables + backend summary command | UI-derived status strings | `SourceHealthReceiptV1` |
| Chunks | ingestion chunker + DB | ad hoc prompt-only chunks | chunk digest + source span |
| Embeddings | FastEmbed provider boundary | Ollama debug fields when FastEmbed active | `EmbeddingDiagnosticsReceipt` and per-chunk embedding receipt |
| Dense index | HNSW/usearch persistence path | vector index as durable truth | index digest + reload/search fixture |
| FTS/BM25 | SQLite FTS5 DB | model-generated keywords as truth | FTS query fixture |
| semantic-memory projection | canonical `semantic-memory` crate via adapter | Gloss-local reimplementation of semantic-memory semantics | `SemanticMemoryProjectionReceiptV1` |
| TurboQuant | canonical `turbo-quant` / semantic-memory codec path | compile-only claim as contribution proof | exact rerank count + artifact digest |
| Retrieval decision | `RetrievalCapabilityDecisionV1` | scattered status strings | every answer includes decision object |
| Runtime truth | `SemanticMemoryRuntimeTruthV1` | settings UI state as proof | backend-authored truth receipt |
| Citations | retrieval/citation modules | model-only citation references | quote/source/chunk anchors |
| Prompt assembly | chat/context assembler | reconstructed prompt after the fact | `PromptReceiptV1` capture/redaction states |
| Generation settings | provider settings + decoding capability map | hardcoded temperature or fake controls | `DecodingSettingsReceiptV1` |
| Answer generation | provider adapter + chat command | UI streaming state as completion proof | `GenerationReceiptV1` |
| Notes | notes store/panel | Inspector Dock replacing notes | Notes preservation tests |
| Inspector Dock | UI rendering of backend truth | UI inventing backend state | UI smoke + receipt/evidence tabs |
| Queue/job state | job/queue modules | pending counted as running | queue status receipt |
| Package/run truth | current-run file + z.py/certifier | hardcoded P30/P36 defaults | package sidecar current run match |
| Public claims | release evidence manifest/public diff | README enthusiasm | proof-bounded claim diff |
