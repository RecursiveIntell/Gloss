//! # Tauri Queue
//!
//! Tauri integration for the `job-queue` background job processing system.
//!
//! This crate provides a [`TauriEventEmitter`] that bridges job-queue events
//! to Tauri's frontend event system, plus re-exports of all core job-queue types.
//!
//! ## Quick Start
//!
//! 1. Define a job type implementing [`JobHandler`]
//! 2. Create a [`QueueManager`] with a [`QueueConfig`]
//! 3. Add jobs with [`QueueManager::add()`]
//! 4. Spawn the executor with [`QueueManager::spawn()`] using a [`TauriEventEmitter`]

// Re-export everything from job-queue for backward compatibility
pub use job_queue::config::{self, QueueConfig, QueueConfigBuilder};
pub use job_queue::db;
pub use job_queue::error::{self, QueueError};
pub use job_queue::events::{self, *};
pub use job_queue::executor;
pub use job_queue::queue::{self, QueueManager};
pub use job_queue::types::{self, JobResult, QueueJob, QueueJobStatus, QueuePriority};
pub use job_queue::{JobContext, JobHandler, QueueEventEmitter};

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter};

// Re-export canonical trace context from stack-ids
pub use stack_ids::TraceCtx;

/// Extract a canonical [`TraceCtx`] from a legacy `trace_id` string, if present.
///
/// This is the recommended way to obtain structured trace context from job-queue
/// events whose `trace_id` field is still `Option<String>`.
///
/// Phase status: current / canonical bridge utility.
pub fn trace_ctx_from_event_trace_id(trace_id: &Option<String>) -> Option<TraceCtx> {
    trace_id
        .as_deref()
        .map(TraceCtx::from_legacy_trace_id)
}

/// Policy for handling event overflow when the downstream consumer is slow.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DropPolicy {
    /// Drop the oldest pending event to make room for the new one.
    DropOldest,
    /// Drop the incoming event when the buffer is full.
    #[default]
    DropNewest,
    /// Block the emitter until space is available (not recommended in UI contexts).
    Block,
}

/// Configuration for event emission backpressure and coalescing.
///
/// When the frontend cannot consume events as fast as they are produced
/// (e.g., rapid-fire progress updates), this config controls buffering
/// and deduplication behavior.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EmitterConfig {
    /// Maximum number of pending events before the drop policy kicks in.
    /// Default: 256.
    pub buffer_size: usize,
    /// What to do when the buffer is full. Default: DropNewest.
    pub drop_policy: DropPolicy,
    /// Minimum interval between events of the same type for the same job.
    /// Events arriving faster than this interval are coalesced (only the
    /// latest value is kept). Default: 50 ms.
    pub coalesce_interval_ms: u64,
    /// Whether to include a `trace_id` field on emitted events (if available).
    /// Default: false (legacy — use `include_trace_ctx` instead).
    ///
    /// Phase status: compatibility / migration-only.
    /// The `trace_id` is a plain string extracted from `job_queue::QueueJob`.
    /// The canonical replacement is [`include_trace_ctx`](EmitterConfig::include_trace_ctx).
    /// When `include_trace_ctx` is true, trace strings are preserved regardless
    /// of this flag.
    ///
    /// **Removal condition**: removed when all UI consumers migrate to `TraceCtx`.
    #[deprecated(since = "0.3.0", note = "Use include_trace_ctx instead")]
    pub include_trace_id: bool,
    /// Whether to propagate canonical trace context on emitted events.
    /// Default: true.
    ///
    /// When true, the legacy `trace_id: Option<String>` field is preserved
    /// on events so that downstream consumers can convert it to a
    /// [`TraceCtx`] via [`trace_ctx_from_event_trace_id`].
    ///
    /// This flag is the canonical replacement for [`include_trace_id`](EmitterConfig::include_trace_id).
    /// When both flags disagree, `include_trace_ctx` takes precedence:
    /// if `include_trace_ctx` is true the trace string is kept even when
    /// `include_trace_id` is false.
    ///
    /// Phase status: current / canonical.
    pub include_trace_ctx: bool,
}

