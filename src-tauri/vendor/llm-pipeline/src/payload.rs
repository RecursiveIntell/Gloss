//! Core payload trait and output types.
//!
//! Payloads are the fundamental execution unit in this crate. Each payload
//! takes a `serde_json::Value` input, does some work (typically an LLM call),
//! and returns a [`PayloadOutput`]. The `Value`-based wire type allows
//! heterogeneous workflows where each node produces a different shape.

use crate::diagnostics::ParseDiagnostics;
use crate::error::Result;
use crate::exec_ctx::ExecCtx;
#[allow(deprecated)]
use crate::trace::TraceId;
use crate::PipelineError;
use serde::de::DeserializeOwned;
use serde_json::Value;
use stack_ids::TraceCtx;
use std::future::Future;
use std::pin::Pin;

/// A boxed, pinned, Send future -- the return type of [`Payload::invoke`].
pub type BoxFut<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Object-safe trait for executable payloads.
///
/// Payloads are the building blocks for LLM workflows. They accept a
/// `Value` input, execute against an [`ExecCtx`], and return a
/// [`PayloadOutput`].
///
/// The trait is object-safe so payloads can be stored as `Box<dyn Payload>`
/// and composed in chains or graphs without generic constraints.
pub trait Payload: Send + Sync {
    /// Stable identifier for the payload type (e.g. `"llm-call"`, `"chain"`).
    fn kind(&self) -> &'static str;

    /// Instance name (for logging/debugging/events).
    fn name(&self) -> &str;

    /// Execute the payload.
    fn invoke<'a>(&'a self, ctx: &'a ExecCtx, input: Value) -> BoxFut<'a, Result<PayloadOutput>>;
}

/// Output from a payload invocation.
///
/// Uses `serde_json::Value` as the wire type so heterogeneous workflows
/// can pass data between nodes without sharing a single generic `T`.
#[derive(Debug, Clone)]
pub struct PayloadOutput {
    /// Structured output (JSON value).
    pub value: Value,
    /// Raw response text from the LLM (before parsing).
    pub raw_response: String,
    /// Extracted thinking/reasoning (if extended thinking was enabled).
    pub thinking: Option<String>,
    /// Model that produced this output (if applicable).
    pub model: Option<String>,
    /// Parse diagnostics (strategy used, errors, retry info).
    /// `None` for legacy code paths (Chain passthrough, `from_value`).
    pub diagnostics: Option<ParseDiagnostics>,
    /// Phase status: compatibility / migration-only
    ///
    /// Legacy trace ID correlating this output to its execution context.
    /// `None` for legacy code paths (Chain passthrough, `from_value`).
    /// Use [`trace_ctx`](Self::trace_ctx) for the canonical trace form.
    ///
    /// **Removal condition**: removed when all callers migrate to `TraceCtx`.
    pub trace_id: Option<TraceId>,
    /// Canonical trace context from `stack_ids`.
    ///
    /// Populated on the normal path by `LlmCall::invoke`. `None` for legacy
    /// code paths (Chain passthrough, `from_value`).
    pub trace_ctx: Option<TraceCtx>,
    /// Number of transport-level retries used (429, 5xx).
    pub transport_retries_used: u32,
    /// Number of semantic-level retries used (parse/validation failures).
    pub semantic_retries_used: u32,
    /// Response size in bytes.
    pub response_bytes: usize,
    /// Wall-clock time for the entire invocation in milliseconds.
    pub wall_time_ms: u64,
}

impl PayloadOutput {
    /// Create an output wrapping a pre-existing `Value`.
    pub fn from_value(value: Value) -> Self {
        let raw = value.to_string();
        Self {
            value,
            raw_response: raw,
            thinking: None,
            model: None,
            diagnostics: None,
            trace_id: None,
            trace_ctx: None,
            transport_retries_used: 0,
            semantic_retries_used: 0,
            response_bytes: 0,
            wall_time_ms: 0,
        }
    }

    /// Parse the output value into a typed `T`.
    ///
    /// This is the primary way to extract typed data at workflow edges.
    ///
    /// ```ignore
    /// let output: PayloadOutput = payload.invoke(&ctx, input).await?;
    /// let analysis: MyStruct = output.parse_as()?;
    /// ```
    pub fn parse_as<T: DeserializeOwned>(&self) -> Result<T> {
        serde_json::from_value(self.value.clone()).map_err(|e| {
            let snippet = self.value.to_string();
            let snippet = &snippet[..snippet.len().min(200)];
            PipelineError::Other(format!(
                "Failed to parse PayloadOutput into target type: {}. Value (truncated): {}",
                e, snippet
            ))
        })
    }
}
