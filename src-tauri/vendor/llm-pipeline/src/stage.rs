use crate::{client::LlmConfig, error::Result, types::PipelineContext, PipelineError};

/// A single stage in the pipeline.
#[derive(Clone)]
pub struct Stage {
    /// Name of the stage (for logging/progress).
    pub name: String,

    /// Prompt template with `{input}` and context `{key}` placeholders.
    pub prompt_template: String,

    /// Optional system prompt (used with `/api/chat` endpoint).
    pub system_prompt: Option<String>,

    /// Model to use for this stage.
    pub model: String,

    /// LLM configuration.
    pub config: LlmConfig,

    /// Whether this stage is enabled.
    pub enabled: bool,
}

impl Stage {
    /// Create a new stage with a prompt template.
    ///
    /// The template supports `{input}` for the previous stage's output
    /// and `{key}` for any context values.
    pub fn new(name: impl Into<String>, prompt_template: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            prompt_template: prompt_template.into(),
            system_prompt: None,
            model: "llama3.2:3b".to_string(),
            config: LlmConfig::default(),
            enabled: true,
        }
    }

    /// Set the model for this stage.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = model.into();
        self
    }

    /// Set a system prompt (enables `/api/chat` mode).
    pub fn with_system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = Some(prompt.into());
        self
    }

    /// Enable extended thinking mode.
    pub fn with_thinking(mut self, enabled: bool) -> Self {
        self.config.thinking = enabled;
        self
    }

    /// Set temperature.
    pub fn with_temperature(mut self, temp: f64) -> Self {
        self.config.temperature = temp;
        self
    }

    /// Set max tokens.
    pub fn with_max_tokens(mut self, tokens: u32) -> Self {
        self.config.max_tokens = tokens;
        self
    }

    /// Enable JSON format mode.
    pub fn with_json_mode(mut self, enabled: bool) -> Self {
        self.config.json_mode = enabled;
        self
    }

    /// Set the full LLM config.
    pub fn with_config(mut self, config: LlmConfig) -> Self {
        self.config = config;
        self
    }

    /// Disable this stage (it will be skipped during execution).
    pub fn disabled(mut self) -> Self {
        self.enabled = false;
        self
    }

    /// Render the prompt template with input and context values.
    pub fn render_prompt(&self, input: &str, context: &PipelineContext) -> String {
        let mut rendered = self.prompt_template.clone();
        rendered = rendered.replace("{input}", input);
        for (key, value) in &context.data {
            let placeholder = format!("{{{}}}", key);
            rendered = rendered.replace(&placeholder, value);
        }
        rendered
    }

    /// Render the system prompt with context values (if set).
    pub fn render_system_prompt(&self, context: &PipelineContext) -> Option<String> {
        self.system_prompt.as_ref().map(|sp| {
            let mut rendered = sp.clone();
            for (key, value) in &context.data {
                let placeholder = format!("{{{}}}", key);
                rendered = rendered.replace(&placeholder, value);
            }
            rendered
        })
    }
}

/// Builder for creating stages with validation.
pub struct StageBuilder {
    stage: Stage,
}

impl StageBuilder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            stage: Stage {
                name: name.into(),
                prompt_template: String::new(),
                system_prompt: None,
                model: "llama3.2:3b".to_string(),
                config: LlmConfig::default(),
                enabled: true,
            },
        }
    }

    pub fn prompt(mut self, template: impl Into<String>) -> Self {
        self.stage.prompt_template = template.into();
        self
    }

    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.stage.system_prompt = Some(prompt.into());
        self
    }

    pub fn model(mut self, model: impl Into<String>) -> Self {
        self.stage.model = model.into();
        self
    }

    pub fn thinking(mut self, enabled: bool) -> Self {
        self.stage.config.thinking = enabled;
        self
    }

    pub fn temperature(mut self, temp: f64) -> Self {
        self.stage.config.temperature = temp;
        self
    }

    pub fn max_tokens(mut self, tokens: u32) -> Self {
        self.stage.config.max_tokens = tokens;
        self
    }

    pub fn json_mode(mut self, enabled: bool) -> Self {
        self.stage.config.json_mode = enabled;
        self
    }

    pub fn enabled(mut self, enabled: bool) -> Self {
        self.stage.enabled = enabled;
        self
    }

    pub fn build(self) -> Result<Stage> {
        if self.stage.prompt_template.is_empty() {
            return Err(PipelineError::InvalidConfig(
                "Stage prompt template cannot be empty".to_string(),
            ));
        }
        Ok(self.stage)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        assert!(stage.enabled);
    }

    #[test]
    fn test_stage_disabled() {
        let stage = Stage::new("test", "prompt").disabled();
        assert!(!stage.enabled);
    }

    #[test]
    fn test_prompt_rendering() {
        let context = PipelineContext::new()
            .insert("name", "Alice")
            .insert("age", "30");

        let stage = Stage::new("test", "Hello {name}, you are {age}. Input: {input}");
        let rendered = stage.render_prompt("test input", &context);
        assert_eq!(rendered, "Hello Alice, you are 30. Input: test input");
    }

    #[test]
    fn test_prompt_rendering_no_context() {
        let context = PipelineContext::new();
        let stage = Stage::new("test", "Process: {input}");
        let rendered = stage.render_prompt("my data", &context);
        assert_eq!(rendered, "Process: my data");
    }

    #[test]
    fn test_system_prompt_rendering() {
        let context = PipelineContext::new().insert("domain", "medical");
        let stage = Stage::new("test", "{input}").with_system_prompt("You are a {domain} expert.");
        let rendered = stage.render_system_prompt(&context);
        assert_eq!(rendered, Some("You are a medical expert.".to_string()));
    }

    #[test]
    fn test_system_prompt_none() {
        let stage = Stage::new("test", "{input}");
        let rendered = stage.render_system_prompt(&PipelineContext::new());
        assert!(rendered.is_none());
    }

    #[test]
    fn test_stage_builder_success() {
        let stage = StageBuilder::new("builder-test")
            .prompt("Do something with {input}")
            .model("my-model")
            .thinking(true)
            .temperature(0.5)
            .max_tokens(1024)
            .json_mode(true)
            .build();

        assert!(stage.is_ok());
        let stage = stage.unwrap();
        assert_eq!(stage.name, "builder-test");
        assert_eq!(stage.model, "my-model");
        assert!(stage.config.thinking);
        assert_eq!(stage.config.temperature, 0.5);
        assert_eq!(stage.config.max_tokens, 1024);
        assert!(stage.config.json_mode);
    }

    #[test]
    fn test_stage_builder_empty_prompt_fails() {
        let result = StageBuilder::new("test").model("model").build();
        assert!(result.is_err());
    }
}
