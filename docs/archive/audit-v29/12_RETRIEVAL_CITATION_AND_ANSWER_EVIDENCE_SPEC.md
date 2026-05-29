# Retrieval, Citation, and Answer Evidence Spec

## Requirements

- Every answer has a `RetrievalCapabilityDecisionV1`.
- Every answer has a `RetrievalReceiptV1` linking requested/effective backend, source scope, candidates, context passages, fallback chain, and degraded states.
- Every context passage has source id, chunk id, quote/text digest, retrieval engine, score provenance where available, and citation anchor.
- Citation filtering must record reason codes, not just counts.
- No answer may display “grounded” unless citations/anchors pass validation.
- Strict source scope mode must fail or degrade explicitly if candidates fall outside scope.
- Evidence tab must render captured receipt state, not recompute from UI state.

## Acceptance gates

- BM25-only answer emits anchors.
- native-hybrid answer emits dense+BM25 provenance.
- semantic-memory answer emits semantic-memory candidate mapping.
- fallback answer marks fallback and degraded truth.
- no-citation answer says unsupported/no citation rather than hiding it.
