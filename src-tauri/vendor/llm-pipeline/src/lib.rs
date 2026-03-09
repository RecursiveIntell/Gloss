//! # LLM Pipeline
//!
//! Reusable node payloads for LLM workflows, with optional sequential chaining.
//!
//! This crate provides the building blocks for LLM-powered workflows:
//! **payloads** that execute LLM calls, **parsing utilities** for messy
//! model output, and a **chain** helper for sequential composition.
//!
//! Orchestration (routing, loops, concurrency, checkpoints) belongs in your
//! graph runtime (e.g. LangGraph). This crate provides what runs *inside*
//! each node.
//!
//! ## Core Concepts
//!
//! - **[`Payload`]** — object-safe trait for executable units. Takes a
//!   `serde_json::Value` input, returns a [`PayloadOutput`].
//! - **[`ExecCtx`]** — shared execution context (HTTP client, endpoint,
//!   template vars, cancellation, optional event handler).
//! - **[`LlmCall`]** — the primary payload: renders prompts, calls Ollama,
//!   parses responses.
//! - **[`Chain`]** — sequential composition of payloads.
//! - **[`PayloadOutput`]** — `Value`-based output with `parse_as::<T>()` for
//!   typed extraction at workflow edges.
//!
//! ## Quick Start (Payload API)
//!
//! ```no_run
//! use llm_pipeline::{LlmCall, Chain, ExecCtx};
//! use llm_pipeline::payload::Payload;
//! use serde::Deserialize;
//! use serde_json::json;
//!
//! #[derive(Debug, Deserialize)]
//! struct Analysis { summary: String }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let ctx = ExecCtx::builder("http://localhost:11434").build();
//!
//!     let chain = Chain::new("analyze")
//!         .push(Box::new(
//!             LlmCall::new("draft", "Analyze: {input}")
//!                 .with_config(llm_pipeline::LlmConfig::default().with_json_mode(true))
//!         ))
//!         .push(Box::new(
//!             LlmCall::new("refine", "Refine this analysis: {input}")
//!                 .with_config(llm_pipeline::LlmConfig::default().with_json_mode(true))
//!         ));
//!
//!     let output = chain.execute(&ctx, json!("Your text here")).await?;
//!     let result: Analysis = output.parse_as()?;
//!     println!("{}", result.summary);
//!     Ok(())
//! }
//! ```
//!
//! ## Pipeline API (compatibility)
//!
//! The original `Pipeline<T>` API is still available and works as before.
//! Internally it now uses [`LlmCall`] payloads.
//!
//! ```no_run
//! use llm_pipeline::{Pipeline, Stage, PipelineInput};
//! use serde::{Deserialize, Serialize};
//!
//! #[derive(Debug, Clone, Serialize, Deserialize)]
//! struct Analysis { summary: String, insights: Vec<String> }
//!
//! #[tokio::main]
//! async fn main() -> Result<(), Box<dyn std::error::Error>> {
//!     let client = reqwest::Client::new();
//!     let pipeline = Pipeline::<Analysis>::builder()
//!         .add_stage(Stage::new("analyze", "Analyze: {input}").with_json_mode(true))
//!         .add_stage(Stage::new("refine", "Refine: {input}").with_json_mode(true))
//!         .build()?;
//!
//!     let result = pipeline.execute(
//!         &client, "http://localhost:11434", PipelineInput::new("Your text"),
//!     ).await?;
//!     println!("{}", result.final_output.summary);
//!     Ok(())
//! }
//! ```

// --- New payload layer ---
pub mod backend;
pub mod chain;
pub mod diagnostics;
pub mod events;
#[allow(deprecated)]
pub mod exec_ctx;
pub mod limits;
pub mod llm_call;
pub mod output_parser;
pub mod output_strategy;
pub mod parsing;
#[allow(deprecated)]
pub mod payload;
pub mod retry;
pub mod retry_policy;
pub mod streaming;
#[allow(deprecated)]
pub mod trace;

// --- Original modules (still public) ---
pub mod client;
pub mod error;
#[allow(deprecated)]
pub mod pipeline;
pub mod prompt;
pub mod stage;
pub mod types;

// --- Primary exports: new payload API ---
#[cfg(feature = "openai")]
pub use backend::OpenAiBackend;
pub use backend::{BackoffConfig, MockBackend, OllamaBackend, RecordingBackend};
pub use chain::Chain;
pub use diagnostics::ParseDiagnostics;
pub use exec_ctx::{ExecCtx, ExecCtxBuilder};
pub use limits::PipelineLimits;
pub use llm_call::LlmCall;
pub use output_strategy::OutputStrategy;
pub use payload::{BoxFut, Payload, PayloadOutput};
pub use retry::RetryConfig;
pub use retry_policy::{SemanticRetryPolicy, TransportRetryPolicy};
pub use streaming::StreamingDecoder;
#[allow(deprecated)]
pub use trace::TraceId;

// --- Canonical cross-crate trace/identity re-exports from stack-ids ---
pub use stack_ids::TraceCtx;

// --- Re-exports: original API (compatibility) ---
pub use client::LlmConfig;
pub use error::{PipelineError, Result};
pub use pipeline::{Pipeline, PipelineBuilder};
pub use stage::{Stage, StageBuilder};
pub use types::{PipelineContext, PipelineInput, PipelineProgress, PipelineResult, StageOutput};
