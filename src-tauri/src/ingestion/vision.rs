use crate::error::GlossError;
use crate::providers::{ChatMessage, ChatRequest, LlmExecutionContext, LlmProvider};
use futures::StreamExt;
use std::time::Instant;

/// Receipt for a vision/image description LLM call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct VisionCallReceipt {
    pub schema: String,
    pub call_purpose: String,
    pub model: String,
    pub provider: String,
    pub duration_ms: u128,
    pub success: bool,
    pub error_message: Option<String>,
    pub response_chars: Option<usize>,
}

/// Describe an image using a vision-capable LLM.
///
/// Takes a base64-encoded image and sends it to the vision model for description.
/// The description is used as the source's content_text for RAG retrieval.
/// Returns a tuple of (description_text, call_receipt).
pub async fn describe_image(
    image_base64: &str,
    filename: &str,
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<(String, VisionCallReceipt), GlossError> {
    let request = ChatRequest {
        model: model.to_string(),
        system_prompt: Some(
            "You are an image description assistant. Describe the image in detail, including: \
             main subjects, text or labels visible, layout, colors, and any notable features. \
             Be thorough but concise. This description will be used for search and retrieval."
                .to_string(),
        ),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: format!("Describe this image (filename: {}):", filename),
            images: Some(vec![image_base64.to_string()]),
        }],
        max_tokens: 1024,
        temperature: 0.3,
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        stream: false,
        num_ctx: Some(8192),
    };

    let start = Instant::now();
    let provider_type = provider.provider_type().as_str().to_string();
    let result = async {
        let mut stream = provider
            .chat(request, LlmExecutionContext::uncancellable())
            .await?;
        let mut response = String::new();
        while let Some(token_result) = stream.next().await {
            let token = token_result?;
            response.push_str(&token.token);
        }
        Ok::<String, GlossError>(response)
    }
    .await;
    let elapsed = start.elapsed();

    match result {
        Ok(response) => {
            let chars = response.len();
            let receipt = VisionCallReceipt {
                schema: "VisionCallReceipt".to_string(),
                call_purpose: "describe_image".to_string(),
                model: model.to_string(),
                provider: provider_type,
                duration_ms: elapsed.as_millis(),
                success: true,
                error_message: None,
                response_chars: Some(chars),
            };
            Ok((response.trim().to_string(), receipt))
        }
        Err(e) => {
            let _receipt = VisionCallReceipt {
                schema: "VisionCallReceipt".to_string(),
                call_purpose: "describe_image".to_string(),
                model: model.to_string(),
                provider: provider_type,
                duration_ms: elapsed.as_millis(),
                success: false,
                error_message: Some(e.to_string()),
                response_chars: None,
            };
            Err(e)
        }
    }
}
