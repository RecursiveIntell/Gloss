use crate::error::GlossError;
use crate::memory::RetrievalOutcome;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAttemptTraceEvent {
    pub phase: String,
    pub recorded_at: String,
    pub elapsed_ms: Option<u128>,
    pub detail: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatAttemptTraceV1 {
    pub schema: String,
    pub attempt_id: String,
    pub notebook_id: String,
    pub conversation_id: String,
    pub message_id: String,
    pub model: String,
    pub provider: String,
    pub provider_base_url: Option<String>,
    pub memory_backend: Option<String>,
    pub memory_backend_fallback: Option<bool>,
    pub source_scope_mode: Option<String>,
    pub first_token_seen: bool,
    pub done_seen: bool,
    pub assistant_persisted: bool,
    pub error: Option<String>,
    pub retrieval_trace_ref: Option<String>,
    pub retrieval_outcome: Option<RetrievalOutcome>,
    pub events: Vec<ChatAttemptTraceEvent>,
}

// ---------------------------------------------------------------------------
// Batch receipt types for llm-pipeline batching
// ---------------------------------------------------------------------------

/// Receipt for a single LLM call within a batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchCallReceiptV1 {
    pub schema: String,
    pub call_index: usize,
    pub call_purpose: String,
    pub model: String,
    pub provider: String,
    pub prompt_tokens: Option<u32>,
    pub completion_tokens: Option<u32>,
    pub duration_ms: u128,
    pub success: bool,
    pub error_message: Option<String>,
    pub recorded_at: String,
}

/// Receipt wrapping multiple batched LLM calls with batch-level metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatchReceiptV1 {
    pub schema: String,
    pub batch_id: String,
    pub batch_type: String,
    pub notebook_id: Option<String>,
    pub source_id: Option<String>,
    pub total_calls: usize,
    pub successful_calls: usize,
    pub failed_calls: usize,
    pub total_duration_ms: u128,
    pub calls: Vec<BatchCallReceiptV1>,
    pub recorded_at: String,
}

