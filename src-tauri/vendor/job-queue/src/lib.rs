//! # Job Queue
//!
//! Production-grade background job queue system extracted from tauri-queue.
//! Framework-agnostic — no Tauri dependency.
//!
//! ## Features
//!
//! - Priority-based scheduling (High, Normal, Low)
//! - SQLite persistence with crash recovery
//! - Hardware throttling (cooldown, max consecutive runs)
//! - Real-time cancellation during job execution
//! - Progress tracking via pluggable event emitter
//! - Pause/resume capability
//!
//! ## Quick Start
//!
//! 1. Define a job type implementing [`JobHandler`]
//! 2. Create a [`QueueManager`] with a [`QueueConfig`]
//! 3. Add jobs with [`QueueManager::add()`]
//! 4. Spawn the executor with [`QueueManager::spawn()`]

pub mod config;
pub mod db;
pub mod error;
pub mod events;
pub mod executor;
pub mod queue;
pub mod types;

pub use config::{QueueConfig, QueueConfigBuilder};
pub use error::QueueError;
pub use executor::ProcessedJob;
pub use queue::QueueManager;
pub use types::{
    FailureClass, JobResult, QueueJob, QueueJobDetails, QueueJobStatus, QueuePriority, QueueStats,
};

use rusqlite::Connection;
use std::sync::{Arc, Mutex};

/// Trait for emitting queue lifecycle events.
///
/// Implement this trait to receive notifications about job state changes.
/// For example, a Tauri app would implement this to emit events to the frontend,
/// while a CLI app might log to stdout or update a progress bar.
pub trait QueueEventEmitter: Send + Sync + 'static {
    /// Called when a job starts executing.
    fn emit_job_started(&self, event: events::JobStartedEvent);
    /// Called when a job completes successfully.
    fn emit_job_completed(&self, event: events::JobCompletedEvent);
    /// Called when a job fails.
    fn emit_job_failed(&self, event: events::JobFailedEvent);
    /// Called when a job reports progress.
    fn emit_job_progress(&self, event: events::JobProgressEvent);
    /// Called when a job is cancelled.
    fn emit_job_cancelled(&self, event: events::JobCancelledEvent);
}

/// A no-op event emitter that discards all events.
///
/// Useful for testing or when you don't need event notifications.
pub struct NoopEventEmitter;

impl QueueEventEmitter for NoopEventEmitter {
    fn emit_job_started(&self, _event: events::JobStartedEvent) {}
    fn emit_job_completed(&self, _event: events::JobCompletedEvent) {}
    fn emit_job_failed(&self, _event: events::JobFailedEvent) {}
    fn emit_job_progress(&self, _event: events::JobProgressEvent) {}
    fn emit_job_cancelled(&self, _event: events::JobCancelledEvent) {}
}

/// A logging event emitter that logs events via `tracing`.
///
/// Useful for daemon or CLI usage where events should appear in logs.
pub struct LoggingEventEmitter;

