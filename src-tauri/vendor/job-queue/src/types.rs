use serde::{de::DeserializeOwned, Deserialize, Serialize};

/// Priority levels for queue jobs.
///
/// Jobs are processed in priority order: High (1), Normal (2), Low (3).
/// Within the same priority, jobs are processed in FIFO order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueuePriority {
    Low,
    Normal,
    High,
}

impl QueuePriority {
    pub fn as_i32(&self) -> i32 {
        match self {
            QueuePriority::Low => 3,
            QueuePriority::Normal => 2,
            QueuePriority::High => 1,
        }
    }

    pub fn from_i32(val: i32) -> Self {
        match val {
            1 => QueuePriority::High,
            2 => QueuePriority::Normal,
            _ => QueuePriority::Low,
        }
    }
}

/// Job status lifecycle: Pending -> Processing -> Completed/Failed/Cancelled
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueueJobStatus {
    Pending,
    Processing,
    Completed,
    Failed,
    Cancelled,
}

impl QueueJobStatus {
    pub fn as_str(&self) -> &str {
        match self {
            QueueJobStatus::Pending => "pending",
            QueueJobStatus::Processing => "processing",
            QueueJobStatus::Completed => "completed",
            QueueJobStatus::Failed => "failed",
            QueueJobStatus::Cancelled => "cancelled",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(QueueJobStatus::Pending),
            "processing" => Some(QueueJobStatus::Processing),
            "completed" => Some(QueueJobStatus::Completed),
            "failed" => Some(QueueJobStatus::Failed),
            "cancelled" => Some(QueueJobStatus::Cancelled),
            _ => None,
        }
    }
}

/// A generic queue job carrying a custom data payload.
///
/// The data field is stored as JSON in SQLite and deserialized back when the
/// executor picks up the job. Your data type must implement `Serialize`,
/// `DeserializeOwned`, `Clone`, `Send`, and `Sync`.
///
/// When creating jobs via [`QueueJob::new`], only `id`, `priority`, and `data`
/// are meaningful — the remaining fields (`status`, `created_at`, `started_at`,
/// `completed_at`, `error_message`) are set to defaults and populated by the
/// database when reading job records back.
///
/// ## Trace and retry primitives
///
/// The canonical trace/retry fields are:
/// - `trace_ctx`: canonical `stack_ids::TraceCtx` for end-to-end correlation
/// - `attempt_id`: one per re-enqueue (this crate is the retry owner)
/// - `trial_id`: one per concrete execution attempt
///
/// The legacy `trace_id: Option<String>` field is preserved for backward
/// compatibility and is kept in sync with `trace_ctx` automatically.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(bound(deserialize = "T: DeserializeOwned"))]
pub struct QueueJob<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + Sync,
{
    pub id: String,
    /// Phase status: compatibility / migration-only.
    ///
    /// Legacy trace identifier for end-to-end correlation.
    /// The canonical replacement is [`trace_ctx`](Self::trace_ctx).
    /// Use `stack_ids::TraceCtx::from_legacy_trace_id()` to convert.
    /// Removal condition: replaced when all callers migrate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_id: Option<String>,
    /// Canonical trace context for end-to-end correlation.
    ///
    /// Replaces the legacy `trace_id` field. The trace ID string is
    /// extractable via `trace_ctx.trace_id` for DB storage (the DB schema
    /// stores `trace_id TEXT` and is NOT changed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace_ctx: Option<stack_ids::TraceCtx>,
    /// Canonical attempt identity — one per re-enqueue.
    ///
    /// This crate is the retry owner: each re-enqueue creates a new
    /// `AttemptId`. Retries within the same attempt use different `TrialId`s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<stack_ids::AttemptId>,
    /// Canonical trial identity — one per concrete execution.
    ///
    /// Each time the executor picks up a job for execution, a new `TrialId`
    /// is generated. Multiple trials may share the same `AttemptId`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trial_id: Option<stack_ids::TrialId>,
    pub priority: QueuePriority,
    pub status: QueueJobStatus,
    pub data: T,
    /// Only populated when reading from DB.
    pub created_at: Option<String>,
    /// Only populated when reading from DB.
    pub started_at: Option<String>,
    /// Only populated when reading from DB.
    pub completed_at: Option<String>,
    /// Only populated when reading from DB.
    pub error_message: Option<String>,
}

