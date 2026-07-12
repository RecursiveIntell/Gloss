//! Event system for payload lifecycle and streaming hooks.
//!
//! Provides an optional, non-intrusive way to observe payload execution.
//! Payloads emit events when they start, produce tokens, and finish.
//! Users can implement [`EventHandler`] to receive these events for
//! logging, progress tracking, or streaming UIs.

use stack_ids::{AttemptId, TrialId};
use std::sync::Arc;

/// Events emitted during payload execution.
#[derive(Debug, Clone)]
pub enum Event {
    /// A payload has started executing.
    PayloadStart {
        /// Instance name of the payload.
        name: String,
        /// Stable type identifier (e.g. `"llm-call"`, `"chain"`).
        kind: &'static str,
    },
    /// A token was received during streaming.
    Token {
        /// Instance name of the payload producing this token.
        name: String,
        /// The token text.
        chunk: String,
    },
    /// A payload has finished executing.
    PayloadEnd {
        /// Instance name of the payload.
        name: String,
        /// Whether execution succeeded.
        ok: bool,
    },
    /// A semantic retry attempt is starting.
    RetryStart {
        /// Instance name of the payload being retried.
        name: String,
        /// The retry attempt number (1-indexed).
        attempt: u32,
        /// Why the retry was triggered (parse error or validator message).
        reason: String,
        /// Logical retry family identifier.
        attempt_id: AttemptId,
        /// Concrete execution identifier for this trial.
        trial_id: TrialId,
    },
    /// A semantic retry sequence has completed.
    RetryEnd {
        /// Instance name of the payload.
        name: String,
        /// Total number of retry attempts made.
        attempts: u32,
        /// Whether the final attempt succeeded.
        success: bool,
        /// Logical retry family identifier.
        attempt_id: AttemptId,
    },
    /// A partial parse result from streaming JSON.
    PartialParse {
        /// Instance name of the payload.
        name: String,
        /// The partial JSON value extracted so far.
        value: serde_json::Value,
        /// Whether the JSON appears complete (all brackets closed).
        complete: bool,
    },
    /// A transport-level retry due to HTTP error.
    TransportRetry {
        /// Instance name or operation description.
        name: String,
        /// The retry attempt number (1-indexed).
        attempt: u32,
        /// Delay before this retry attempt in milliseconds.
        delay_ms: u64,
        /// Reason for the retry (error description).
        reason: String,
    },
}

/// Handler for payload lifecycle events.
///
/// Implement this trait to receive streaming tokens, progress updates,
/// and other lifecycle signals during payload execution.
///
/// This is entirely optional -- payloads work without an event handler.
///
/// # Example
///
/// ```
/// use llm_pipeline::events::{Event, EventHandler};
///
/// struct PrintHandler;
///
/// impl EventHandler for PrintHandler {
///     fn on_event(&self, event: Event) {
///         match event {
///             Event::Token { chunk, .. } => print!("{}", chunk),
///             Event::PayloadStart { name, .. } => println!("[start] {}", name),
///             Event::PayloadEnd { name, ok, .. } => println!("[end] {} ok={}", name, ok),
///             _ => {} // RetryStart, RetryEnd, PartialParse, TransportRetry
///         }
///     }
/// }
/// ```
pub trait EventHandler: Send + Sync {
    /// Called when a payload emits an event.
    fn on_event(&self, event: Event);
}

/// Emit an event if a handler is present. No-op otherwise.
pub(crate) fn emit(handler: &Option<Arc<dyn EventHandler>>, event: Event) {
    if let Some(ref h) = handler {
        h.on_event(event);
    }
}

/// An [`EventHandler`] backed by a closure.
///
/// # Example
///
/// ```
/// use llm_pipeline::events::{Event, FnEventHandler};
/// use std::sync::Arc;
///
/// let handler = Arc::new(FnEventHandler(|event: Event| {
///     if let Event::Token { chunk, .. } = event {
///         print!("{}", chunk);
///     }
/// }));
/// ```
pub struct FnEventHandler<F: Fn(Event) + Send + Sync>(pub F);

impl<F: Fn(Event) + Send + Sync> EventHandler for FnEventHandler<F> {
    fn on_event(&self, event: Event) {
        (self.0)(event);
    }
}