#[allow(deprecated)]
impl QueueEventEmitter for LoggingEventEmitter {
    fn emit_job_started(&self, event: events::JobStartedEvent) {
        // Prefer canonical trace_ctx over legacy trace_id
        let trace = event.trace_ctx.as_ref()
            .map(|ctx| ctx.trace_id.as_str())
            .or(event.trace_id.as_deref())
            .unwrap_or("");
        let attempt = event.attempt_id.as_ref()
            .map(|a| a.to_string())
            .unwrap_or_else(|| event.attempt_count.map(|c| c.to_string()).unwrap_or_default());
        tracing::info!(
            job_id = %event.job_id,
            worker_id = event.worker_id.as_deref().unwrap_or(""),
            trace_id = %trace,
            attempt = %attempt,
            trial_id = event.trial_id.as_ref().map(|t| t.to_string()).unwrap_or_default().as_str(),
            "Job started"
        );
    }
    fn emit_job_completed(&self, event: events::JobCompletedEvent) {
        let trace = event.trace_ctx.as_ref()
            .map(|ctx| ctx.trace_id.as_str())
            .or(event.trace_id.as_deref())
            .unwrap_or("");
        let attempt = event.attempt_id.as_ref()
            .map(|a| a.to_string())
            .unwrap_or_else(|| event.attempt_count.map(|c| c.to_string()).unwrap_or_default());
        tracing::info!(
            job_id = %event.job_id,
            worker_id = event.worker_id.as_deref().unwrap_or(""),
            trace_id = %trace,
            attempt = %attempt,
            trial_id = event.trial_id.as_ref().map(|t| t.to_string()).unwrap_or_default().as_str(),
            "Job completed"
        );
    }
    fn emit_job_failed(&self, event: events::JobFailedEvent) {
        let trace = event.trace_ctx.as_ref()
            .map(|ctx| ctx.trace_id.as_str())
            .or(event.trace_id.as_deref())
            .unwrap_or("");
        let attempt = event.attempt_id.as_ref()
            .map(|a| a.to_string())
            .unwrap_or_else(|| event.attempt_count.map(|c| c.to_string()).unwrap_or_default());
        tracing::warn!(
            job_id = %event.job_id,
            worker_id = event.worker_id.as_deref().unwrap_or(""),
            trace_id = %trace,
            attempt = %attempt,
            trial_id = event.trial_id.as_ref().map(|t| t.to_string()).unwrap_or_default().as_str(),
            failure_class = event.failure_class.as_deref().unwrap_or(""),
            error = %event.error,
            "Job failed"
        );
    }
    fn emit_job_progress(&self, event: events::JobProgressEvent) {
        let trace = event.trace_ctx.as_ref()
            .map(|ctx| ctx.trace_id.as_str())
            .or(event.trace_id.as_deref())
            .unwrap_or("");
        tracing::debug!(
            job_id = %event.job_id,
            worker_id = event.worker_id.as_deref().unwrap_or(""),
            trace_id = %trace,
            progress = %event.progress,
            step = %event.current_step,
            total = %event.total_steps,
            "Job progress"
        );
    }
    fn emit_job_cancelled(&self, event: events::JobCancelledEvent) {
        let trace = event.trace_ctx.as_ref()
            .map(|ctx| ctx.trace_id.as_str())
            .or(event.trace_id.as_deref())
            .unwrap_or("");
        tracing::info!(
            job_id = %event.job_id,
            worker_id = event.worker_id.as_deref().unwrap_or(""),
            trace_id = %trace,
            "Job cancelled"
        );
    }
}

/// Context provided to job handlers during execution.
///
/// Gives access to an event emitter for reporting progress and
/// methods for checking cancellation.
#[allow(deprecated)]
pub struct JobContext {
    /// The ID of the currently executing job.
    pub job_id: String,
    /// Phase status: compatibility / migration-only.
    ///
    /// Legacy trace ID for correlating queue work with upstream orchestration.
    /// Prefer [`trace_ctx`](Self::trace_ctx) for new code.
    #[deprecated(note = "Use trace_ctx instead. Will be removed when all consumers migrate.")]
    pub trace_id: Option<String>,
    /// Canonical trace context for end-to-end correlation.
    pub trace_ctx: Option<stack_ids::TraceCtx>,
    /// Canonical attempt identity — one per re-enqueue.
    pub attempt_id: Option<stack_ids::AttemptId>,
    /// Canonical trial identity — one per concrete execution.
    pub trial_id: Option<stack_ids::TrialId>,
    /// Worker identity currently holding the job lease.
    pub worker_id: Option<String>,
    /// Phase status: compatibility / migration-only.
    ///
    /// Current attempt count (legacy counter).
    #[deprecated(note = "Use attempt_id/trial_id instead. Will be removed when all consumers migrate.")]
    pub attempt_count: u32,
    /// Event emitter for reporting progress.
    pub(crate) event_emitter: Arc<dyn QueueEventEmitter>,
    /// Shared database connection for cancellation checks.
    pub(crate) db: Arc<Mutex<Connection>>,
}

