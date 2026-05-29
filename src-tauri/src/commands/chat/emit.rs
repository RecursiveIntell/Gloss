use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;

#[derive(Debug, Clone, Serialize)]
pub(crate) struct ChatStatusPayload<'a> {
    pub notebook_id: &'a str,
    pub conversation_id: &'a str,
    pub message_id: &'a str,
    pub phase: &'a str,
    pub message: &'a str,
    pub provider: Option<&'a str>,
    pub model: Option<&'a str>,
    pub gate: Option<&'a str>,
    pub owner: Option<&'a str>,
    pub owner_detail: Option<&'a str>,
    pub elapsed_ms: u128,
    pub timeout_ms: Option<u128>,
    pub truncated: bool,
    pub error: Option<&'a str>,
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn emit_chat_status(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    phase: &str,
    message: &str,
    provider: Option<&str>,
    model: Option<&str>,
    gate: Option<&str>,
    owner: Option<&str>,
    owner_detail: Option<&str>,
    elapsed: Duration,
    timeout: Option<Duration>,
    truncated: bool,
    error: Option<&str>,
) {
    let _ = handle.emit(
        "chat:status",
        ChatStatusPayload {
            notebook_id,
            conversation_id,
            message_id,
            phase,
            message,
            provider,
            model,
            gate,
            owner,
            owner_detail,
            elapsed_ms: elapsed.as_millis(),
            timeout_ms: timeout.map(|timeout| timeout.as_millis()),
            truncated,
            error,
        },
    );
}

/// Guarantees that exactly one terminal event (chat:done, chat:error, or
/// chat:cancelled) is emitted per chat stream. Uses an `AtomicBool` guard so
/// that even when multiple exit paths race (e.g. notebook-switch cancellation
/// and normal completion), only the first caller actually emits.
pub(crate) struct ChatTerminalGuard {
    fired: AtomicBool,
    handle: tauri::AppHandle,
    notebook_id: String,
    conversation_id: String,
    message_id: String,
}

impl ChatTerminalGuard {
    pub fn new(
        handle: tauri::AppHandle,
        notebook_id: &str,
        conversation_id: &str,
        message_id: &str,
    ) -> Self {
        Self {
            fired: AtomicBool::new(false),
            handle: handle.clone(),
            notebook_id: notebook_id.to_string(),
            conversation_id: conversation_id.to_string(),
            message_id: message_id.to_string(),
        }
    }

    /// Emit `chat:done` as the terminal event. Returns `true` if this call
    /// actually emitted (i.e. no prior terminal was fired).
    pub fn emit_done(&self) -> bool {
        if self.fired.swap(true, Ordering::SeqCst) {
            return false; // already fired
        }
        emit_chat_done(
            &self.handle,
            &self.notebook_id,
            &self.conversation_id,
            &self.message_id,
        );
        true
    }

    /// Emit `chat:error` as the terminal event. Returns `true` if this call
    /// actually emitted.
    pub fn emit_error(&self, error: &str) -> bool {
        if self.fired.swap(true, Ordering::SeqCst) {
            return false;
        }
        emit_chat_error(
            &self.handle,
            &self.notebook_id,
            &self.conversation_id,
            &self.message_id,
            error,
        );
        true
    }

    /// Emit `chat:cancelled` as the terminal event. Returns `true` if this call
    /// actually emitted.
    pub fn emit_cancelled(&self, reason: &str) -> bool {
        if self.fired.swap(true, Ordering::SeqCst) {
            return false;
        }
        emit_chat_cancelled(
            &self.handle,
            &self.notebook_id,
            &self.conversation_id,
            &self.message_id,
            reason,
        );
        true
    }

    /// Returns `true` if a terminal event has already been emitted.
    #[allow(dead_code)]
    pub fn is_fired(&self) -> bool {
        self.fired.load(Ordering::SeqCst)
    }
}

/// Convenience: wraps a `ChatTerminalGuard` in `Arc` and provides helper methods.
/// This is the primary API for spawned chat tasks.
#[derive(Clone)]
pub(crate) struct ChatTerminalEmitter(Arc<ChatTerminalGuard>);

impl ChatTerminalEmitter {
    pub fn new(
        handle: tauri::AppHandle,
        notebook_id: &str,
        conversation_id: &str,
        message_id: &str,
    ) -> Self {
        Self(Arc::new(ChatTerminalGuard::new(
            handle,
            notebook_id,
            conversation_id,
            message_id,
        )))
    }

    pub fn emit_done(&self) -> bool {
        self.0.emit_done()
    }

    pub fn emit_error(&self, error: &str) -> bool {
        self.0.emit_error(error)
    }

    pub fn emit_cancelled(&self, reason: &str) -> bool {
        self.0.emit_cancelled(reason)
    }

    #[allow(dead_code)]
    pub fn is_fired(&self) -> bool {
        self.0.is_fired()
    }
}

pub(crate) fn emit_chat_done(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
) {
    let _ = handle.emit(
        "chat:token",
        serde_json::json!({
            "notebook_id": notebook_id,
            "conversation_id": conversation_id,
            "message_id": message_id,
            "token": "",
            "done": true,
        }),
    );
}

pub(crate) fn emit_chat_error(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    error: &str,
) {
    let _ = handle.emit(
        "chat:error",
        serde_json::json!({
            "notebook_id": notebook_id,
            "conversation_id": conversation_id,
            "message_id": message_id,
            "error": error,
        }),
    );
}

/// Emit a `chat:cancelled` event so the frontend can clear streaming state
/// when a chat was cancelled (e.g. notebook switch) rather than treating it
/// as an error. The frontend should treat this as a terminal event: it ends
/// the streaming state.
pub(crate) fn emit_chat_cancelled(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    reason: &str,
) {
    let _ = handle.emit(
        "chat:cancelled",
        serde_json::json!({
            "notebook_id": notebook_id,
            "conversation_id": conversation_id,
            "message_id": message_id,
            "reason": reason,
        }),
    );
}
