use crate::memory::RetrievalReasonCode;
use crate::state::AppState;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{Emitter, Manager};

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
    pub reason_code: Option<RetrievalReasonCode>,
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
    let payload = ChatStatusPayload {
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
        reason_code: status_reason_code(phase, owner_detail, error),
        elapsed_ms: elapsed.as_millis(),
        timeout_ms: timeout.map(|timeout| timeout.as_millis()),
        truncated,
        error,
    };
    emit_chat_stream_event(
        handle,
        "chat:status",
        "status",
        message_id,
        notebook_id,
        conversation_id,
        message_id,
        serde_json::to_value(&payload).unwrap_or_else(|_| serde_json::json!({})),
    );
    let _ = handle.emit("chat:status", payload);
}

fn status_reason_code(
    phase: &str,
    owner_detail: Option<&str>,
    error: Option<&str>,
) -> Option<RetrievalReasonCode> {
    let haystack = [Some(phase), owner_detail, error]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    if haystack.contains("search-timeout") || haystack.contains("timed out") {
        Some(RetrievalReasonCode::SemanticMemoryTimeout)
    } else if haystack.contains("not-compiled") || haystack.contains("feature is not enabled") {
        Some(RetrievalReasonCode::SemanticMemoryBuildFeatureMissing)
    } else if haystack.contains("flag-off") || haystack.contains("feature gate") {
        Some(RetrievalReasonCode::SemanticMemoryFeatureDisabled)
    } else if haystack.contains("links-missing") || haystack.contains("projection-required") {
        Some(RetrievalReasonCode::SemanticMemoryLinksMissing)
    } else if haystack.contains("projection-failed") || haystack.contains("degraded") {
        Some(RetrievalReasonCode::SemanticMemoryLinksDegraded)
    } else if haystack.contains("no-candidates") || haystack.contains("no mapped candidates") {
        Some(RetrievalReasonCode::NoRetrievalContext)
    } else {
        None
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_chat_stream_event(
    handle: &tauri::AppHandle,
    event_name: &str,
    kind: &str,
    attempt_id: &str,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    payload: serde_json::Value,
) {
    let Some(state) = handle.try_state::<AppState>() else {
        return;
    };
    let event = state.record_chat_stream_event(
        attempt_id,
        kind,
        notebook_id,
        conversation_id,
        message_id,
        payload,
    );
    let _ = handle.emit("chat:stream_event", &event);
    tracing::trace!(
        event_name,
        seq = event.seq,
        attempt_id = %event.attempt_id,
        kind = %event.kind,
        "recorded chat stream replay event"
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
    emit_chat_token(handle, notebook_id, conversation_id, message_id, "", true);
}

pub(crate) fn emit_chat_token(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    token: &str,
    done: bool,
) {
    let kind = if done { "done" } else { "token" };
    let payload = serde_json::json!({
        "notebook_id": notebook_id,
        "conversation_id": conversation_id,
        "message_id": message_id,
        "attempt_id": message_id,
        "token": token,
        "done": done,
    });
    emit_chat_stream_event(
        handle,
        "chat:token",
        kind,
        message_id,
        notebook_id,
        conversation_id,
        message_id,
        payload.clone(),
    );
    let _ = handle.emit("chat:token", payload);
}

pub(crate) fn emit_chat_evidence(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    citations: serde_json::Value,
    evidence: serde_json::Value,
) {
    let payload = serde_json::json!({
        "notebook_id": notebook_id,
        "conversation_id": conversation_id,
        "message_id": message_id,
        "attempt_id": message_id,
        "citations": citations,
        "evidence": evidence,
    });
    emit_chat_stream_event(
        handle,
        "chat:evidence",
        "evidence",
        message_id,
        notebook_id,
        conversation_id,
        message_id,
        payload.clone(),
    );
    let _ = handle.emit("chat:evidence", payload);
}

pub(crate) fn emit_chat_error(
    handle: &tauri::AppHandle,
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    error: &str,
) {
    let payload = serde_json::json!({
        "notebook_id": notebook_id,
        "conversation_id": conversation_id,
        "message_id": message_id,
        "attempt_id": message_id,
        "error": error,
    });
    emit_chat_stream_event(
        handle,
        "chat:error",
        "error",
        message_id,
        notebook_id,
        conversation_id,
        message_id,
        payload.clone(),
    );
    let _ = handle.emit("chat:error", payload);
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
    let payload = serde_json::json!({
        "notebook_id": notebook_id,
        "conversation_id": conversation_id,
        "message_id": message_id,
        "attempt_id": message_id,
        "reason": reason,
    });
    emit_chat_stream_event(
        handle,
        "chat:cancelled",
        "cancelled",
        message_id,
        notebook_id,
        conversation_id,
        message_id,
        payload.clone(),
    );
    let _ = handle.emit("chat:cancelled", payload);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_payload_reason_code_is_typed_for_semantic_timeout() {
        assert_eq!(
            status_reason_code(
                "semantic_memory_search_timeout",
                Some("semantic-memory"),
                Some("search-timeout: semantic-memory preview timed out")
            ),
            Some(RetrievalReasonCode::SemanticMemoryTimeout)
        );
    }
}
