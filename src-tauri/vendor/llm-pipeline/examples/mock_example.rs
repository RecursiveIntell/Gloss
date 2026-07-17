//! Example: using MockBackend for testing without a live LLM.
//!
//! Run with: `cargo run --example mock_example`

use llm_pipeline::payload::Payload;
use llm_pipeline::{ExecCtx, LlmCall, MockBackend};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct MovieReview {
    title: String,
    rating: f64,
    summary: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create a mock backend with a canned JSON response
    let mock = MockBackend::fixed(
        r#"{"title": "Inception", "rating": 9.2, "summary": "A mind-bending thriller about dreams within dreams."}"#,
    );

    // Build an execution context using the mock backend
    let ctx = ExecCtx::builder("http://unused")
        .backend(Arc::new(mock))
        .build();

    // Create an LlmCall that expects JSON output
    let call = LlmCall::new("review", "Review the movie: {input}").expecting_json();

    // Execute the call
    let output = call.invoke(&ctx, json!("Inception")).await?;

    // Parse into a typed struct
    let review: MovieReview = output.parse_as()?;
    println!("Movie: {}", review.title);
    println!("Rating: {}/10", review.rating);
    println!("Summary: {}", review.summary);

    // Check diagnostics
    if let Some(ref diag) = output.diagnostics {
        println!("Parse strategy: {:?}", diag.strategy);
        println!("Parse OK: {}", diag.ok());
    }

    Ok(())
}
