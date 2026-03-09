use crate::error::GlossError;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use futures::StreamExt;

/// Generate a summary of a source document using the LLM.
pub async fn summarize_source(
    content: &str,
    title: &str,
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<String, GlossError> {
    // Truncate content to ~3000 tokens (~12000 chars) for single-pass summary
    let truncated = if content.len() > 12000 {
        let mut end = 12000;
        while end > 0 && !content.is_char_boundary(end) {
            end -= 1;
        }
        &content[..end]
    } else {
        content
    };

    let request = ChatRequest {
        model: model.to_string(),
        system_prompt: Some(
            "You are a concise summarizer. Produce a clear, informative summary of the given document. \
             Include key topics, main arguments, and important details. Keep it under 300 words."
                .to_string(),
        ),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: format!(
                "Summarize this document titled \"{}\":\n\n{}",
                title, truncated
            ),
            images: None,
        }],
        max_tokens: 512,
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

/// Generate suggested questions from source summaries.
pub async fn generate_suggested_questions(
    summaries: &[(String, String, Option<String>)], // (source_id, title, summary)
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<Vec<String>, GlossError> {
    if summaries.is_empty() {
        return Ok(Vec::new());
    }

    let summary_text: String = summaries
        .iter()
        .filter_map(|(_, title, summary)| summary.as_ref().map(|s| format!("**{}**: {}", title, s)))
        .collect::<Vec<_>>()
        .join("\n\n");

    if summary_text.is_empty() {
        return Ok(Vec::new());
    }

    let request = ChatRequest {
        model: model.to_string(),
        system_prompt: Some(
            "Generate exactly 3 interesting questions that could be asked about the following source material. \
             Return ONLY a JSON array of 3 strings, nothing else. Example: [\"Question 1?\", \"Question 2?\", \"Question 3?\"]"
                .to_string(),
        ),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: summary_text,
            images: None,
        }],
        max_tokens: 256,
        temperature: 0.7,
        stream: false,
        num_ctx: Some(8192),
    };

    let mut stream = provider.chat(request).await?;
    let mut response = String::new();
    while let Some(result) = stream.next().await {
        let token = result?;
        response.push_str(&token.token);
    }

    // Parse JSON array from response
    let questions: Vec<String> = llm_pipeline::parsing::parse_as(&response).unwrap_or_else(|_| {
        tracing::warn!("Failed to parse suggested questions as JSON, falling back");
        Vec::new()
    });

    Ok(questions)
}
