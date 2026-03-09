#![allow(deprecated)] // Constructs events with legacy trace_id/attempt_count fields during migration

use crate::{
    config::QueueConfig, db, error::QueueError, events::*, types::JobResult, JobContext,
    JobHandler, QueueEventEmitter,
};
use rusqlite::Connection;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Result of processing a single job via [`QueueExecutor::process_one`].
#[derive(Debug)]
pub struct ProcessedJob {
    /// The job ID.
    pub job_id: String,
    /// The raw job data as JSON (can be deserialized into the handler type).
    pub job_data: serde_json::Value,
    /// Whether the job succeeded.
    pub success: bool,
    /// Output from the job (if any).
    pub output: Option<String>,
    /// Error message (if the job failed).
    pub error: Option<String>,
}

/// The background job executor.
///
/// Polls the database for pending jobs and processes them using the
/// registered [`JobHandler`] implementation. Supports pause/resume,
/// consecutive job limits with cooldown, graceful shutdown, and cancellation.
pub struct QueueExecutor {
    config: QueueConfig,
    pub(crate) db: Arc<Mutex<Connection>>,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
}

impl QueueExecutor {
    pub fn new(config: QueueConfig, db: Arc<Mutex<Connection>>) -> Self {
        Self {
            config,
            db,
            paused: Arc::new(AtomicBool::new(false)),
            shutdown: Arc::new(AtomicBool::new(false)),
        }
    }

    async fn job_details(
        &self,
        job_id: &str,
    ) -> Result<Option<crate::types::QueueJobDetails>, QueueError> {
        let job_id = job_id.to_string();
        self.with_db(move |conn| db::get_job_details(conn, &job_id))
            .await
    }

    /// Helper to run a synchronous DB operation on a blocking thread.
    async fn with_db<F, T>(&self, f: F) -> Result<T, QueueError>
    where
        F: FnOnce(&Connection) -> Result<T, anyhow::Error> + Send + 'static,
        T: Send + 'static,
    {
        let db = Arc::clone(&self.db);
        tokio::task::spawn_blocking(move || {
            let conn = db.lock().map_err(|e| anyhow::anyhow!(e.to_string()))?;
            f(&conn)
        })
        .await
        .map_err(|e| QueueError::Other(e.to_string()))?
        .map_err(QueueError::from)
    }

    async fn persist_canonical_lineage_or_fail(
        &self,
        event_emitter: &Arc<dyn QueueEventEmitter>,
        job_id: &str,
        trace_id: Option<String>,
        attempt_count: u32,
        trace_ctx: Option<stack_ids::TraceCtx>,
        attempt_id: Option<stack_ids::AttemptId>,
        trial_id: Option<stack_ids::TrialId>,
    ) -> Result<(), QueueError> {
        let jid = job_id.to_string();
        let aid = attempt_id.as_ref().map(|a| a.as_str().to_string());
        let tid = trial_id.as_ref().map(|t| t.as_str().to_string());
        let worker_id = self.config.worker_id.clone();

        let persistence_error = match self
            .with_db(move |conn| {
                db::update_canonical_lineage(conn, &jid, aid.as_deref(), tid.as_deref())
            })
            .await
        {
            Ok(true) => return Ok(()),
            Ok(false) => format!(
                "Canonical lineage persistence failed before execution: job '{}' disappeared before attempt_id/trial_id could be stored",
                job_id
            ),
            Err(error) => format!(
                "Canonical lineage persistence failed before execution: {error}"
            ),
        };

        let jid = job_id.to_string();
        let error_for_db = persistence_error.clone();
        self.with_db(move |conn| {
            let marked = db::mark_failed_owned(conn, &jid, Some(&worker_id), &error_for_db)?;
            if marked {
                Ok(())
            } else {
                Err(anyhow::anyhow!(
                    "job could not be marked failed after canonical lineage persistence error"
                ))
            }
        })
        .await?;

        event_emitter.emit_job_failed(JobFailedEvent {
            job_id: job_id.to_string(),
            error: persistence_error.clone(),
            trace_id,
            worker_id: Some(self.config.worker_id.clone()),
            attempt_count: Some(attempt_count),
            status: Some("failed".to_string()),
            failure_class: Some("permanent".to_string()),
            next_retry_at: None,
            trace_ctx,
            attempt_id,
            trial_id,
        });

        Err(QueueError::Other(persistence_error))
    }

