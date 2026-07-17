# llm-pipeline

Reusable node payloads for LLM workflows: prompt templating, multi-backend calls (Ollama/OpenAI), defensive parsing, streaming, and sequential chaining.

## Example

```rust
use llm_pipeline::{ExecCtx, LlmCall, OutputStrategy};

let ctx = ExecCtx::builder("http://localhost:11434").build()?;
let call = LlmCall::new("Summarize: {{text}}")
    .with_model("llama3.2:3b")
    .with_output_strategy(OutputStrategy::JsonObject);

let output = call.invoke(&ctx).await?;
println!("{}", output.value);
```

## Ecosystem

- **stack-ids**: `TraceCtx` for request correlation, `AttemptId`/`TrialId` for retry lineage
- **agent-graph**: LlmCall implements `Payload` trait for use as graph nodes
- **llm-output-parser**: Defensive JSON/text extraction from model responses

## stack-ids integration

Fully integrated. `TraceCtx` is re-exported from the crate root. Per-call trace context flows through the execution pipeline.
