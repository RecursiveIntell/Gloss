# llm-output-parser

Production-grade parser for extracting structured data from LLM responses.

Handles think blocks, markdown fences, malformed JSON, and real-world model
output without an additional LLM call.

## Features

- Strip `<think>` blocks from LLM responses
- Extract JSON from markdown code fences
- Tolerant parsing of malformed JSON (trailing commas, single quotes, comments)
- No external LLM call needed — pure Rust parsing
- Optional YAML support behind the `yaml` feature

## Usage

```rust
use llm_output_parser::parse_json;

let raw = "```json\n{\"key\": \"value\"}\n```";
let result: serde_json::Value = parse_json(raw).unwrap();
```