impl<T> QueueJob<T>
where
    T: Serialize + DeserializeOwned + Clone + Send + Sync,
{
    /// Create a new job with a generated UUID and Normal priority.
    ///
    /// A fresh `AttemptId` is generated automatically (this crate is the
    /// retry owner — each enqueue is a new attempt).
    pub fn new(data: T) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            trace_id: None,
            trace_ctx: None,
            attempt_id: Some(stack_ids::AttemptId::generate()),
            trial_id: None,
            priority: QueuePriority::Normal,
            status: QueueJobStatus::Pending,
            data,
            created_at: None,
            started_at: None,
            completed_at: None,
            error_message: None,
        }
    }

    /// Set the priority for this job (builder pattern).
    pub fn with_priority(mut self, priority: QueuePriority) -> Self {
        self.priority = priority;
        self
    }

    /// Set a custom ID for this job (builder pattern).
    pub fn with_id(mut self, id: String) -> Self {
        self.id = id;
        self
    }

    /// Phase status: compatibility / migration-only.
    ///
    /// Attach an optional trace ID string for end-to-end correlation.
    /// Prefer [`with_trace_ctx()`](Self::with_trace_ctx) for new code.
    /// Removal condition: replaced when all callers migrate.
    pub fn with_trace_id(mut self, trace_id: impl Into<String>) -> Self {
        let id = trace_id.into();
        // Keep canonical field in sync when set via legacy path.
        self.trace_ctx = Some(stack_ids::TraceCtx::from_legacy_trace_id(&id));
        self.trace_id = Some(id);
        self
    }

    /// Attach a canonical `stack_ids::TraceCtx` for end-to-end correlation.
    ///
    /// Also sets the legacy `trace_id` string for DB storage compatibility
    /// (the DB schema stores `trace_id TEXT` and is NOT changed).
    pub fn with_trace_ctx(mut self, ctx: stack_ids::TraceCtx) -> Self {
        self.trace_id = Some(ctx.trace_id.clone());
        self.trace_ctx = Some(ctx);
        self
    }

    /// Set a pre-existing `AttemptId` (e.g., when reconstructing from DB).
    ///
    /// Normally callers do not need this — `new()` generates a fresh
    /// `AttemptId` automatically.
    pub fn with_attempt_id(mut self, attempt_id: stack_ids::AttemptId) -> Self {
        self.attempt_id = Some(attempt_id);
        self
    }

    /// Set a `TrialId` (normally assigned by the executor, not callers).
    pub fn with_trial_id(mut self, trial_id: stack_ids::TrialId) -> Self {
        self.trial_id = Some(trial_id);
        self
    }

    /// Get the canonical `TraceCtx`, preferring the canonical field,
    /// falling back to reconstruction from the legacy `trace_id` string.
    ///
    /// Returns `None` if neither field is set.
    pub fn resolve_trace_ctx(&self) -> Option<stack_ids::TraceCtx> {
        self.trace_ctx.clone().or_else(|| {
            self.trace_id
                .as_ref()
                .map(|id| stack_ids::TraceCtx::from_legacy_trace_id(id))
        })
    }

    /// Phase status: compatibility / migration-only.
    ///
    /// Convert the stored trace ID to a canonical `stack_ids::TraceCtx`.
    /// Returns `None` if no trace ID is set.
    /// Prefer [`resolve_trace_ctx()`](Self::resolve_trace_ctx) for new code.
    pub fn trace_ctx_compat(&self) -> Option<stack_ids::TraceCtx> {
        self.resolve_trace_ctx()
    }
}

/// Aggregate count of jobs by status.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct QueueStats {
    pub pending: u32,
    pub processing: u32,
    pub completed: u32,
    pub failed: u32,
    pub cancelled: u32,
}

/// Classification of a job failure, used for retry strategy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FailureClass {
    /// Failure is temporary (network blip, timeout). Will be retried.
    Transient,
    /// Failure is permanent (bad input, unrecoverable). Will not be retried.
    Permanent,
    /// Rate-limited. Retry after the specified delay.
    RateLimited { retry_after_secs: u64 },
}

impl FailureClass {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Transient => "transient",
            Self::Permanent => "permanent",
            Self::RateLimited { .. } => "rate_limited",
        }
    }
}

/// Result returned by a job handler after execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub success: bool,
    pub output: Option<String>,
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<FailureClass>,
}

impl JobResult {
    /// Create a successful result with no output.
    pub fn success() -> Self {
        Self {
            success: true,
            output: None,
            error: None,
            failure_class: None,
        }
    }

    /// Create a successful result with output data.
    pub fn success_with_output(output: String) -> Self {
        Self {
            success: true,
            output: Some(output),
            error: None,
            failure_class: None,
        }
    }

    /// Create a failure result with an error message.
    pub fn failure(error: String) -> Self {
        Self {
            success: false,
            output: None,
            error: Some(error),
            failure_class: Some(FailureClass::Permanent),
        }
    }

    /// Mark a failure result as retryable with structured classification.
    pub fn with_failure_class(mut self, failure_class: FailureClass) -> Self {
        self.failure_class = Some(failure_class);
        self
    }

    /// Create a transient failure result that should be retried.
    pub fn transient_failure(error: String) -> Self {
        Self::failure(error).with_failure_class(FailureClass::Transient)
    }

    /// Create a rate-limited failure result that should be retried later.
    pub fn rate_limited(error: String, retry_after_secs: u64) -> Self {
        Self::failure(error).with_failure_class(FailureClass::RateLimited { retry_after_secs })
    }
}

/// Queryable runtime details for a specific job.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueueJobDetails {
    pub id: String,
    pub priority: QueuePriority,
    pub status: QueueJobStatus,
    pub data_json: String,
    pub trace_id: Option<String>,
    pub created_at: Option<String>,
    pub started_at: Option<String>,
    pub completed_at: Option<String>,
    pub error_message: Option<String>,
    pub worker_id: Option<String>,
    pub heartbeat_at: Option<String>,
    pub visibility_timeout_secs: u64,
    pub failure_class: Option<String>,
    pub next_run_at: Option<String>,
    pub attempt_count: u32,
    /// Canonical attempt identity — one per re-enqueue. `None` for pre-v4 rows.
    pub attempt_id: Option<String>,
    /// Canonical trial identity — one per concrete execution. `None` for pre-v4 rows.
    pub trial_id: Option<String>,
}