#[allow(deprecated)]
impl JobContext {
    /// Create a context for direct (non-queued) job execution.
    ///
    /// This creates a lightweight context backed by an in-memory SQLite database
    /// and a no-op event emitter. Useful for CLI tools that execute jobs directly
    /// without going through the queue infrastructure.
    pub fn new_direct(job_id: &str) -> Self {
        let conn = Connection::open_in_memory().expect("in-memory DB");
        let _ = conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS queue_jobs (
                id TEXT PRIMARY KEY,
                priority INTEGER DEFAULT 2,
                status TEXT DEFAULT 'processing',
                data_json TEXT NOT NULL DEFAULT '{}',
                created_at DATETIME DEFAULT CURRENT_TIMESTAMP,
                started_at DATETIME,
                completed_at DATETIME,
                error_message TEXT
            )",
        );
        Self {
            job_id: job_id.to_string(),
            trace_id: None,
            trace_ctx: None,
            attempt_id: Some(stack_ids::AttemptId::generate()),
            trial_id: Some(stack_ids::TrialId::generate()),
            worker_id: None,
            attempt_count: 1,
            event_emitter: Arc::new(NoopEventEmitter),
            db: Arc::new(Mutex::new(conn)),
        }
    }

    /// Emit a progress event.
    ///
    /// # Arguments
    /// * `current` - Current step number
    /// * `total` - Total number of steps
    pub fn emit_progress(&self, current: u32, total: u32) {
        self.event_emitter
            .emit_job_progress(events::JobProgressEvent {
                job_id: self.job_id.clone(),
                current_step: current,
                total_steps: total,
                progress: if total > 0 {
                    current as f64 / total as f64
                } else {
                    0.0
                },
                trace_id: self.trace_id.clone(),
                worker_id: self.worker_id.clone(),
                attempt_count: Some(self.attempt_count),
                status: Some("processing".to_string()),
                trace_ctx: self.trace_ctx.clone(),
                attempt_id: self.attempt_id.clone(),
                trial_id: self.trial_id.clone(),
            });
    }

    /// Check if this job has been cancelled.
    ///
    /// Call this periodically during long-running jobs to support
    /// cooperative cancellation. If it returns `true`, your handler
    /// should return `Err(QueueError::Cancelled)`.
    pub fn is_cancelled(&self) -> bool {
        match self.db.lock() {
            Ok(conn) => db::is_cancelled(&conn, &self.job_id).unwrap_or(false),
            Err(_) => false,
        }
    }
}

/// Trait that job types must implement to be processed by the queue.
///
/// Your job type must be serializable (stored as JSON in SQLite),
/// cloneable, and thread-safe.
///
/// # Example
///
/// ```ignore
/// use serde::{Serialize, Deserialize};
/// use job_queue::*;
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct EmailJob {
///     to: String,
///     subject: String,
/// }
///
/// impl JobHandler for EmailJob {
///     async fn execute(&self, ctx: &JobContext) -> Result<JobResult, QueueError> {
///         // Send email...
///         ctx.emit_progress(1, 1);
///         Ok(JobResult::success())
///     }
/// }
/// ```
pub trait JobHandler: Send + Sync + serde::Serialize + serde::de::DeserializeOwned + Clone {
    /// Execute the job. This is called by the executor when the job is picked up.
    ///
    /// Use `ctx.emit_progress()` to report progress and `ctx.is_cancelled()`
    /// to check for cancellation during long-running operations.
    fn execute(
        &self,
        ctx: &JobContext,
    ) -> impl std::future::Future<Output = Result<JobResult, QueueError>> + Send;

    /// Optional: a human-readable name for this job type, used in logging.
    fn job_type(&self) -> &str {
        std::any::type_name::<Self>()
    }
}
