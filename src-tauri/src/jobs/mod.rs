use crate::db::notebook_db::NotebookDb;
use crate::providers::ollama::OllamaProvider;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tauri_queue::{JobContext, JobHandler, JobResult, QueueError};

/// Background jobs for Gloss.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlossJob {
    /// Generate a summary for a single source using the LLM.
    SummarizeSource {
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        ollama_url: String,
        model: String,
    },
}

impl JobHandler for GlossJob {
    async fn execute(&self, ctx: &JobContext) -> Result<JobResult, QueueError> {
        match self {
            GlossJob::SummarizeSource {
                notebook_id,
                source_id,
                source_title,
                data_dir,
                ollama_url,
                model,
            } => {
                execute_summarize(ctx, notebook_id, source_id, source_title, data_dir, ollama_url, model).await
            }
        }
    }

    fn job_type(&self) -> &str {
        match self {
            GlossJob::SummarizeSource { .. } => "SummarizeSource",
        }
    }
}

async fn execute_summarize(
    ctx: &JobContext,
    notebook_id: &str,
    source_id: &str,
    source_title: &str,
    data_dir: &str,
    ollama_url: &str,
    model: &str,
) -> Result<JobResult, QueueError> {
    let db_path = PathBuf::from(data_dir)
        .join("notebooks")
        .join(notebook_id)
        .join("notebook.db");

    let db = NotebookDb::open(&db_path).map_err(|e| QueueError::Execution(e.to_string()))?;

    // Load source content
    let source = db
        .get_source(source_id)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Skip if source already has a summary (dedup: prevents duplicate jobs from
    // re-generating summaries that were already completed by an earlier job).
    if source.summary.is_some() {
        tracing::debug!(source_id, "Source already has summary, skipping duplicate job");
        return Ok(JobResult::success_with_output(
            serde_json::json!({ "notebook_id": notebook_id, "source_id": source_id, "skipped": true }).to_string(),
        ));
    }

    let content = match source.content_text.as_deref() {
        Some(text) if !text.is_empty() => text.to_string(),
        _ => {
            tracing::debug!(source_id, "Source has no content, skipping summary");
            return Ok(JobResult::success_with_output(
                serde_json::json!({ "notebook_id": notebook_id, "source_id": source_id, "skipped": true }).to_string(),
            ));
        }
    };

    // Check cancellation before the LLM call
    if ctx.is_cancelled() {
        return Err(QueueError::Cancelled);
    }

    // Create provider and generate summary
    let provider = OllamaProvider::new(ollama_url);

    tracing::info!(source_id, source_title, model, "Generating summary");

    let summary =
        crate::ingestion::summarize::summarize_source(&content, source_title, &provider, model)
            .await
            .map_err(|e| QueueError::Execution(format!("Summary generation failed: {}", e)))?;

    // Store the summary
    db.update_source_summary(source_id, &summary, model)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    tracing::info!(
        source_id,
        summary_len = summary.len(),
        "Summary generated successfully"
    );

    Ok(JobResult::success_with_output(
        serde_json::json!({ "notebook_id": notebook_id }).to_string(),
    ))
}
