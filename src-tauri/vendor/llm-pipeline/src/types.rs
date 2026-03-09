use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Input to a pipeline execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineInput {
    /// The main input text/idea.
    pub idea: String,

    /// Number of variations to generate (for ideation stages).
    pub num_concepts: u32,
}

impl PipelineInput {
    pub fn new(idea: impl Into<String>) -> Self {
        Self {
            idea: idea.into(),
            num_concepts: 3,
        }
    }

    pub fn with_concepts(mut self, num: u32) -> Self {
        self.num_concepts = num;
        self
    }
}

/// Output from a single stage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageOutput<T> {
    /// The structured output parsed from the LLM response.
    pub output: T,

    /// Thinking process (if extended thinking was enabled).
    pub thinking: Option<String>,

    /// Raw response text from the LLM.
    pub raw_response: String,
}

/// Complete pipeline execution result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineResult<T> {
    /// Final output from the last executed stage.
    pub final_output: T,

    /// Results from each executed stage in order.
    pub stage_results: Vec<StageOutput<T>>,

    /// Which stages were enabled during execution (indexed by stage position).
    pub stages_enabled: Vec<bool>,
}

/// Progress update emitted during pipeline execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineProgress {
    /// Current stage index (0-based).
    pub stage_index: usize,

    /// Total number of stages in the pipeline.
    pub total_stages: usize,

    /// Name of the current stage.
    pub stage_name: String,

    /// Current step within the stage (optional).
    pub current_step: Option<u32>,

    /// Total steps in the stage (optional).
    pub total_steps: Option<u32>,
}

/// Context that can be injected into prompt templates via `{key}` placeholders.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PipelineContext {
    pub data: HashMap<String, String>,
}

impl PipelineContext {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.data.insert(key.into(), value.into());
        self
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.data.get(key).map(|s| s.as_str())
    }
}
