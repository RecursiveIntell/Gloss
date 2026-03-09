use serde::{Deserialize, Serialize};

// ── Phase status: compatibility / migration-only ──
//
// All event types below use `trace_id: Option<String>` and `attempt_count: Option<u32>`
// for backward compatibility. The canonical replacements are:
//   - `trace_ctx: Option<stack_ids::TraceCtx>` (for trace_id)
//   - `attempt_id: Option<stack_ids::AttemptId>` (for attempt identity)
//   - `trial_id: Option<stack_ids::TrialId>` (for per-execution identity)
//
// Use `stack_ids::TraceCtx::from_legacy_trace_id()` to convert trace IDs.
// The `attempt_count: u32` counter semantics map to one `AttemptId` per
// re-enqueue (queue is the retry owner), with each re-enqueue creating
// a new logical retry family.
//
// Removal condition: replaced when all event consumers migrate to canonical types.

/// Emitted when a job starts executing.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobStartedEvent {
    pub job_id: String,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`trace_ctx`](Self::trace_ctx) instead.
    /// Removal condition: removed when all event consumers migrate to `TraceCtx`.
    #[deprecated(note = "Use trace_ctx instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`attempt_id`](Self::attempt_id) / [`trial_id`](Self::trial_id) instead.
    /// Removal condition: removed when all event consumers migrate to `AttemptId`/`TrialId`.
    #[deprecated(note = "Use attempt_id/trial_id instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Canonical trace context (replaces `trace_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_ctx: Option<stack_ids::TraceCtx>,
    /// Canonical attempt identity — one per re-enqueue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<stack_ids::AttemptId>,
    /// Canonical trial identity — one per concrete execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<stack_ids::TrialId>,
}

/// Emitted when a job completes successfully.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCompletedEvent {
    pub job_id: String,
    pub output: Option<String>,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`trace_ctx`](Self::trace_ctx) instead.
    /// Removal condition: removed when all event consumers migrate to `TraceCtx`.
    #[deprecated(note = "Use trace_ctx instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`attempt_id`](Self::attempt_id) / [`trial_id`](Self::trial_id) instead.
    /// Removal condition: removed when all event consumers migrate to `AttemptId`/`TrialId`.
    #[deprecated(note = "Use attempt_id/trial_id instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Canonical trace context (replaces `trace_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_ctx: Option<stack_ids::TraceCtx>,
    /// Canonical attempt identity — one per re-enqueue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<stack_ids::AttemptId>,
    /// Canonical trial identity — one per concrete execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<stack_ids::TrialId>,
}

/// Emitted when a job fails.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobFailedEvent {
    pub job_id: String,
    pub error: String,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`trace_ctx`](Self::trace_ctx) instead.
    /// Removal condition: removed when all event consumers migrate to `TraceCtx`.
    #[deprecated(note = "Use trace_ctx instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`attempt_id`](Self::attempt_id) / [`trial_id`](Self::trial_id) instead.
    /// Removal condition: removed when all event consumers migrate to `AttemptId`/`TrialId`.
    #[deprecated(note = "Use attempt_id/trial_id instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_retry_at: Option<String>,
    /// Canonical trace context (replaces `trace_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_ctx: Option<stack_ids::TraceCtx>,
    /// Canonical attempt identity — one per re-enqueue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<stack_ids::AttemptId>,
    /// Canonical trial identity — one per concrete execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<stack_ids::TrialId>,
}

/// Emitted during job execution to report progress.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobProgressEvent {
    pub job_id: String,
    pub current_step: u32,
    pub total_steps: u32,
    pub progress: f64,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`trace_ctx`](Self::trace_ctx) instead.
    /// Removal condition: removed when all event consumers migrate to `TraceCtx`.
    #[deprecated(note = "Use trace_ctx instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`attempt_id`](Self::attempt_id) / [`trial_id`](Self::trial_id) instead.
    /// Removal condition: removed when all event consumers migrate to `AttemptId`/`TrialId`.
    #[deprecated(note = "Use attempt_id/trial_id instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Canonical trace context (replaces `trace_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_ctx: Option<stack_ids::TraceCtx>,
    /// Canonical attempt identity — one per re-enqueue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<stack_ids::AttemptId>,
    /// Canonical trial identity — one per concrete execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<stack_ids::TrialId>,
}

/// Emitted when a job is cancelled.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct JobCancelledEvent {
    pub job_id: String,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`trace_ctx`](Self::trace_ctx) instead.
    /// Removal condition: removed when all event consumers migrate to `TraceCtx`.
    #[deprecated(note = "Use trace_ctx instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worker_id: Option<String>,
    /// **Deprecated — Phase status: compatibility / migration-only.**
    /// Use [`attempt_id`](Self::attempt_id) / [`trial_id`](Self::trial_id) instead.
    /// Removal condition: removed when all event consumers migrate to `AttemptId`/`TrialId`.
    #[deprecated(note = "Use attempt_id/trial_id instead. Will be removed when all consumers migrate.")]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    /// Canonical trace context (replaces `trace_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_ctx: Option<stack_ids::TraceCtx>,
    /// Canonical attempt identity — one per re-enqueue.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<stack_ids::AttemptId>,
    /// Canonical trial identity — one per concrete execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<stack_ids::TrialId>,
}
