# Gloss source state model spec

## Problem

Current UI contradictions come from a single field:

```ts
source.status: string
```

It is used for lifecycle, indexing, semantic-memory projection, errors, and frontend readiness. This creates impossible states such as source rows marked `semantic_memory_synced` while the chat panel counts all selected sources as `not ready`.

## Required model

Add backend + frontend model:

```ts
export type SourceLifecycleStatus =
  | "pending" | "extracting" | "chunking" | "indexing" | "ready" | "error" | "skipped";

export type SummaryStatus =
  | "missing" | "queued" | "processing" | "ready" | "failed" | "skipped" | "stale";

export type FtsIndexStatus = "missing" | "indexed" | "failed";
export type DenseIndexStatus = "disabled" | "missing" | "queued" | "indexing" | "indexed" | "failed" | "stale";
export type SemanticProjectionStatus =
  | "disabled" | "not_projected" | "queued" | "projecting" | "synced" | "partial" | "degraded" | "failed" | "stale";

export interface SourceProcessingState {
  source_id: string;
  lifecycle_status: SourceLifecycleStatus;
  summary_status: SummaryStatus;
  fts_index_status: FtsIndexStatus;
  dense_index_status: DenseIndexStatus;
  semantic_projection_status: SemanticProjectionStatus;
  lifecycle_error?: string | null;
  summary_error?: string | null;
  index_error?: string | null;
  semantic_projection_error?: string | null;
  last_ingest_receipt_id?: string | null;
  last_dense_index_receipt_id?: string | null;
  last_projection_receipt_id?: string | null;
  updated_at: string;
}

export interface Source {
  ...existing fields...
  status: string; // legacy display only; do not use for readiness when processing_state exists
  processing_state?: SourceProcessingState | null;
}
```

## Required DB table

```sql
CREATE TABLE IF NOT EXISTS source_processing_state (
  source_id TEXT PRIMARY KEY REFERENCES sources(id) ON DELETE CASCADE,
  lifecycle_status TEXT NOT NULL DEFAULT 'pending',
  summary_status TEXT NOT NULL DEFAULT 'missing',
  fts_index_status TEXT NOT NULL DEFAULT 'missing',
  dense_index_status TEXT NOT NULL DEFAULT 'missing',
  semantic_projection_status TEXT NOT NULL DEFAULT 'disabled',
  lifecycle_error TEXT,
  summary_error TEXT,
  index_error TEXT,
  semantic_projection_error TEXT,
  last_ingest_receipt_id TEXT,
  last_dense_index_receipt_id TEXT,
  last_projection_receipt_id TEXT,
  updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
```

## Frontend readiness formulas

```ts
const lifecycle = source.processing_state?.lifecycle_status ?? source.status;
const notReady = lifecycle !== "ready";
const ftsMissing = source.processing_state?.fts_index_status !== "indexed";
const denseProblem = ["missing", "failed", "stale"].includes(source.processing_state?.dense_index_status ?? "missing");
const projectionProblem = ["failed", "partial", "degraded", "stale", "not_projected"].includes(source.processing_state?.semantic_projection_status ?? "not_projected");
```

No code may compute release readiness from `source.status !== "ready"` after this pass.
