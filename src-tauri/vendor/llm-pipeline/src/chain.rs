//! Sequential chain of payloads.
//!
//! [`Chain`] composes multiple payloads into a sequential pipeline,
//! passing each payload's output `value` as the next payload's input.
//! For branching, loops, or parallel execution, use a graph runtime.

use crate::{
    error::Result,
    exec_ctx::ExecCtx,
    payload::{BoxFut, Payload, PayloadOutput},
    PipelineError,
};
use serde_json::Value;

/// A sequential chain of payloads.
///
/// Executes payloads in order, piping each output's `value` as the next
/// payload's input. This is the simplest composition primitive.
///
/// `Chain` itself implements [`Payload`], so chains can be nested.
///
/// # Example
///
/// ```ignore
/// use llm_pipeline::{Chain, LlmCall, ExecCtx};
/// use serde_json::json;
///
/// let chain = Chain::new("my-chain")
///     .push(Box::new(LlmCall::new("step1", "Analyze: {input}")))
///     .push(Box::new(LlmCall::new("step2", "Refine: {input}")));
///
/// let ctx = ExecCtx::builder("http://localhost:11434").build();
/// let output = chain.execute(&ctx, json!("some input")).await?;
/// let result: MyType = output.parse_as()?;
/// ```
pub struct Chain {
    name: String,
    payloads: Vec<Box<dyn Payload>>,
}

impl Chain {
    /// Create a new empty chain.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            payloads: Vec::new(),
        }
    }

    /// Add a payload to the end of the chain (builder style).
    pub fn push(mut self, payload: Box<dyn Payload>) -> Self {
        self.payloads.push(payload);
        self
    }

    /// Add a payload to the end of the chain (mutation style).
    pub fn add(&mut self, payload: Box<dyn Payload>) {
        self.payloads.push(payload);
    }

    /// Number of payloads in the chain.
    pub fn len(&self) -> usize {
        self.payloads.len()
    }

    /// Whether the chain is empty.
    pub fn is_empty(&self) -> bool {
        self.payloads.is_empty()
    }

    /// Execute all payloads sequentially, returning every intermediate output.
    ///
    /// The first payload receives `input`. Each subsequent payload receives
    /// the previous output's `value`.
    pub async fn execute_all(&self, ctx: &ExecCtx, input: Value) -> Result<Vec<PayloadOutput>> {
        if self.payloads.is_empty() {
            return Err(PipelineError::InvalidConfig(
                "Chain has no payloads".to_string(),
            ));
        }

        let mut outputs = Vec::with_capacity(self.payloads.len());
        let mut current = input;

        for payload in &self.payloads {
            ctx.check_cancelled()?;
            let output = payload.invoke(ctx, current).await?;
            current = output.value.clone();
            outputs.push(output);
        }

        Ok(outputs)
    }

    /// Execute all payloads and return only the final output.
    pub async fn execute(&self, ctx: &ExecCtx, input: Value) -> Result<PayloadOutput> {
        let outputs = self.execute_all(ctx, input).await?;
        outputs
            .into_iter()
            .last()
            .ok_or_else(|| PipelineError::Other("Chain produced no outputs".to_string()))
    }
}

impl Payload for Chain {
    fn kind(&self) -> &'static str {
        "chain"
    }

    fn name(&self) -> &str {
        &self.name
    }

    fn invoke<'a>(&'a self, ctx: &'a ExecCtx, input: Value) -> BoxFut<'a, Result<PayloadOutput>> {
        Box::pin(self.execute(ctx, input))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    /// A test payload that wraps input in a JSON object.
    struct EchoPayload {
        tag: String,
    }

    impl Payload for EchoPayload {
        fn kind(&self) -> &'static str {
            "echo"
        }
        fn name(&self) -> &str {
            &self.tag
        }
        fn invoke<'a>(
            &'a self,
            _ctx: &'a ExecCtx,
            input: Value,
        ) -> BoxFut<'a, Result<PayloadOutput>> {
            let tag = self.tag.clone();
            Box::pin(async move {
                let value = json!({ "from": tag, "input": input });
                Ok(PayloadOutput::from_value(value))
            })
        }
    }

    fn test_ctx() -> ExecCtx {
        ExecCtx::builder("http://test").build()
    }

    #[tokio::test]
    async fn test_chain_single_payload() {
        let chain = Chain::new("test").push(Box::new(EchoPayload { tag: "a".into() }));

        let out = chain.execute(&test_ctx(), json!("hello")).await.unwrap();
        assert_eq!(out.value["from"], "a");
        assert_eq!(out.value["input"], "hello");
    }

    #[tokio::test]
    async fn test_chain_pipes_output() {
        let chain = Chain::new("test")
            .push(Box::new(EchoPayload {
                tag: "first".into(),
            }))
            .push(Box::new(EchoPayload {
                tag: "second".into(),
            }));

        let out = chain.execute(&test_ctx(), json!("start")).await.unwrap();
        assert_eq!(out.value["from"], "second");
        assert_eq!(out.value["input"]["from"], "first");
    }

    #[tokio::test]
    async fn test_chain_execute_all() {
        let chain = Chain::new("test")
            .push(Box::new(EchoPayload { tag: "a".into() }))
            .push(Box::new(EchoPayload { tag: "b".into() }));

        let outputs = chain.execute_all(&test_ctx(), json!("x")).await.unwrap();
        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0].value["from"], "a");
        assert_eq!(outputs[1].value["from"], "b");
    }

    #[tokio::test]
    async fn test_chain_empty_fails() {
        let chain = Chain::new("empty");
        let result = chain.execute(&test_ctx(), json!(null)).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_chain_cancellation() {
        let cancel = Arc::new(AtomicBool::new(true));
        let ctx = ExecCtx::builder("http://test")
            .cancellation(Some(cancel))
            .build();

        let chain = Chain::new("test").push(Box::new(EchoPayload { tag: "a".into() }));
        let result = chain.execute(&ctx, json!("x")).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_chain_as_payload() {
        // Chain implements Payload, so it can be nested
        let inner = Chain::new("inner").push(Box::new(EchoPayload {
            tag: "inner-step".into(),
        }));

        let outer = Chain::new("outer").push(Box::new(inner));

        let out = outer.execute(&test_ctx(), json!("input")).await.unwrap();
        assert_eq!(out.value["from"], "inner-step");
    }
}