#[allow(deprecated)]
impl Default for EmitterConfig {
    fn default() -> Self {
        Self {
            buffer_size: 256,
            drop_policy: DropPolicy::default(),
            coalesce_interval_ms: 50,
            include_trace_id: false,
            include_trace_ctx: true,
        }
    }
}

impl EmitterConfig {
    /// Returns true if the trace string should be kept on events.
    ///
    /// `include_trace_ctx` takes precedence over `include_trace_id`.
    #[allow(deprecated)]
    fn should_keep_trace(&self) -> bool {
        self.include_trace_ctx || self.include_trace_id
    }
}

/// A throttled event emitter that coalesces rapid-fire events.
///
/// Wraps any [`QueueEventEmitter`] and suppresses duplicate events for the
/// same job that arrive within the configured coalesce interval.
pub struct CoalescingEmitter {
    inner: Arc<dyn QueueEventEmitter>,
    config: EmitterConfig,
    last_progress: Mutex<HashMap<String, Instant>>,
    pending_progress: Mutex<HashMap<String, JobProgressEvent>>,
    pending_order: Mutex<VecDeque<String>>,
}

impl CoalescingEmitter {
    pub fn new(inner: Arc<dyn QueueEventEmitter>, config: EmitterConfig) -> Self {
        Self {
            inner,
            config,
            last_progress: Mutex::new(HashMap::new()),
            pending_progress: Mutex::new(HashMap::new()),
            pending_order: Mutex::new(VecDeque::new()),
        }
    }

    pub fn arc(inner: Arc<dyn QueueEventEmitter>, config: EmitterConfig) -> Arc<dyn QueueEventEmitter> {
        Arc::new(Self::new(inner, config))
    }

    fn should_emit_progress(&self, job_id: &str) -> bool {
        let mut map = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
        let interval = Duration::from_millis(self.config.coalesce_interval_ms);
        let now = Instant::now();

        if let Some(last) = map.get(job_id) {
            if now.duration_since(*last) < interval {
                return false;
            }
        }
        map.insert(job_id.to_string(), now);
        true
    }

    /// Strip trace fields from an event when configured to omit them.
    #[allow(deprecated)]
    fn normalize_progress_event(&self, mut event: JobProgressEvent) -> JobProgressEvent {
        if !self.config.should_keep_trace() {
            event.trace_id = None;
            event.trace_ctx = None;
        }
        event
    }

    fn flush_pending(&self, job_id: &str) {
        let pending = {
            let mut pending_progress = self
                .pending_progress
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            let event = pending_progress.remove(job_id);
            if event.is_some() {
                let mut pending_order = self.pending_order.lock().unwrap_or_else(|e| e.into_inner());
                pending_order.retain(|queued_job_id| queued_job_id != job_id);
            }
            event
        };

        if let Some(event) = pending {
            self.inner.emit_job_progress(event);
        }
    }

    fn enqueue_progress(&self, event: JobProgressEvent) {
        let mut pending_progress = self
            .pending_progress
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let mut pending_order = self.pending_order.lock().unwrap_or_else(|e| e.into_inner());

        if pending_progress.contains_key(&event.job_id) {
            pending_progress.insert(event.job_id.clone(), event);
            return;
        }

        if pending_progress.len() >= self.config.buffer_size.max(1) {
            match self.config.drop_policy {
                DropPolicy::DropNewest => return,
                DropPolicy::DropOldest => {
                    if let Some(oldest_job_id) = pending_order.pop_front() {
                        pending_progress.remove(&oldest_job_id);
                    }
                }
                DropPolicy::Block => {
                    if let Some(oldest_job_id) = pending_order.pop_front() {
                        if let Some(oldest_event) = pending_progress.remove(&oldest_job_id) {
                            drop(pending_order);
                            drop(pending_progress);
                            self.inner.emit_job_progress(oldest_event);
                            pending_progress = self
                                .pending_progress
                                .lock()
                                .unwrap_or_else(|e| e.into_inner());
                            pending_order = self.pending_order.lock().unwrap_or_else(|e| e.into_inner());
                        }
                    }
                }
            }
        }

        pending_order.push_back(event.job_id.clone());
        pending_progress.insert(event.job_id.clone(), event);
    }
}

