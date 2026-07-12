use llm_pipeline::{Pipeline, PipelineInput, Stage};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Story {
    title: String,
    content: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let pipeline = Pipeline::<Story>::builder()
        .add_stage(
            Stage::new(
                "write",
                "Write a short story about: {input}. Return JSON with 'title' and 'content'.",
            )
            .with_json_mode(true),
        )
        .add_stage(
            Stage::new(
                "polish",
                "Polish this story. Return JSON with 'title' and 'content':\n\n{input}",
            )
            .with_json_mode(true),
        )
        .build()?;

    let input = PipelineInput::new("a robot learning to paint");

    println!("Running pipeline with progress...\n");

    let result = pipeline
        .execute_streaming(
            &client,
            "http://localhost:11434",
            input,
            |progress| {
                println!(
                    "[{}/{}] Starting stage: {}",
                    progress.stage_index + 1,
                    progress.total_stages,
                    progress.stage_name
                );
            },
            |stage_idx, token| {
                eprint!("{}", token);
                let _ = stage_idx;
            },
        )
        .await?;

    println!("\n\n=== Final Story ===");
    println!("Title: {}", result.final_output.title);
    println!("\n{}", result.final_output.content);

    Ok(())
}
