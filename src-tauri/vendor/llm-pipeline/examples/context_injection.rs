use llm_pipeline::{Pipeline, PipelineContext, PipelineInput, Stage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PersonalizedContent {
    message: String,
    tone: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    // Create context with domain knowledge
    let context = PipelineContext::new()
        .insert("user_name", "Alice")
        .insert("expertise_level", "beginner")
        .insert("preferred_style", "conversational");

    let pipeline = Pipeline::<PersonalizedContent>::builder()
        .add_stage(
            Stage::new(
                "personalize",
                "Create a personalized message about {input} for {user_name}. \
                 Expertise level: {expertise_level}. Style: {preferred_style}. \
                 Return JSON with 'message' and 'tone'.",
            )
            .with_json_mode(true),
        )
        .with_context(context)
        .build()?;

    let input = PipelineInput::new("learning Rust programming");

    let result = pipeline
        .execute(&client, "http://localhost:11434", input)
        .await?;

    println!("Personalized Message:");
    println!("{}", result.final_output.message);
    println!("\nTone: {}", result.final_output.tone);

    Ok(())
}
