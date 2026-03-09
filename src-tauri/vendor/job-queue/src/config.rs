use std::path::PathBuf;
use std::time::Duration;

/// Configuration for the queue system.
///
/// Use [`QueueConfig::builder()`] for ergonomic construction, or
/// [`QueueConfig::default()`] for sensible defaults (in-memory DB, no cooldown).
#[derive(Debug, Clone)]
pub struct QueueConfig {
    /// Path to SQLite database file. `None` = in-memory database.
    pub db_path: Option<PathBuf>,

    /// Stable worker identifier used for leases, heartbeats, and diagnostics.
    pub worker_id: String,

    /// Cooldown duration between job executions (0 = no cooldown).
    pub cooldown: Duration,

    /// Maximum consecutive jobs before a forced cooldown (0 = unlimited).
    pub max_consecutive: u32,

    /// Polling interval for checking pending jobs.
    pub poll_interval: Duration,

    /// Interval between lease heartbeats while a job is running.
    pub heartbeat_interval: Duration,

    /// Visibility timeout / stale lease threshold for reclaiming jobs.
    pub stale_after: Duration,

    /// Maximum retry attempts for retryable failures.
    pub max_retries: u32,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            db_path: None,
            worker_id: format!("worker-{}", uuid::Uuid::new_v4()),
            cooldown: Duration::from_secs(0),
            max_consecutive: 0,
            poll_interval: Duration::from_secs(3),
            heartbeat_interval: Duration::from_secs(10),
            stale_after: Duration::from_secs(300),
            max_retries: 3,
        }
    }
}

impl QueueConfig {
    /// Start building a config with the builder pattern.
    pub fn builder() -> QueueConfigBuilder {
        QueueConfigBuilder::default()
    }
}

/// Builder for [`QueueConfig`].
#[derive(Default)]
pub struct QueueConfigBuilder {
    config: QueueConfig,
}

impl QueueConfigBuilder {
    /// Set the SQLite database path for persistence. Omit for in-memory.
    pub fn with_db_path(mut self, path: PathBuf) -> Self {
        self.config.db_path = Some(path);
        self
    }

    /// Set the worker identifier used for job leases and heartbeats.
    pub fn with_worker_id(mut self, worker_id: impl Into<String>) -> Self {
        self.config.worker_id = worker_id.into();
        self
    }

    /// Set the cooldown duration between consecutive job executions.
    pub fn with_cooldown(mut self, duration: Duration) -> Self {
        self.config.cooldown = duration;
        self
    }

    /// Set the maximum consecutive jobs before a forced cooldown.
    pub fn with_max_consecutive(mut self, max: u32) -> Self {
        self.config.max_consecutive = max;
        self
    }

    /// Set the polling interval for checking pending jobs.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.config.poll_interval = interval;
        self
    }

    /// Set the heartbeat interval for running jobs.
    pub fn with_heartbeat_interval(mut self, interval: Duration) -> Self {
        self.config.heartbeat_interval = interval;
        self
    }

    /// Set the stale lease threshold for reclaiming abandoned jobs.
    pub fn with_stale_after(mut self, duration: Duration) -> Self {
        self.config.stale_after = duration;
        self
    }

    /// Set the maximum retry attempts for retryable failures.
    pub fn with_max_retries(mut self, max_retries: u32) -> Self {
        self.config.max_retries = max_retries;
        self
    }

    /// Build the final [`QueueConfig`].
    pub fn build(self) -> QueueConfig {
        self.config
    }
}
