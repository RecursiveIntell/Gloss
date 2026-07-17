# Data Model and Migration Plan

## Required schema additions

1. `generation_settings` or provider-scoped settings rows:
   - provider
   - model pattern / exact model override
   - temperature
   - top_k/top_p/min_p/typical_p
   - repeat_last_n/repeat_penalty
   - presence/frequency penalty
   - seed
   - num_ctx/num_predict
   - stop sequences
   - keep_alive
   - stream
   - updated_at

2. `answer_generation_receipts`:
   - answer/message id
   - provider/model route
   - decoding settings receipt JSON
   - prompt receipt id
   - retrieval decision id
   - timeout/stream status
   - final status
   - hashes and redaction states

3. `prompt_receipts`:
   - prompt receipt id
   - answer/message id
   - system prompt capture state
   - system prompt digest/preview/redaction
   - user turn digest
   - context payload digest
   - source/chunk ids
   - prompt assembly version

4. `retrieval_decision_receipts`:
   - decision id
   - requested/effective backend
   - runtime truth id
   - reason codes
   - fallback/degraded flags

5. `semantic_memory_runtime_truth_receipts`:
   - truth id
   - build flags
   - settings snapshot
   - projection/link counts
   - decision summary

6. `timeout_continuation_receipts`:
   - message id
   - previous attempt id
   - partial hash
   - continuation query/attempt id
   - state transition

## Migration order

1. Add tables with no behavior changes.
2. Write receipts on new answers while preserving old answer display.
3. Add read APIs for Inspector Dock.
4. Add source health reconciliation/backfill tables/receipts.
5. Add legacy fallback: old answers display `not_captured` instead of reconstructed data.

## Rollback

All receipt tables must be append-only. If UI fails, disable Inspector Dock while leaving receipts intact. Do not drop tables in rollback; use migration guard/feature flag.