impl BatchReceiptV1 {
    /// Create an empty batch receipt with the given batch type.
    pub fn new(batch_type: &str, notebook_id: Option<&str>, source_id: Option<&str>) -> Self {
        Self {
            schema: "BatchReceiptV1".to_string(),
            batch_id: uuid::Uuid::new_v4().to_string(),
            batch_type: batch_type.to_string(),
            notebook_id: notebook_id.map(str::to_string),
            source_id: source_id.map(str::to_string),
            total_calls: 0,
            successful_calls: 0,
            failed_calls: 0,
            total_duration_ms: 0,
            calls: Vec::new(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    /// Record a completed call into the batch receipt.
    #[allow(clippy::too_many_arguments)]
    pub fn record_call(
        &mut self,
        call_index: usize,
        call_purpose: &str,
        model: &str,
        provider: &str,
        prompt_tokens: Option<u32>,
        completion_tokens: Option<u32>,
        duration: Duration,
        success: bool,
        error_message: Option<&str>,
    ) {
        let call_receipt = BatchCallReceiptV1 {
            schema: "BatchCallReceiptV1".to_string(),
            call_index,
            call_purpose: call_purpose.to_string(),
            model: model.to_string(),
            provider: provider.to_string(),
            prompt_tokens,
            completion_tokens,
            duration_ms: duration.as_millis(),
            success,
            error_message: error_message.map(str::to_string),
            recorded_at: chrono::Utc::now().to_rfc3339(),
        };
        self.calls.push(call_receipt);
        self.total_calls += 1;
        self.total_duration_ms += duration.as_millis();
        if success {
            self.successful_calls += 1;
        } else {
            self.failed_calls += 1;
        }
    }

    /// Finalize the batch receipt, updating the recorded_at timestamp.
    pub fn finalize(&mut self) {
        self.recorded_at = chrono::Utc::now().to_rfc3339();
    }
}

/// Persist a batch receipt to the chat-attempt-traces directory.
pub(crate) fn persist_batch_receipt(
    data_dir: &Path,
    receipt: &BatchReceiptV1,
) -> Result<(), GlossError> {
    let trace_dir = data_dir.join("chat-attempt-traces");
    std::fs::create_dir_all(&trace_dir)?;
    let bytes = serde_json::to_vec_pretty(receipt)?;
    std::fs::write(
        trace_dir.join(format!("batch-{}.json", receipt.batch_id)),
        &bytes,
    )?;
    Ok(())
}

/// Record a batch receipt into the chat attempt trace and persist it.
#[allow(dead_code)]
pub(crate) fn record_batch_receipt_in_trace(
    trace: &Arc<Mutex<ChatAttemptTraceV1>>,
    data_dir: &Path,
    batch_receipt: &BatchReceiptV1,
) {
    let detail = serde_json::to_string(batch_receipt).ok();
    let error = if batch_receipt.failed_calls > 0 {
        Some(format!(
            "{} of {} batched calls failed",
            batch_receipt.failed_calls, batch_receipt.total_calls
        ))
    } else {
        None
    };

    record_chat_attempt_trace(
        trace,
        data_dir,
        &format!("batch_receipt_{}", batch_receipt.batch_type),
        Some(Duration::from_millis(
            batch_receipt.total_duration_ms as u64,
        )),
        detail.as_deref(),
        error.as_deref(),
        |_| {},
    );

    if let Err(err) = persist_batch_receipt(data_dir, batch_receipt) {
        tracing::warn!(error = %err, "Failed to persist batch receipt");
    }
}

pub(crate) fn new_chat_attempt_trace(
    notebook_id: &str,
    conversation_id: &str,
    message_id: &str,
    model: &str,
    memory_backend: Option<String>,
    memory_backend_fallback: Option<bool>,
    source_scope_mode: Option<String>,
) -> ChatAttemptTraceV1 {
    ChatAttemptTraceV1 {
        schema: "ChatAttemptTraceV1".to_string(),
        attempt_id: uuid::Uuid::new_v4().to_string(),
        notebook_id: notebook_id.to_string(),
        conversation_id: conversation_id.to_string(),
        message_id: message_id.to_string(),
        model: model.to_string(),
        provider: "unknown".to_string(),
        provider_base_url: None,
        memory_backend,
        memory_backend_fallback,
        source_scope_mode,
        first_token_seen: false,
        done_seen: false,
        assistant_persisted: false,
        error: None,
        retrieval_trace_ref: None,
        retrieval_outcome: None,
        events: Vec::new(),
    }
}

pub(crate) fn persist_chat_attempt_trace(
    data_dir: &Path,
    trace: &ChatAttemptTraceV1,
) -> Result<(), GlossError> {
    let trace_dir = data_dir.join("chat-attempt-traces");
    std::fs::create_dir_all(&trace_dir)?;
    let bytes = serde_json::to_vec_pretty(trace)?;
    std::fs::write(trace_dir.join(format!("{}.json", trace.attempt_id)), &bytes)?;
    Ok(())
}

pub(crate) fn record_chat_attempt_trace<F>(
    trace: &Arc<Mutex<ChatAttemptTraceV1>>,
    data_dir: &Path,
    phase: &str,
    elapsed: Option<Duration>,
    detail: Option<&str>,
    error: Option<&str>,
    update: F,
) where
    F: FnOnce(&mut ChatAttemptTraceV1),
{
    let snapshot = {
        let mut guard = trace.lock().unwrap_or_else(|e| e.into_inner());
        update(&mut guard);
        if let Some(error) = error {
            guard.error = Some(error.to_string());
        }
        guard.events.push(ChatAttemptTraceEvent {
            phase: phase.to_string(),
            recorded_at: chrono::Utc::now().to_rfc3339(),
            elapsed_ms: elapsed.map(|elapsed| elapsed.as_millis()),
            detail: detail.map(str::to_string),
            error: error.map(str::to_string),
        });
        guard.clone()
    };

    if let Err(err) = persist_chat_attempt_trace(data_dir, &snapshot) {
        tracing::warn!(error = %err, "Failed to persist chat attempt trace");
    }
}

pub(crate) fn chat_attempt_trace_snapshot(
    trace: &Arc<Mutex<ChatAttemptTraceV1>>,
) -> ChatAttemptTraceV1 {
    trace.lock().unwrap_or_else(|e| e.into_inner()).clone()
}
