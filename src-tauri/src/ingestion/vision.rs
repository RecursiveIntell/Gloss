use crate::error::GlossError;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use futures::StreamExt;

/// Describe an image using a vision-capable LLM.
///
/// Takes a base64-encoded image and sends it to the vision model for description.
/// The description is used as the source's content_text for RAG retrieval.
pub async fn describe_image(
    image_base64: &str,
    filename: &str,
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<String, GlossError> {
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
        stream: false,
        num_ctx: Some(8192),
    };

    let mut stream = provider.chat(request).await?;
    let mut response = String::new();
    while let Some(result) = stream.next().await {
        let token = result?;
        response.push_str(&token.token);
    }

    Ok(response.trim().to_string())
}
