//! Mock backend for testing without a live LLM.
//!
//! [`MockBackend`] returns pre-configured responses in order, allowing
//! downstream consumers to write deterministic tests against this crate.
//!
//! # Example
//!
//! ```
//! use llm_pipeline::backend::MockBackend;
//!
//! let mock = MockBackend::new(vec!["Hello, world!".to_string()]);
//! ```

use std::sync::atomic::{AtomicUsize, Ordering};

use async_trait::async_trait;
use reqwest::Client;

use super::{Backend, LlmRequest, LlmResponse};
use crate::error::Result;

/// A test backend that returns canned responses in order.
///
/// Cycles back to the beginning when all responses have been consumed.
/// For streaming, emits the entire response as a single token.
#[derive(Debug)]
pub struct MockBackend {
    responses: Vec<String>,
    index: AtomicUsize,
}

impl MockBackend {
    /// Create a mock backend with the given canned responses.
    ///
    /// Responses are returned in order. When exhausted, cycles from the beginning.
    pub fn new(responses: Vec<String>) -> Self {
        assert!(
            !responses.is_empty(),
            "MockBackend requires at least one response"
        );
        Self {
            responses,
            index: AtomicUsize::new(0),
        }
    }

    /// Create a mock that always returns the same response.
    pub fn fixed(response: impl Into<String>) -> Self {
        Self::new(vec![response.into()])
    }

    fn next_response(&self) -> String {
        let idx = self.index.fetch_add(1, Ordering::Relaxed) % self.responses.len();
        self.responses[idx].clone()
    }
}

#[async_trait]
impl Backend for MockBackend {
    async fn complete(
        &self,
        _client: &Client,
        _base_url: &str,
        _request: &LlmRequest,
    ) -> Result<LlmResponse> {
        let text = self.next_response();
        Ok(LlmResponse {
            text,
            status: 200,
            metadata: Default::default(),
        })
    }

    async fn complete_streaming(
        &self,
        _client: &Client,
        _base_url: &str,
        _request: &LlmRequest,
        on_token: &mut (dyn FnMut(String) + Send),
    ) -> Result<LlmResponse> {
        let text = self.next_response();
        on_token(text.clone());
        Ok(LlmResponse {
            text,
            status: 200,
            metadata: Default::default(),
        })
    }

    fn name(&self) -> &'static str {
        "mock"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_fixed_response() {
        let mock = MockBackend::fixed("Hello!");
        let client = Client::new();
        let request = LlmRequest {
            model: "test".to_string(),
            system_prompt: None,
            prompt: "test".to_string(),
            messages: vec![],
            config: Default::default(),
            stream: false,
        };
        let resp = mock
            .complete(&client, "http://unused", &request)
            .await
            .unwrap();
        assert_eq!(resp.text, "Hello!");
        assert_eq!(resp.status, 200);
    }

    #[tokio::test]
    async fn test_mock_cycles_responses() {
        let mock = MockBackend::new(vec!["first".into(), "second".into()]);
        let client = Client::new();
        let request = LlmRequest {
            model: "test".to_string(),
            system_prompt: None,
            prompt: "test".to_string(),
            messages: vec![],
            config: Default::default(),
            stream: false,
        };
        let r1 = mock
            .complete(&client, "http://unused", &request)
            .await
            .unwrap();
        let r2 = mock
            .complete(&client, "http://unused", &request)
            .await
            .unwrap();
        let r3 = mock
            .complete(&client, "http://unused", &request)
            .await
            .unwrap();
        assert_eq!(r1.text, "first");
        assert_eq!(r2.text, "second");
        assert_eq!(r3.text, "first"); // cycles
    }

    #[tokio::test]
    async fn test_mock_streaming() {
        let mock = MockBackend::fixed("streamed");
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
        let resp = mock
            .complete_streaming(&client, "http://unused", &request, &mut |t| tokens.push(t))
            .await
            .unwrap();
        assert_eq!(resp.text, "streamed");
        assert_eq!(tokens, vec!["streamed"]);
    }
}
