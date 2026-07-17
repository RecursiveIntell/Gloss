use llm_pipeline::{Pipeline, PipelineInput, Stage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Analysis {
    summary: String,
    key_points: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    // Create a 2-stage pipeline
    let pipeline = Pipeline::<Analysis>::builder()
        .add_stage(
            Stage::new(
                "analyze",
                "Analyze the following text and return a JSON object with \
                 'summary' (string) and 'key_points' (array of strings):\n\n{input}",
            )
            .with_model("llama3.2:3b")
            .with_json_mode(true),
        )
        .add_stage(
            Stage::new(
                "refine",
                "Refine this analysis to be more concise. Return JSON with \
                 'summary' and 'key_points':\n\n{input}",
            )
            .with_model("llama3.2:3b")
            .with_json_mode(true),
        )
        .build()?;

    let input = PipelineInput::new(
        "Rust is a systems programming language focused on safety and performance.",
    );

    println!("Running pipeline...");
    let result = pipeline
        .execute(&client, "http://localhost:11434", input)
        .await?;

    println!("\nFinal Result:");
    println!("Summary: {}", result.final_output.summary);
    println!("Key Points:");
    for point in &result.final_output.key_points {
        println!("  - {}", point);
    }

    Ok(())
}
