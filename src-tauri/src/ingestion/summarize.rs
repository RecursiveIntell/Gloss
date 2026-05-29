use crate::error::GlossError;
use crate::providers::{ChatMessage, ChatRequest, LlmProvider};
use futures::StreamExt;
use std::time::Instant;

const DEFAULT_SUGGESTED_QUESTIONS_TEMPERATURE: f32 = 0.7;

/// Receipt for a single summary LLM call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SummarizeCallReceipt {
    pub schema: String,
    pub call_purpose: String,
    pub model: String,
    pub provider: String,
    pub duration_ms: u128,
    pub success: bool,
    pub error_message: Option<String>,
    pub response_chars: Option<usize>,
}

/// Generate a summary of a source document using the LLM.
/// Returns a tuple of (summary_text, call_receipt).
pub async fn summarize_source(
    content: &str,
    title: &str,
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<(String, SummarizeCallReceipt), GlossError> {
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
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        stream: false,
        num_ctx: Some(8192),
    };

    let start = Instant::now();
    let result = async {
        let mut stream = provider.chat(request).await?;
        let mut response = String::new();
        while let Some(token_result) = stream.next().await {
            let token = token_result?;
            response.push_str(&token.token);
        }
        Ok::<String, GlossError>(response)
    }
    .await;

    let elapsed = start.elapsed();
    let provider_type = provider.provider_type().as_str().to_string();

    match result {
        Ok(response) => {
            let chars = response.len();
            let receipt = SummarizeCallReceipt {
                schema: "SummarizeCallReceipt".to_string(),
                call_purpose: "summarize_source".to_string(),
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
            let _receipt = SummarizeCallReceipt {
                schema: "SummarizeCallReceipt".to_string(),
                call_purpose: "summarize_source".to_string(),
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

/// Receipt for a suggested-questions LLM call.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SuggestedQuestionsCallReceipt {
    pub schema: String,
    pub call_purpose: String,
    pub model: String,
    pub provider: String,
    pub duration_ms: u128,
    pub success: bool,
    pub error_message: Option<String>,
    pub question_count: Option<usize>,
}

/// Generate suggested questions from source summaries.
/// Returns a tuple of (questions, call_receipt).
#[allow(dead_code)]
pub async fn generate_suggested_questions(
    summaries: &[(String, String, Option<String>)], // (source_id, title, summary)
    provider: &dyn LlmProvider,
    model: &str,
) -> Result<(Vec<String>, SuggestedQuestionsCallReceipt), GlossError> {
    if summaries.is_empty() {
        let receipt = SuggestedQuestionsCallReceipt {
            schema: "SuggestedQuestionsCallReceipt".to_string(),
            call_purpose: "generate_suggested_questions".to_string(),
            model: model.to_string(),
            provider: provider.provider_type().as_str().to_string(),
            duration_ms: 0,
            success: true,
            error_message: None,
            question_count: Some(0),
        };
        return Ok((Vec::new(), receipt));
    }

    let summary_text: String = summaries
        .iter()
        .filter_map(|(_, title, summary)| summary.as_ref().map(|s| format!("**{}**: {}", title, s)))
        .collect::<Vec<_>>()
        .join("\n\n");

    if summary_text.is_empty() {
        let receipt = SuggestedQuestionsCallReceipt {
            schema: "SuggestedQuestionsCallReceipt".to_string(),
            call_purpose: "generate_suggested_questions".to_string(),
            model: model.to_string(),
            provider: provider.provider_type().as_str().to_string(),
            duration_ms: 0,
            success: true,
            error_message: None,
            question_count: Some(0),
        };
        return Ok((Vec::new(), receipt));
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
        temperature: DEFAULT_SUGGESTED_QUESTIONS_TEMPERATURE,
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
        let mut stream = provider.chat(request).await?;
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
            // Parse JSON array from response
            let questions: Vec<String> =
                llm_pipeline::parsing::parse_as(&response).unwrap_or_else(|_| {
                    tracing::warn!("Failed to parse suggested questions as JSON, falling back");
                    Vec::new()
                });
            let count = questions.len();
            let receipt = SuggestedQuestionsCallReceipt {
                schema: "SuggestedQuestionsCallReceipt".to_string(),
                call_purpose: "generate_suggested_questions".to_string(),
                model: model.to_string(),
                provider: provider_type,
                duration_ms: elapsed.as_millis(),
                success: true,
                error_message: None,
                question_count: Some(count),
            };
            Ok((questions, receipt))
        }
        Err(e) => {
            let _receipt = SuggestedQuestionsCallReceipt {
                schema: "SuggestedQuestionsCallReceipt".to_string(),
                call_purpose: "generate_suggested_questions".to_string(),
                model: model.to_string(),
                provider: provider_type,
                duration_ms: elapsed.as_millis(),
                success: false,
                error_message: Some(e.to_string()),
                question_count: None,
            };
            Err(e)
        }
    }
}
