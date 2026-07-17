# Architecture

## Mission

This crate provides **reusable node payloads** for LLM workflows. A graph
runtime (e.g. LangGraph) owns routing, loops, concurrency, interrupts, and
checkpoints. This crate owns what runs _inside_ each node: prompt templating,
LLM provider calls, response parsing, streaming, and cancellation.

## Design Decisions

### Value-based wire type

All payloads communicate via `serde_json::Value`. This allows heterogeneous
workflows where node A outputs `{summary, points}` and node B outputs
`{score, feedback}` without shared generics. Typed extraction happens at
the edges via `PayloadOutput::parse_as::<T>()`.

### Object-safe Payload trait (no `async-trait`)

```rust
pub trait Payload: Send + Sync {
    fn kind(&self) -> &'static str;
    fn name(&self) -> &str;
    fn invoke<'a>(&'a self, ctx: &'a ExecCtx, input: Value)
        -> BoxFut<'a, Result<PayloadOutput>>;
}
```

Returns `Pin<Box<dyn Future + Send>>` so payloads can be stored as
`Box<dyn Payload>` and composed without generics. No `async-trait` dependency.

### Pipeline rebuilt on payloads

`Pipeline<T>` now internally converts `Stage` objects into `LlmCall` payloads
and executes them sequentially. The streaming path uses `StreamingDecoder`
for correct chunk-boundary handling. The public API is unchanged.

### Streaming bug fix

The original streaming code split on `text.lines()` per TCP chunk. A JSON
line split across chunks would silently fail to parse. `StreamingDecoder`
buffers partial lines and only yields complete newline-delimited JSON objects.
Both `LlmCall` (new API) and `Pipeline::execute_streaming` (compat) use it.

## Module Map

```
src/
  lib.rs          Root exports (new API primary, compat re-exports)
  payload.rs      Payload trait, PayloadOutput, BoxFut
  exec_ctx.rs     ExecCtx (client, base_url, vars, cancel, events)
  events.rs       Event enum, EventHandler trait
  llm_call.rs     LlmCall payload (Ollama generate/chat/streaming)
  chain.rs        Chain (sequential payload composition)
  parsing.rs      extract_thinking, JSON extraction, defensive parsing
  streaming.rs    StreamingDecoder (buffered NDJSON framing)
  prompt.rs       Template rendering helpers
  client.rs       LlmConfig + low-level call_llm* functions (uses parsing.rs)
  pipeline.rs     Pipeline<T> (rebuilt on LlmCall payloads)
  stage.rs        Stage + StageBuilder (compat, used by Pipeline)
  types.rs        PipelineInput, StageOutput, PipelineResult, etc.
  error.rs        PipelineError, Result
```

## Migration Path

**New code** should use:
- `ExecCtx` + `LlmCall` + `Chain` for sequential workflows
- `ExecCtx` + `LlmCall` + your graph runtime for complex workflows
- `PayloadOutput::parse_as::<T>()` for typed extraction

**Existing code** using `Pipeline<T>` + `Stage` continues to work unchanged.
Pipeline internally delegates to the payload layer.
