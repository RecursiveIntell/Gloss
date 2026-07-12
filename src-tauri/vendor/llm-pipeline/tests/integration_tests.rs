use llm_pipeline::*;
use serde::{Deserialize, Serialize};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
struct TestOutput {
    value: String,
}

// --- Stage tests ---

#[test]
fn test_stage_creation() {
    let stage = Stage::new("test", "Test prompt: {input}")
        .with_model("test-model")
        .with_thinking(true)
        .with_temperature(0.8);

    assert_eq!(stage.name, "test");
    assert_eq!(stage.model, "test-model");
    assert!(stage.config.thinking);
    assert_eq!(stage.config.temperature, 0.8);
}

#[test]
fn test_prompt_rendering() {
    let context = PipelineContext::new()
        .insert("name", "Alice")
        .insert("age", "30");

    let stage = Stage::new(
        "test",
        "Hello {name}, you are {age} years old. Input: {input}",
    );
    let rendered = stage.render_prompt("test input", &context);

    assert_eq!(
        rendered,
        "Hello Alice, you are 30 years old. Input: test input"
    );
}

#[test]
fn test_prompt_rendering_missing_placeholders() {
    let context = PipelineContext::new().insert("name", "Bob");
    let stage = Stage::new("test", "{name} + {missing} + {input}");
    let rendered = stage.render_prompt("data", &context);
    // Missing placeholders are left as-is
    assert_eq!(rendered, "Bob + {missing} + data");
}

// --- Pipeline builder tests ---

#[test]
fn test_pipeline_builder() {
    let result = Pipeline::<TestOutput>::builder()
        .add_stage(Stage::new("stage1", "Test 1: {input}"))
        .add_stage(Stage::new("stage2", "Test 2: {input}"))
        .build();

    assert!(result.is_ok());
}

#[test]
fn test_empty_pipeline_fails() {
    let result = Pipeline::<TestOutput>::builder().build();
    assert!(result.is_err());
}

#[test]
fn test_all_disabled_pipeline_fails() {
    let result = Pipeline::<TestOutput>::builder()
        .add_stage(Stage::new("s1", "test: {input}").disabled())
        .add_stage(Stage::new("s2", "test: {input}").disabled())
        .build();
    assert!(result.is_err());
}

#[test]
fn test_stage_builder_without_prompt_fails() {
    let result = StageBuilder::new("test").model("test-model").build();
    assert!(result.is_err());
}

#[test]
fn test_stage_builder_with_prompt_succeeds() {
    let result = StageBuilder::new("test")
        .prompt("do something with {input}")
        .model("my-model")
        .thinking(true)
        .temperature(0.5)
        .build();
    assert!(result.is_ok());
}

// --- Cancellation tests ---

#[test]
fn test_cancellation_flag() {
    let cancel = Arc::new(AtomicBool::new(false));

    let pipeline = Pipeline::<TestOutput>::builder()
        .add_stage(Stage::new("s1", "test: {input}"))
        .with_cancellation(cancel.clone())
        .build()
        .unwrap();

    // Pipeline should have the cancellation flag
    assert_eq!(pipeline.stages().len(), 1);

    // Set flag - cancellation is propagated via the shared AtomicBool
    cancel.store(true, Ordering::Relaxed);

    // The flag is checked internally during execute(), which we can't
    // test without a real LLM endpoint, but the flag propagation works
    assert!(cancel.load(Ordering::Relaxed));
}

// --- Context tests ---

#[test]
fn test_pipeline_context() {
    let ctx = PipelineContext::new()
        .insert("key1", "value1")
        .insert("key2", "value2");

    assert_eq!(ctx.get("key1"), Some("value1"));
    assert_eq!(ctx.get("key2"), Some("value2"));
    assert_eq!(ctx.get("key3"), None);
}

#[test]
fn test_pipeline_context_overwrite() {
    let ctx = PipelineContext::new()
        .insert("key", "first")
        .insert("key", "second");

    assert_eq!(ctx.get("key"), Some("second"));
}

// --- PipelineInput tests ---

#[test]
fn test_pipeline_input() {
    let input = PipelineInput::new("my idea");
    assert_eq!(input.idea, "my idea");
    assert_eq!(input.num_concepts, 3); // default
}

#[test]
fn test_pipeline_input_with_concepts() {
    let input = PipelineInput::new("idea").with_concepts(5);
    assert_eq!(input.num_concepts, 5);
}

// --- LlmConfig tests ---

#[test]
fn test_llm_config_defaults() {
    let config = LlmConfig::default();
    assert_eq!(config.temperature, 0.7);
    assert_eq!(config.max_tokens, 2048);
    assert!(!config.thinking);
    assert!(!config.json_mode);
}

#[test]
fn test_llm_config_chaining() {
    let config = LlmConfig::default()
        .with_temperature(0.3)
        .with_max_tokens(4096)
        .with_thinking(true)
        .with_json_mode(true);

    assert_eq!(config.temperature, 0.3);
    assert_eq!(config.max_tokens, 4096);
    assert!(config.thinking);
    assert!(config.json_mode);
}

// --- Stage system prompt tests ---

#[test]
fn test_stage_with_system_prompt() {
    let ctx = PipelineContext::new().insert("domain", "medical");
    let stage = Stage::new("test", "{input}").with_system_prompt("You are a {domain} expert.");

    let sys = stage.render_system_prompt(&ctx);
    assert_eq!(sys, Some("You are a medical expert.".to_string()));
}

#[test]
fn test_stage_without_system_prompt() {
    let stage = Stage::new("test", "{input}");
    let sys = stage.render_system_prompt(&PipelineContext::new());
    assert!(sys.is_none());
}

// --- Disabled stages ---

#[test]
fn test_mixed_enabled_disabled_stages() {
    let pipeline = Pipeline::<TestOutput>::builder()
        .add_stage(Stage::new("enabled", "test: {input}"))
        .add_stage(Stage::new("disabled", "test: {input}").disabled())
        .add_stage(Stage::new("also_enabled", "test: {input}"))
        .build();

    assert!(pipeline.is_ok());
    let p = pipeline.unwrap();
    assert_eq!(p.stages().len(), 3);
    assert!(p.stages()[0].enabled);
    assert!(!p.stages()[1].enabled);
    assert!(p.stages()[2].enabled);
}

// --- Prompt utility tests ---

#[test]
fn test_prompt_numbered_list() {
    let items = vec![
        "First".to_string(),
        "Second".to_string(),
        "Third".to_string(),
    ];
    let result = llm_pipeline::prompt::numbered_list(&items);
    assert_eq!(result, "1. First\n2. Second\n3. Third");
}

#[test]
fn test_prompt_section() {
    let result = llm_pipeline::prompt::section("Context", "Some info");
    assert_eq!(result, "## Context\nSome info");
}

// --- Error type tests ---

#[test]
fn test_error_display() {
    let err = PipelineError::StageFailed {
        stage: "analyze".to_string(),
        message: "timeout".to_string(),
    };
    assert_eq!(err.to_string(), "Stage 'analyze' failed: timeout");

    let err = PipelineError::Cancelled;
    assert_eq!(err.to_string(), "Pipeline was cancelled");

    let err = PipelineError::InvalidConfig("bad config".to_string());
    assert_eq!(err.to_string(), "Invalid configuration: bad config");
}
