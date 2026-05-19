# semantic-memory and Gloss integration plan

## semantic-memory

Source-of-truth rule:

- SQLite/canonical vector store remains authoritative.
- Raw/canonical f32 vectors must not be deleted or replaced by compressed codes.
- TurboQuant codes are derived sidecars.
- Approximate scores are candidate-generation evidence only.
- Exact f32 rerank is required before final retrieval output.

Required future artifacts:

- `RetrievalWitness`
- `CompressionReceiptV1`
- `BenchmarkReceiptV1`
- `DriftReport`
- `PromotionReport`

## Gloss

Gloss should expose TurboQuant through shadow mode first:

- exact retrieval result
- compressed candidate result
- top-k overlap
- score drift
- encoded bytes
- codec profile digest
- warnings/limitations

No silent feature flip.

## Promotion gate

Do not make TurboQuant default unless:

- exact rerank exists
- answer-evidence parity passes
- recall@k threshold is met on representative corpora
- receipts are persisted
- rollback path exists