    /// Spawn the executor loop as a background tokio task.
    ///
    /// The executor will poll for pending jobs at the configured interval
    /// and process them using the provided `JobHandler` implementation.
    ///
    /// If the current thread is inside a tokio runtime, the loop is spawned
    /// directly via `tokio::spawn`. Otherwise (e.g., during Tauri's synchronous
    /// `setup()` phase), a dedicated background thread with its own single-threaded
    /// tokio runtime is created automatically.
    pub fn spawn<H>(self: Arc<Self>, event_emitter: Arc<dyn QueueEventEmitter>)
    where
        H: JobHandler + 'static,
    {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn(async move {
                    self.run_loop::<H>(event_emitter).await;
                });
            }
            Err(_) => {
                tracing::debug!(
                    "No tokio runtime on current thread, spawning queue executor on a new thread"
                );
                std::thread::Builder::new()
                    .name("queue-executor".into())
                    .spawn(move || {
                        let rt = tokio::runtime::Builder::new_current_thread()
                            .enable_all()
                            .build()
                            .expect("queue executor: failed to create tokio runtime");
                        rt.block_on(self.run_loop::<H>(event_emitter));
                    })
                    .expect("queue executor: failed to spawn thread");
            }
        }
    }

    /// Spawn the executor loop on a specific tokio runtime handle.
    ///
    /// Use this instead of [`spawn()`](Self::spawn) when you have an explicit
    /// runtime handle (e.g., from `tauri::async_runtime::handle()`).
    pub fn spawn_on<H>(
        self: Arc<Self>,
        event_emitter: Arc<dyn QueueEventEmitter>,
        handle: &tokio::runtime::Handle,
    ) where
        H: JobHandler + 'static,
    {
        handle.spawn(async move {
            self.run_loop::<H>(event_emitter).await;
        });
    }

    async fn run_loop<H>(&self, event_emitter: Arc<dyn QueueEventEmitter>)
    where
        H: JobHandler,
    {
        let mut consecutive_count: u32 = 0;

        loop {
            // Check shutdown flag
            if self.shutdown.load(Ordering::Relaxed) {
                tracing::info!("Queue executor shutting down");
                break;
            }

            // Check if paused
            if self.paused.load(Ordering::Relaxed) {
                tokio::time::sleep(self.config.poll_interval).await;
                continue;
            }

            let stale_after_secs = self.config.stale_after.as_secs();
            if stale_after_secs > 0 {
                match self
                    .with_db(move |conn| db::reclaim_stale(conn, stale_after_secs))
                    .await
                {
                    Ok(reclaimed) if reclaimed > 0 => {
                        tracing::warn!(reclaimed, "Reclaimed stale queue jobs");
                    }
                    Ok(_) => {}
                    Err(e) => tracing::error!("Failed to reclaim stale jobs: {:#}", e),
                }
            }

            // Check consecutive limit
            if self.config.max_consecutive > 0 && consecutive_count >= self.config.max_consecutive {
                tracing::info!(
                    max_consecutive = self.config.max_consecutive,
                    cooldown = ?self.config.cooldown,
                    "Consecutive limit reached, cooling down"
                );
                tokio::time::sleep(self.config.cooldown).await;
                consecutive_count = 0;
                continue;
            }

            let worker_id = self.config.worker_id.clone();
            let visibility_timeout_secs = self.config.stale_after.as_secs();
            let claimed = match self
                .with_db(move |conn| {
                    db::claim_with_lease(conn, &worker_id, visibility_timeout_secs)
                })
                .await
            {
                Ok(Some(job)) => job,
                Ok(None) => {
                    consecutive_count = 0;
                    tokio::time::sleep(self.config.poll_interval).await;
                    continue;
                }
                Err(e) => {
                    tracing::error!("Failed to claim next job: {:#}", e);
                    tokio::time::sleep(self.config.poll_interval).await;
                    continue;
                }
            };

            let (job_id, job_data) = claimed;
            let job_details = self.job_details(&job_id).await.ok().flatten();
            let trace_id = job_details
                .as_ref()
                .and_then(|details| details.trace_id.clone());
            let attempt_count = job_details
                .as_ref()
                .map(|details| details.attempt_count)
                .unwrap_or(1);

            // Build canonical trace/retry primitives for this execution
            let trace_ctx = trace_id
                .as_ref()
                .map(|id| stack_ids::TraceCtx::from_legacy_trace_id(id));
            // AttemptId: prefer persisted value from DB, synthesize only for pre-v4 rows
            let attempt_id = job_details
                .as_ref()
                .and_then(|d| d.attempt_id.as_ref())
                .map(|id| stack_ids::AttemptId::new(id.clone()))
                .or_else(|| {
                    Some(stack_ids::AttemptId::new(format!(
                        "{}-attempt-{}",
                        job_id, attempt_count
                    )))
                });
            // TrialId: always fresh per concrete execution
            let trial_id = Some(stack_ids::TrialId::generate());

            if let Err(error) = self
                .persist_canonical_lineage_or_fail(
                    &event_emitter,
                    &job_id,
                    trace_id.clone(),
                    attempt_count,
                    trace_ctx.clone(),
                    attempt_id.clone(),
                    trial_id.clone(),
                )
                .await
            {
                tracing::error!(
                    job_id = %job_id,
                    error = %error,
                    "Failed to persist canonical lineage"
                );
                consecutive_count = 0;
                continue;
            }

            // Count every attempt toward throttling (not just successes)
            consecutive_count += 1;

            // Deserialize job data into the handler type
            let job_handler: H = match serde_json::from_value(job_data) {
                Ok(h) => h,
                Err(e) => {
                    tracing::error!(job_id = %job_id, "Failed to deserialize job: {}", e);
                    let err_msg = format!("Deserialization failed: {}", e);
                    let jid = job_id.clone();
                    let msg = err_msg.clone();
                    let worker_id = self.config.worker_id.clone();
                    let _ = self
                        .with_db(move |conn| {
                            db::mark_failed_owned(conn, &jid, Some(&worker_id), &msg).map(|_| ())
                        })
                        .await;
                    event_emitter.emit_job_failed(JobFailedEvent {
                        job_id,
                        error: err_msg,
                        trace_id,
                        worker_id: Some(self.config.worker_id.clone()),
                        attempt_count: Some(attempt_count),
                        status: Some("failed".to_string()),
                        failure_class: Some("permanent".to_string()),
                        next_retry_at: None,
                        trace_ctx,
                        attempt_id,
                        trial_id,
                    });
                    continue;
                }
            };

            // Process the job — process_job owns all DB transitions and events
            let _result = self
                .process_job::<H>(
                    &event_emitter,
                    &job_id,
                    job_handler,
                    trace_id,
                    attempt_count,
                    trace_ctx,
                    attempt_id,
                    trial_id,
                )
                .await;

            if self.config.cooldown.as_secs() > 0 {
                tokio::time::sleep(self.config.cooldown).await;
            }
        }
    }

    async fn process_job<H>(
        &self,
        event_emitter: &Arc<dyn QueueEventEmitter>,
        job_id: &str,
        job_handler: H,
        trace_id: Option<String>,
        attempt_count: u32,
        trace_ctx: Option<stack_ids::TraceCtx>,
        attempt_id: Option<stack_ids::AttemptId>,
        trial_id: Option<stack_ids::TrialId>,
    ) -> Result<JobResult, QueueError>
    where
        H: JobHandler,
    {
        let worker_id = self.config.worker_id.clone();
        event_emitter.emit_job_started(JobStartedEvent {
            job_id: job_id.to_string(),
            trace_id: trace_id.clone(),
            worker_id: Some(worker_id.clone()),
            attempt_count: Some(attempt_count),
            status: Some("processing".to_string()),
            trace_ctx: trace_ctx.clone(),
            attempt_id: attempt_id.clone(),
            trial_id: trial_id.clone(),
        });

        // Create job context with DB reference for cancellation checks
        let ctx = JobContext {
            job_id: job_id.to_string(),
            trace_id: trace_id.clone(),
            trace_ctx: trace_ctx.clone(),
            attempt_id: attempt_id.clone(),
            trial_id: trial_id.clone(),
            worker_id: Some(worker_id.clone()),
            attempt_count,
            event_emitter: Arc::clone(event_emitter),
            db: Arc::clone(&self.db),
        };

        let heartbeat_stop = Arc::new(AtomicBool::new(false));
        if !self.config.heartbeat_interval.is_zero() {
            let db = Arc::clone(&self.db);
            let job_id = job_id.to_string();
            let worker_id = worker_id.clone();
            let stop = Arc::clone(&heartbeat_stop);
            let heartbeat_interval = self.config.heartbeat_interval;
            tokio::spawn(async move {
                while !stop.load(Ordering::Relaxed) {
                    tokio::time::sleep(heartbeat_interval).await;
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let db = Arc::clone(&db);
                    let job_id = job_id.clone();
                    let worker_id = worker_id.clone();
                    let _ = tokio::task::spawn_blocking(move || {
                        if let Ok(conn) = db.lock() {
                            let _ = db::heartbeat(&conn, &job_id, &worker_id);
                        }
                    })
                    .await;
                }
            });
        }

        // Execute job
        let result = job_handler.execute(&ctx).await;
        heartbeat_stop.store(true, Ordering::Relaxed);

        // Check cancellation before deciding what to write to DB
        let jid = job_id.to_string();
        let cancelled = self
            .with_db(move |conn| db::is_cancelled(conn, &jid))
            .await
            .unwrap_or(false);

        match &result {
            _ if cancelled || matches!(&result, Err(QueueError::Cancelled)) => {
                // Job was cancelled — it may already be marked cancelled in DB.
                // Emit the event and return.
                event_emitter.emit_job_cancelled(JobCancelledEvent {
                    job_id: job_id.to_string(),
                    trace_id: trace_id.clone(),
                    worker_id: Some(worker_id.clone()),
                    attempt_count: Some(attempt_count),
                    status: Some("cancelled".to_string()),
                    trace_ctx: trace_ctx.clone(),
                    attempt_id: attempt_id.clone(),
                    trial_id: trial_id.clone(),
                });
                Err(QueueError::Cancelled)
            }
            Ok(job_result) if job_result.success => {
                let jid = job_id.to_string();
                let worker_id_for_update = worker_id.clone();
                let updated = self
                    .with_db(move |conn| {
                        db::mark_completed_owned(conn, &jid, Some(&worker_id_for_update))
                    })
                    .await?;

                if updated {
                    event_emitter.emit_job_completed(JobCompletedEvent {
                        job_id: job_id.to_string(),
                        output: job_result.output.clone(),
                        trace_id: trace_id.clone(),
                        worker_id: Some(worker_id.clone()),
                        attempt_count: Some(attempt_count),
                        status: Some("completed".to_string()),
                        trace_ctx: trace_ctx.clone(),
                        attempt_id: attempt_id.clone(),
                        trial_id: trial_id.clone(),
                    });
                } else {
                    // Status changed underneath us (cancelled between execute and here)
                    event_emitter.emit_job_cancelled(JobCancelledEvent {
                        job_id: job_id.to_string(),
                        trace_id: trace_id.clone(),
                        worker_id: Some(worker_id.clone()),
                        attempt_count: Some(attempt_count),
                        status: Some("cancelled".to_string()),
                        trace_ctx: trace_ctx.clone(),
                        attempt_id: attempt_id.clone(),
                        trial_id: trial_id.clone(),
                    });
                }
                Ok(job_result.clone())
            }
            Ok(job_result) => {
                let error_msg = job_result
                    .error
                    .clone()
                    .unwrap_or_else(|| "Unknown error".to_string());
                let jid = job_id.to_string();
                let msg = error_msg.clone();
                let failure_class = job_result
                    .failure_class
                    .clone()
                    .unwrap_or(crate::types::FailureClass::Permanent);
                let failure_class_for_update = failure_class.clone();
                let worker_id_for_update = worker_id.clone();
                let max_retries = self.config.max_retries;
                let updated = self
                    .with_db(move |conn| {
                        db::mark_failed_with_retry_owned(
                            conn,
                            &jid,
                            Some(&worker_id_for_update),
                            &msg,
                            &failure_class_for_update,
                            max_retries,
                        )
                    })
                    .await?;
                let details = self.job_details(job_id).await.ok().flatten();

                if updated {
                    event_emitter.emit_job_failed(JobFailedEvent {
                        job_id: job_id.to_string(),
                        error: error_msg,
                        trace_id: trace_id.clone(),
                        worker_id: Some(worker_id.clone()),
                        attempt_count: Some(attempt_count),
                        status: details
                            .as_ref()
                            .map(|details| details.status.as_str().to_string()),
                        failure_class: Some(failure_class.as_str().to_string()),
                        next_retry_at: details.and_then(|details| details.next_run_at),
                        trace_ctx: trace_ctx.clone(),
                        attempt_id: attempt_id.clone(),
                        trial_id: trial_id.clone(),
                    });
                } else {
                    event_emitter.emit_job_cancelled(JobCancelledEvent {
                        job_id: job_id.to_string(),
                        trace_id: trace_id.clone(),
                        worker_id: Some(worker_id.clone()),
                        attempt_count: Some(attempt_count),
                        status: Some("cancelled".to_string()),
                        trace_ctx: trace_ctx.clone(),
                        attempt_id: attempt_id.clone(),
                        trial_id: trial_id.clone(),
                    });
                }
                Ok(job_result.clone())
            }
            Err(e) => {
                let error_msg = e.to_string();
                let jid = job_id.to_string();
                let msg = error_msg.clone();
                let worker_id_for_update = worker_id.clone();
                let updated = self
                    .with_db(move |conn| {
                        db::mark_failed_owned(conn, &jid, Some(&worker_id_for_update), &msg)
                    })
                    .await?;

                if updated {
                    event_emitter.emit_job_failed(JobFailedEvent {
                        job_id: job_id.to_string(),
                        error: error_msg.clone(),
                        trace_id: trace_id.clone(),
                        worker_id: Some(worker_id.clone()),
                        attempt_count: Some(attempt_count),
                        status: Some("failed".to_string()),
                        failure_class: Some("permanent".to_string()),
                        next_retry_at: None,
                        trace_ctx: trace_ctx.clone(),
                        attempt_id: attempt_id.clone(),
                        trial_id: trial_id.clone(),
                    });
                } else {
                    event_emitter.emit_job_cancelled(JobCancelledEvent {
                        job_id: job_id.to_string(),
                        trace_id: trace_id.clone(),
                        worker_id: Some(worker_id),
                        attempt_count: Some(attempt_count),
                        status: Some("cancelled".to_string()),
                        trace_ctx,
                        attempt_id,
                        trial_id,
                    });
                }
                Err(QueueError::Execution(error_msg))
            }
        }
    }

    /// Pause the executor. The current job (if any) will finish,
    /// but no new jobs will be started until [`resume()`](Self::resume) is called.
    pub fn pause(&self) {
        self.paused.store(true, Ordering::Relaxed);
    }

    /// Resume the executor after a pause.
    pub fn resume(&self) {
        self.paused.store(false, Ordering::Relaxed);
    }

    /// Check if the executor is currently paused.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Signal the executor to shut down gracefully.
    ///
    /// The currently running job (if any) will finish, then the loop exits.
    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }

    /// Check if a shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.shutdown.load(Ordering::Relaxed)
    }

    /// Process the next pending job and return the result.
    ///
    /// Unlike [`spawn()`](Self::spawn), this method processes exactly one job
    /// in the foreground and returns. Returns `Ok(None)` if no pending jobs
    /// are available.
    ///
    /// This is useful for CLI tools that want to drive the execution loop
    /// manually (e.g., to run cascade logic between jobs).
    pub async fn process_one<H>(
        &self,
        event_emitter: &Arc<dyn QueueEventEmitter>,
    ) -> Result<Option<ProcessedJob>, QueueError>
    where
        H: JobHandler,
    {
        let worker_id = self.config.worker_id.clone();
        let visibility_timeout_secs = self.config.stale_after.as_secs();
        let claimed = self
            .with_db(move |conn| db::claim_with_lease(conn, &worker_id, visibility_timeout_secs))
            .await?;

        let (job_id, job_data) = match claimed {
            Some(job) => job,
            None => return Ok(None),
        };
        let job_details = self.job_details(&job_id).await.ok().flatten();
        let trace_id = job_details
            .as_ref()
            .and_then(|details| details.trace_id.clone());
        let attempt_count = job_details
            .as_ref()
            .map(|details| details.attempt_count)
            .unwrap_or(1);

        // Build canonical trace/retry primitives for this execution
        let trace_ctx = trace_id
            .as_ref()
            .map(|id| stack_ids::TraceCtx::from_legacy_trace_id(id));
        // AttemptId: prefer persisted value from DB, synthesize only for pre-v4 rows
        let attempt_id = job_details
            .as_ref()
            .and_then(|d| d.attempt_id.as_ref())
            .map(|id| stack_ids::AttemptId::new(id.clone()))
            .or_else(|| {
                Some(stack_ids::AttemptId::new(format!(
                    "{}-attempt-{}",
                    job_id, attempt_count
                )))
            });
        // TrialId: always fresh per concrete execution
        let trial_id = Some(stack_ids::TrialId::generate());

        self.persist_canonical_lineage_or_fail(
            event_emitter,
            &job_id,
            trace_id.clone(),
            attempt_count,
            trace_ctx.clone(),
            attempt_id.clone(),
            trial_id.clone(),
        )
        .await?;

        // Deserialize and execute
        let job_handler: H = serde_json::from_value(job_data.clone()).map_err(|e| {
            // Mark as failed in DB (best-effort, fire-and-forget via blocking)
            let jid = job_id.clone();
            let msg = format!("Deserialization failed: {e}");
            let db = Arc::clone(&self.db);
            let worker_id = self.config.worker_id.clone();
            let _ = std::thread::spawn(move || {
                if let Ok(conn) = db.lock() {
                    let _ = db::mark_failed_owned(&conn, &jid, Some(&worker_id), &msg);
                }
            });
            QueueError::Other(format!("Deserialization failed: {e}"))
        })?;

        // process_job owns all status transitions and event emissions
        let result = self
            .process_job::<H>(
                event_emitter,
                &job_id,
                job_handler,
                trace_id,
                attempt_count,
                trace_ctx,
                attempt_id,
                trial_id,
            )
            .await;

        match result {
            Ok(job_result) => Ok(Some(ProcessedJob {
                job_id,
                job_data,
                success: job_result.success,
                output: job_result.output,
                error: job_result.error,
            })),
            Err(QueueError::Cancelled) => Ok(Some(ProcessedJob {
                job_id,
                job_data,
                success: false,
                output: None,
                error: Some("Cancelled".to_string()),
            })),
            Err(e) => Ok(Some(ProcessedJob {
                job_id,
                job_data,
                success: false,
                output: None,
                error: Some(e.to_string()),
            })),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db, JobContext, JobHandler, JobResult, QueueConfig, QueueEventEmitter};
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingEmitter {
        failed: Mutex<Vec<JobFailedEvent>>,
    }

    impl QueueEventEmitter for RecordingEmitter {
        fn emit_job_started(&self, _event: JobStartedEvent) {}
        fn emit_job_completed(&self, _event: JobCompletedEvent) {}
        fn emit_job_failed(&self, event: JobFailedEvent) {
            self.failed.lock().unwrap().push(event);
        }
        fn emit_job_progress(&self, _event: JobProgressEvent) {}
        fn emit_job_cancelled(&self, _event: JobCancelledEvent) {}
    }

    static EXECUTION_CALLS: AtomicUsize = AtomicUsize::new(0);

    #[derive(Clone, Serialize, Deserialize)]
    struct CountingJob;

    impl JobHandler for CountingJob {
        async fn execute(&self, _ctx: &JobContext) -> Result<JobResult, QueueError> {
            EXECUTION_CALLS.fetch_add(1, Ordering::SeqCst);
            Ok(JobResult::success())
        }
    }

    #[tokio::test]
    async fn lineage_persistence_failure_aborts_execution_and_marks_job_failed() {
        EXECUTION_CALLS.store(0, Ordering::SeqCst);

        let conn = db::open_database(None).unwrap();
        db::insert_job(&conn, "job-lineage-fail", 2, &serde_json::json!({})).unwrap();
        conn.execute_batch(
            "CREATE TRIGGER fail_lineage_update
             BEFORE UPDATE OF attempt_id, trial_id ON queue_jobs
             BEGIN
                 SELECT RAISE(FAIL, 'lineage write blocked');
             END;",
        )
        .unwrap();

        let db = Arc::new(Mutex::new(conn));
        let config = QueueConfig::builder().with_worker_id("worker-test").build();
        let executor = QueueExecutor::new(config, db.clone());
        let emitter = Arc::new(RecordingEmitter::default());
        let emitter_trait: Arc<dyn QueueEventEmitter> = emitter.clone();

        let err = executor
            .process_one::<CountingJob>(&emitter_trait)
            .await
            .unwrap_err();
        assert!(
            err.to_string()
                .contains("Canonical lineage persistence failed before execution"),
            "expected explicit lineage failure, got: {err}"
        );
        assert_eq!(
            EXECUTION_CALLS.load(Ordering::SeqCst),
            0,
            "job handler must not run when canonical lineage persistence fails"
        );

        let conn = db.lock().unwrap();
        let details = db::get_job_details(&conn, "job-lineage-fail")
            .unwrap()
            .expect("job details");
        assert_eq!(details.status.as_str(), "failed");
        assert!(
            details
                .error_message
                .as_deref()
                .unwrap_or_default()
                .contains("Canonical lineage persistence failed before execution"),
            "failure must be persisted on the job row"
        );

        let failed = emitter.failed.lock().unwrap();
        assert_eq!(failed.len(), 1, "executor must emit a hard failure event");
        assert!(failed[0].attempt_id.is_some());
        assert!(failed[0].trial_id.is_some());
        assert!(
            failed[0]
                .error
                .contains("Canonical lineage persistence failed before execution"),
            "event must surface the lineage persistence error"
        );
    }
}
