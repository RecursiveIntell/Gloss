use crate::{
    config::QueueConfig,
    db,
    error::QueueError,
    executor::{ProcessedJob, QueueExecutor},
    types::{QueueJob, QueueJobDetails, QueuePriority, QueueStats},
    JobHandler, QueueEventEmitter,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

/// High-level queue manager providing the public API.
///
/// Create a `QueueManager`, add jobs to it, then call [`spawn()`](Self::spawn)
/// to start the background executor that processes them.
///
/// # Example
///
/// ```ignore
/// let config = QueueConfig::builder()
///     .with_db_path(PathBuf::from("queue.db"))
///     .build();
///
/// let manager = QueueManager::new(config).unwrap();
/// let job = QueueJob::new(MyJob { ... });
/// manager.add(job).unwrap();
///
/// manager.spawn::<MyJob>(Arc::new(LoggingEventEmitter));
/// ```
pub struct QueueManager {
    db: Arc<Mutex<Connection>>,
    executor: Arc<QueueExecutor>,
    config: QueueConfig,
}

impl QueueManager {
    /// Create a new queue manager with the given configuration.
    ///
    /// Opens (or creates) the SQLite database and requeues any jobs that
    /// were interrupted by a previous crash.
    pub fn new(config: QueueConfig) -> Result<Self, QueueError> {
        let conn = db::open_database(config.db_path.as_deref())?;

        // Requeue interrupted jobs from a previous crash
        let requeued = db::requeue_interrupted(&conn)?;
        if requeued > 0 {
            tracing::info!(count = requeued, "Requeued interrupted jobs");
        }

        let db = Arc::new(Mutex::new(conn));
        let executor = Arc::new(QueueExecutor::new(config.clone(), Arc::clone(&db)));

        Ok(Self {
            db,
            executor,
            config,
        })
    }

    /// Add a job to the queue. Returns the job ID.
    pub fn add<H>(&self, job: QueueJob<H>) -> Result<String, QueueError>
    where
        H: JobHandler,
    {
        let conn = self.db.lock()?;
        let data = serde_json::to_value(&job.data)?;
        db::insert_job_full(
            &conn,
            &job.id,
            job.priority.as_i32(),
            &data,
            job.trace_id.as_deref(),
            job.attempt_id.as_ref().map(|a| a.as_str()),
            job.trial_id.as_ref().map(|t| t.as_str()),
        )?;
        Ok(job.id)
    }

    /// Cancel a pending or processing job by ID.
    pub fn cancel(&self, job_id: &str) -> Result<(), QueueError> {
        let conn = self.db.lock()?;
        db::cancel_job(&conn, job_id)?;
        Ok(())
    }

    /// Reorder a pending job to a new priority.
    pub fn reorder(&self, job_id: &str, new_priority: QueuePriority) -> Result<(), QueueError> {
        let conn = self.db.lock()?;

        match db::reorder_pending(&conn, job_id, new_priority.as_i32()) {
            Ok(true) => Ok(()),
            Ok(false) => {
                // Job exists but is not pending — fetch its status for the error message
                let status = db::get_job(&conn, job_id)
                    .ok()
                    .flatten()
                    .map(|j| j.2)
                    .unwrap_or_else(|| "unknown".to_string());
                Err(QueueError::Other(format!(
                    "Can only reorder pending jobs (job {} is {})",
                    job_id, status
                )))
            }
            Err(_) => Err(QueueError::NotFound(job_id.to_string())),
        }
    }

    /// Pause the queue. The current job will finish, but no new jobs start.
    pub fn pause(&self) {
        self.executor.pause();
    }

    /// Resume the queue after a pause.
    pub fn resume(&self) {
        self.executor.resume();
    }

    /// Check if the queue is currently paused.
    pub fn is_paused(&self) -> bool {
        self.executor.is_paused()
    }

    /// Get all jobs as `(id, status)` pairs, ordered by status then priority.
    pub fn list_jobs(&self) -> Result<Vec<(String, String)>, QueueError> {
        let conn = self.db.lock()?;
        let jobs = db::list_all_jobs(&conn)?;
        Ok(jobs
            .into_iter()
            .map(|(id, status, _)| (id, status))
            .collect())
    }

    /// Get all jobs as `(id, status, data_json)` tuples.
    pub fn list_jobs_with_data(&self) -> Result<Vec<(String, String, String)>, QueueError> {
        let conn = self.db.lock()?;
        Ok(db::list_all_jobs(&conn)?)
    }

    /// Fetch a structured view of a single job for debugging or UI inspection.
    pub fn get_job_details(&self, job_id: &str) -> Result<Option<QueueJobDetails>, QueueError> {
        let conn = self.db.lock()?;
        Ok(db::get_job_details(&conn, job_id)?)
    }

    /// Prune completed/failed/cancelled jobs older than `days`.
    /// Returns the number of jobs deleted.
    pub fn prune(&self, days: u32) -> Result<u32, QueueError> {
        let conn = self.db.lock()?;
        Ok(db::prune_old_jobs(&conn, days)?)
    }

    /// Process the next pending job in the foreground and return the result.
    ///
    /// Returns `Ok(None)` if no pending jobs are available.
    /// Unlike [`spawn()`](Self::spawn), this does not start a background loop —
    /// it processes exactly one job and returns. Useful for CLI-driven loops
    /// where cascade logic runs between jobs.
    pub async fn process_one<H>(
        &self,
        event_emitter: &Arc<dyn QueueEventEmitter>,
    ) -> Result<Option<ProcessedJob>, QueueError>
    where
        H: JobHandler,
    {
        self.executor.process_one::<H>(event_emitter).await
    }

    /// Count jobs by status.
    pub fn count_by_status(&self) -> Result<QueueStats, QueueError> {
        let conn = self.db.lock()?;
        Ok(db::count_by_status(&conn)?)
    }

    /// Signal the executor to shut down gracefully.
    ///
    /// The currently running job (if any) will finish, then the background
    /// loop exits. This is safe to call multiple times.
    pub fn shutdown(&self) {
        self.executor.shutdown();
    }

    /// Check if a shutdown has been requested.
    pub fn is_shutdown(&self) -> bool {
        self.executor.is_shutdown()
    }

    /// Returns the configured worker identifier.
    pub fn worker_id(&self) -> &str {
        &self.config.worker_id
    }

    /// Spawn the background executor and return the manager wrapped in an `Arc`.
    ///
    /// The event emitter receives notifications about job lifecycle events.
    ///
    /// If the current thread is inside a tokio runtime, the executor loop is
    /// spawned directly. Otherwise (e.g., during Tauri's synchronous `setup()`
    /// phase), a dedicated background thread is created automatically.
    pub fn spawn<H>(self, event_emitter: Arc<dyn QueueEventEmitter>) -> Arc<Self>
    where
        H: JobHandler + 'static,
    {
        let manager = Arc::new(self);
        let executor = Arc::clone(&manager.executor);
        executor.spawn::<H>(event_emitter);
        manager
    }

    /// Spawn the background executor on a specific tokio runtime handle.
    ///
    /// Use this instead of [`spawn()`](Self::spawn) when you have an explicit
    /// runtime handle (e.g., from `tauri::async_runtime::handle()`).
    pub fn spawn_on<H>(
        self,
        event_emitter: Arc<dyn QueueEventEmitter>,
        handle: &tokio::runtime::Handle,
    ) -> Arc<Self>
    where
        H: JobHandler + 'static,
    {
        let manager = Arc::new(self);
        let executor = Arc::clone(&manager.executor);
        executor.spawn_on::<H>(event_emitter, handle);
        manager
    }
}
