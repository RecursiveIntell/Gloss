# Source-of-Truth Map for Gloss Super-Pass

| Concept | Canonical owner | Gloss role | Forbidden drift |
|---|---|---|---|
| Notebook UX/local workflow | Gloss | Owns UI, commands, notebook workflow | Do not make UI state durable truth without receipts |
| Source files/imports | Gloss + receipt schemas | Owns app import pipeline | No silent text fallback; no unreceipted repair |
| Semantic memory | `semantic-memory` | Adapter/integration/user-visible runtime truth | No local semantic-memory fork |
| TurboQuant codec/proof | `semantic-memory`/`turbo-quant` where available | Candidate acceleration, exact proof or demotion | No speed/quality claim without benchmark/receipt |
| Queue semantics | `tauri-queue` if canonical | Schedule jobs and receipts | No hidden queue truth or untracked retries |
| Claim/evidence export | ClaimLedger/evidence package semantics | Notebook claim/evidence projection/import/export | No app-local claim ledger shadow truth |
| Prompt/generation receipts | Gloss receipt module, aligned to doctrine | Persist provider route, settings, prompt digest/redaction, output digest/status | No success-only receipts |
| Graphs | Explicit graph-surface declarations | Storage/retrieval/inference/control/repair separated | Retrieval expansion is not causal proof |
| Research references | ResearchPromotionPacketV1 | Advisory until promoted | No speculative claim leakage |
| Public claims | Proof packet/public claim gate | Safe copy only | No release/benchmark/security claims without proof |
