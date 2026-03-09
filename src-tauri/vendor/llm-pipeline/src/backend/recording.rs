//! Recording backend that wraps another backend and captures all requests/responses.
//!
//! Useful for testing, debugging, and audit logging.

use super::{Backend, LlmRequest, LlmResponse};
use crate::error::Result;
use async_trait::async_trait;
use reqwest::Client;
use std::sync::{Arc, Mutex};

/// A captured request-response pair from the recording backend.
#[derive(Debug, Clone)]
pub struct RecordedCall {
    /// The request that was sent.
    pub request: LlmRequest,
    /// The response text that was returned.
    pub response_text: String,
    /// HTTP status code.
    pub status: u16,
}

/// A backend wrapper that records all requests and responses.
///
/// Delegates to an inner backend and captures every call for later inspection.
/// Thread-safe via internal `Mutex`.
///
/// # Example
///
/// ```
/// use llm_pipeline::backend::{MockBackend, RecordingBackend};
/// use std::sync::Arc;
///
/// let mock = Arc::new(MockBackend::fixed(r#"{"answer": 42}"#));
/// let recording = RecordingBackend::wrap(mock);
///
/// // After some calls...
/// let calls = recording.calls();
/// // Each call contains the request and response
/// ```
pub struct RecordingBackend {
    inner: Arc<dyn Backend>,
    calls: Mutex<Vec<RecordedCall>>,
}

impl RecordingBackend {
    /// Wrap an existing backend with recording.
    pub fn wrap(inner: Arc<dyn Backend>) -> Self {
        Self {
            inner,
            calls: Mutex::new(Vec::new()),
        }
    }

    /// Return all recorded calls.
    pub fn calls(&self) -> Vec<RecordedCall> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    /// Return the number of recorded calls.
    pub fn call_count(&self) -> usize {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .len()
    }

    /// Clear all recorded calls.
    pub fn clear(&self) {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
    }
}

impl std::fmt::Debug for RecordingBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingBackend")
            .field("inner", &self.inner.name())
            .field("call_count", &self.call_count())
            .finish()
    }
}

#[async_trait]
impl Backend for RecordingBackend {
    async fn complete(
        &self,
        client: &Client,
        base_url: &str,
        request: &LlmRequest,
    ) -> Result<LlmResponse> {
        let response = self.inner.complete(client, base_url, request).await?;
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(RecordedCall {
                request: request.clone(),
                response_text: response.text.clone(),
                status: response.status,
            });
        Ok(response)
    }

    async fn complete_streaming(
        &self,
        client: &Client,
        base_url: &str,
        request: &LlmRequest,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> Result<LlmResponse> {
        let response = self
            .inner
            .complete_streaming(client, base_url, request, on_token)
            .await?;
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push(RecordedCall {
                request: request.clone(),
                response_text: response.text.clone(),
                status: response.status,
            });
        Ok(response)
    }

    fn name(&self) -> &'static str {
        "recording"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::MockBackend;

    #[tokio::test]
    async fn test_recording_backend_captures_calls() {
        let mock = Arc::new(MockBackend::fixed("hello"));
        let recording = RecordingBackend::wrap(mock);
        let client = Client::new();
        let request = LlmRequest {
            model: "test".to_string(),
            system_prompt: None,
            prompt: "say hi".to_string(),
            messages: vec![],
            config: Default::default(),
            stream: false,
        };

        let resp = recording
            .complete(&client, "http://unused", &request)
            .await
            .unwrap();
        assert_eq!(resp.text, "hello");

        let calls = recording.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].request.prompt, "say hi");
        assert_eq!(calls[0].response_text, "hello");
        assert_eq!(calls[0].status, 200);
    }

    #[tokio::test]
    async fn test_recording_backend_clear() {
        let mock = Arc::new(MockBackend::fixed("test"));
        let recording = RecordingBackend::wrap(mock);
        let client = Client::new();
        let request = LlmRequest {
            model: "test".to_string(),
            system_prompt: None,
            prompt: "test".to_string(),
            messages: vec![],
            config: Default::default(),
            stream: false,
        };

        recording
            .complete(&client, "http://unused", &request)
            .await
            .unwrap();
        assert_eq!(recording.call_count(), 1);

        recording.clear();
        assert_eq!(recording.call_count(), 0);
    }

    #[tokio::test]
    async fn test_recording_backend_streaming() {
        let mock = Arc::new(MockBackend::fixed("streamed"));
        let recording = RecordingBackend::wrap(mock);
        let client = Client::new();
        let request = LlmRequest {
            model: "test".to_string(),
            system_prompt: None,
            prompt: "test".to_string(),
            messages: vec![],
            config: Default::default(),
            stream: true,
        };

        let mut tokens = Vec::new();
        recording
            .complete_streaming(&client, "http://unused", &request, &mut |t| {
                tokens.push(t);
            })
            .await
            .unwrap();

        assert_eq!(tokens, vec!["streamed"]);
        assert_eq!(recording.call_count(), 1);
    }
}
