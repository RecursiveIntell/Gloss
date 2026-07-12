//! Example: Payload chain with heterogeneous Value flow and typed parse.
//!
//! Demonstrates the new payload API:
//! - ExecCtx for shared runtime state
//! - LlmCall payloads with different output shapes
//! - Chain for sequential composition
//! - PayloadOutput::parse_as::<T>() for typed extraction at the end
//! - Optional EventHandler for streaming token hooks

use llm_pipeline::events::{Event, FnEventHandler};
use llm_pipeline::{Chain, ExecCtx, LlmCall, LlmConfig};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
struct FinalSummary {
    summary: String,
    key_points: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Build an execution context
    let ctx = ExecCtx::builder("http://localhost:11434")
        .var("domain", "technology")
        .var("audience", "engineers")
        // Optional: attach an event handler for streaming/lifecycle hooks
        .event_handler(Arc::new(FnEventHandler(|event: Event| match event {
            Event::PayloadStart { name, kind } => {
                eprintln!("[start] {} ({})", name, kind);
            }
            Event::Token { chunk, .. } => {
                eprint!("{}", chunk);
            }
            Event::PayloadEnd { name, ok } => {
                eprintln!("\n[end] {} ok={}", name, ok);
            }
            _ => {}
        })))
        .build();

    // 2. Build a chain of LlmCall payloads
    //    Each payload can produce different output shapes — the wire type is Value.
    let chain = Chain::new("analyze-and-summarize")
        .push(Box::new(
            LlmCall::new(
                "draft",
                "You are a {domain} expert writing for {audience}. \
                 Analyze the following and return a JSON object with 'topics' \
                 (array of strings) and 'raw_analysis' (string):\n\n{input}",
            )
            .with_model("llama3.2:3b")
            .with_config(LlmConfig::default().with_json_mode(true)),
        ))
        .push(Box::new(
            LlmCall::new(
                "summarize",
                "Given this analysis, produce a final JSON with 'summary' (string) \
                 and 'key_points' (array of strings):\n\n{input}",
            )
            .with_model("llama3.2:3b")
            .with_config(LlmConfig::default().with_json_mode(true))
            .with_streaming(true), // stream tokens for the final step
        ));

    // 3. Execute with a Value input
    let input = json!("Rust's ownership model and its impact on systems programming safety");

    println!("Running chain...\n");
    let output = chain.execute(&ctx, input).await?;

    // 4. Parse the final output into a typed struct
    let result: FinalSummary = output.parse_as()?;

    println!("\n\n=== Final Result ===");
    println!("Summary: {}", result.summary);
    println!("Key Points:");
    for point in &result.key_points {
        println!("  - {}", point);
    }

    // 5. You can also inspect metadata
    if let Some(ref model) = output.model {
        println!("\nModel: {}", model);
    }
    if let Some(ref thinking) = output.thinking {
        println!("\nThinking: {}", thinking);
    }

    Ok(())
}
