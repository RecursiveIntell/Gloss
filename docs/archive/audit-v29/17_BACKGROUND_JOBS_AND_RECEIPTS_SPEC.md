# Background Jobs and Receipts Spec

## Required job truth

- pending count separate from processing count;
- explicit paused/quiet mode state;
- per-job batch id;
- retry/deadline/cancel status;
- receipt per ingestion/backfill/projection/summarization batch;
- source-terminal counts match DB rows.

## Required job classes

- ingestion;
- embedding/dense indexing;
- semantic-memory projection/backfill;
- summarization;
- source health reconciliation;
- release smoke.

## Quiet Mode

Quiet Mode must pause or throttle summaries, projection/backfill, dense rebuilds, and other background model work while chat/generation is active.

## Acceptance gates

- Queue UI shows `processing / pending`.
- Cancelling a projection batch leaves a safe resumable receipt.
- Import batch failed counts include normal ingestion errors, not only panics.
