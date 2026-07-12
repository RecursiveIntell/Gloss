use llm_pipeline::{Pipeline, PipelineInput, Stage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Solution {
    problem: String,
    approach: String,
    solution: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    // Use extended thinking for complex reasoning
    let pipeline = Pipeline::<Solution>::builder()
        .add_stage(
            Stage::new(
                "solve",
                "Solve this problem and return JSON with 'problem', \
                 'approach', and 'solution' fields:\n\n{input}",
            )
            .with_thinking(true) // Enable extended thinking (<think> tags)
            .with_model("deepseek-r1:8b")
            .with_json_mode(true),
        )
        .build()?;

    let input =
        PipelineInput::new("How can I optimize a binary search tree for better cache locality?");

    println!("Solving with extended thinking...\n");

    let result = pipeline
        .execute(&client, "http://localhost:11434", input)
        .await?;

    println!("=== Solution ===");
    println!("Problem: {}", result.final_output.problem);
    println!("\nApproach: {}", result.final_output.approach);
    println!("\nSolution: {}", result.final_output.solution);

    // Show thinking process if available
    if let Some(stage) = result.stage_results.first() {
        if let Some(ref thinking) = stage.thinking {
            println!("\n=== Thinking Process ===");
            println!("{}", thinking);
        }
    }

    Ok(())
}