#[allow(deprecated)]
impl QueueEventEmitter for CoalescingEmitter {
    fn emit_job_started(&self, event: JobStartedEvent) {
        let mut event = event;
        if !self.config.should_keep_trace() {
            event.trace_id = None;
            event.trace_ctx = None;
        }
        self.inner.emit_job_started(event);
    }

    fn emit_job_completed(&self, event: JobCompletedEvent) {
        let mut event = event;
        if !self.config.should_keep_trace() {
            event.trace_id = None;
            event.trace_ctx = None;
        }
        self.flush_pending(&event.job_id);
        // Clean up progress tracking
        let mut map = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&event.job_id);
        drop(map);
        self.inner.emit_job_completed(event);
    }

    fn emit_job_failed(&self, event: JobFailedEvent) {
        let mut event = event;
        if !self.config.should_keep_trace() {
            event.trace_id = None;
            event.trace_ctx = None;
        }
        self.flush_pending(&event.job_id);
        let mut map = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&event.job_id);
        drop(map);
        self.inner.emit_job_failed(event);
    }

    fn emit_job_progress(&self, event: JobProgressEvent) {
        let event = self.normalize_progress_event(event);
        if self.should_emit_progress(&event.job_id) {
            self.flush_pending(&event.job_id);
            self.inner.emit_job_progress(event);
        } else {
            self.enqueue_progress(event);
        }
    }

    fn emit_job_cancelled(&self, event: JobCancelledEvent) {
        let mut event = event;
        if !self.config.should_keep_trace() {
            event.trace_id = None;
            event.trace_ctx = None;
        }
        self.flush_pending(&event.job_id);
        let mut map = self.last_progress.lock().unwrap_or_else(|e| e.into_inner());
        map.remove(&event.job_id);
        drop(map);
        self.inner.emit_job_cancelled(event);
    }
}

/// Event emitter that bridges job-queue events to Tauri's frontend event system.
///
/// Wraps a `tauri::AppHandle` and emits events with the `queue:` prefix.
pub struct TauriEventEmitter {
    app_handle: AppHandle,
}

impl TauriEventEmitter {
    /// Create a new Tauri event emitter from an app handle.
    pub fn new(app_handle: AppHandle) -> Self {
        Self { app_handle }
    }

    /// Create a new Tauri event emitter wrapped in an `Arc` for use with `QueueManager::spawn()`.
    pub fn arc(app_handle: AppHandle) -> Arc<dyn QueueEventEmitter> {
        Arc::new(Self::new(app_handle))
    }
}

impl QueueEventEmitter for TauriEventEmitter {
    fn emit_job_started(&self, event: JobStartedEvent) {
        let _ = self.app_handle.emit("queue:job_started", event);
    }

    fn emit_job_completed(&self, event: JobCompletedEvent) {
        let _ = self.app_handle.emit("queue:job_completed", event);
    }

    fn emit_job_failed(&self, event: JobFailedEvent) {
        let _ = self.app_handle.emit("queue:job_failed", event);
    }

    fn emit_job_progress(&self, event: JobProgressEvent) {
        let _ = self.app_handle.emit("queue:job_progress", event);
    }

    fn emit_job_cancelled(&self, event: JobCancelledEvent) {
        let _ = self.app_handle.emit("queue:job_cancelled", event);
    }
}
