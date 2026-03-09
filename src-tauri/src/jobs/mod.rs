use crate::db::notebook_db::NotebookDb;
use crate::ingestion::chunk::chunk_text_with_title;
use crate::providers::ollama::OllamaProvider;
use base64::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri_queue::{JobContext, JobHandler, JobResult, QueueError, QueueManager};

/// Background jobs for Gloss.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlossJob {
    /// Generate a summary for a single source using the LLM.
    SummarizeSource {
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        ollama_url: String,
        model: String,
    },
    /// Describe an image using a vision-capable LLM.
    DescribeImage {
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        ollama_url: String,
        model: String,
    },
    /// Extract frames from a video and describe them using a vision model.
    DescribeVideo {
        #[serde(default)]
        epoch: u64,
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
                epoch: _,
                notebook_id,
                source_id,
                source_title,
                data_dir,
                ollama_url,
                model,
            } => {
                execute_summarize(
                    ctx,
                    notebook_id,
                    source_id,
                    source_title,
                    data_dir,
                    ollama_url,
                    model,
                )
                .await
            }
            GlossJob::DescribeImage {
                epoch: _,
                notebook_id,
                source_id,
                source_title,
                data_dir,
                ollama_url,
                model,
            } => {
                execute_describe_image(
                    ctx,
                    notebook_id,
                    source_id,
                    source_title,
                    data_dir,
                    ollama_url,
                    model,
                )
                .await
            }
            GlossJob::DescribeVideo {
                epoch: _,
                notebook_id,
                source_id,
                source_title,
                data_dir,
                ollama_url,
                model,
            } => {
                execute_describe_video(
                    ctx,
                    notebook_id,
                    source_id,
                    source_title,
                    data_dir,
                    ollama_url,
                    model,
                )
                .await
            }
        }
    }

    fn job_type(&self) -> &str {
        match self {
            GlossJob::SummarizeSource { .. } => "SummarizeSource",
            GlossJob::DescribeImage { .. } => "DescribeImage",
            GlossJob::DescribeVideo { .. } => "DescribeVideo",
        }
    }
}

impl GlossJob {
    pub fn notebook_id(&self) -> &str {
        match self {
            GlossJob::SummarizeSource { notebook_id, .. }
            | GlossJob::DescribeImage { notebook_id, .. }
            | GlossJob::DescribeVideo { notebook_id, .. } => notebook_id,
        }
    }

    pub fn epoch(&self) -> u64 {
        match self {
            GlossJob::SummarizeSource { epoch, .. }
            | GlossJob::DescribeImage { epoch, .. }
            | GlossJob::DescribeVideo { epoch, .. } => *epoch,
        }
    }
}

pub(crate) fn cancel_jobs_matching<F>(queue: &Arc<QueueManager>, mut should_cancel: F) -> u32
where
    F: FnMut(&GlossJob, &str) -> bool,
{
    let jobs = match queue.list_jobs_with_data() {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to inspect queue jobs");
            return 0;
        }
    };

    let mut cancelled = 0u32;
    for (job_id, status, data_json) in jobs {
        if !matches!(status.as_str(), "pending" | "processing") {
            continue;
        }

        let job = match serde_json::from_str::<GlossJob>(&data_json) {
            Ok(job) => job,
            Err(e) => {
                tracing::warn!(job_id, error = %e, "Failed to deserialize queue job");
                continue;
            }
        };

        if !should_cancel(&job, &status) {
            continue;
        }

        match queue.cancel(&job_id) {
            Ok(()) => cancelled += 1,
            Err(e) => tracing::debug!(job_id, error = %e, "Queue cancellation skipped"),
        }
    }

    cancelled
}

pub(crate) fn cancel_jobs_not_matching_active_notebook(
    queue: &Arc<QueueManager>,
    active_notebook_id: Option<&str>,
    active_epoch: u64,
) -> u32 {
    cancel_jobs_matching(queue, |job, _status| match active_notebook_id {
        Some(active_notebook_id) => {
            job.notebook_id() != active_notebook_id || job.epoch() != active_epoch
        }
        None => true,
    })
}

pub(crate) fn has_jobs_for_notebook_epoch(
    queue: &Arc<QueueManager>,
    notebook_id: &str,
    epoch: u64,
) -> bool {
    match queue.list_jobs_with_data() {
        Ok(jobs) => jobs.into_iter().any(|(_job_id, status, data_json)| {
            if !matches!(status.as_str(), "pending" | "processing") {
                return false;
            }
            match serde_json::from_str::<GlossJob>(&data_json) {
                Ok(job) => job.notebook_id() == notebook_id && job.epoch() == epoch,
                Err(_) => false,
            }
        }),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to inspect queue jobs for dedup");
            false
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

    // If the notebook has been deleted, skip gracefully instead of erroring
    if !db_path.exists() {
        tracing::info!(
            notebook_id,
            source_id,
            "Notebook deleted, skipping summary job"
        );
        return Ok(JobResult::success_with_output(
            serde_json::json!({ "notebook_id": notebook_id, "source_id": source_id, "skipped": true }).to_string(),
        ));
    }

    let db = NotebookDb::connect(&db_path).map_err(|e| QueueError::Execution(e.to_string()))?;

    // Load source content
    let source = db
        .get_source(source_id)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Skip if source already has a summary (dedup: prevents duplicate jobs from
    // re-generating summaries that were already completed by an earlier job).
    if source.summary.is_some() {
        tracing::debug!(
            source_id,
            "Source already has summary, skipping duplicate job"
        );
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

    let summary_future =
        crate::ingestion::summarize::summarize_source(&content, source_title, &provider, model);
    tokio::pin!(summary_future);
    let summary = loop {
        if ctx.is_cancelled() {
            return Err(QueueError::Cancelled);
        }

        tokio::select! {
            result = &mut summary_future => {
                break result
                    .map_err(|e| QueueError::Execution(format!("Summary generation failed: {}", e)))?;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
        }
    };

    if ctx.is_cancelled() {
        return Err(QueueError::Cancelled);
    }

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

async fn execute_describe_image(
    ctx: &JobContext,
    notebook_id: &str,
    source_id: &str,
    source_title: &str,
    data_dir: &str,
    ollama_url: &str,
    model: &str,
) -> Result<JobResult, QueueError> {
    let nb_dir = PathBuf::from(data_dir).join("notebooks").join(notebook_id);
    let db_path = nb_dir.join("notebook.db");

    if !db_path.exists() {
        tracing::info!(
            notebook_id,
            source_id,
            "Notebook deleted, skipping describe job"
        );
        return Ok(JobResult::success_with_output(
            serde_json::json!({ "notebook_id": notebook_id, "source_id": source_id, "skipped": true }).to_string(),
        ));
    }

    let db = NotebookDb::connect(&db_path).map_err(|e| QueueError::Execution(e.to_string()))?;
    let source = db
        .get_source(source_id)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Skip if already described
    if source.content_text.is_some() && source.status != "pending" {
        tracing::debug!(source_id, "Image already described, skipping");
        return Ok(JobResult::success_with_output(
            serde_json::json!({
                "notebook_id": notebook_id,
                "source_id": source_id,
                "skipped": true
            })
            .to_string(),
        ));
    }

    // Read image file and base64 encode
    let file_path = source.file_path.as_deref().ok_or_else(|| {
        QueueError::Execution(format!("Image source {} has no file_path", source_id))
    })?;
    let full_path = nb_dir.join("sources").join(file_path);
    let full_path_clone = full_path.clone();
    let image_bytes = tokio::task::spawn_blocking(move || std::fs::read(&full_path_clone))
        .await
        .map_err(|e| QueueError::Execution(e.to_string()))?
        .map_err(|e| {
            QueueError::Execution(format!(
                "Failed to read image {}: {}",
                full_path.display(),
                e
            ))
        })?;
    let image_base64 = BASE64_STANDARD.encode(&image_bytes);

    if ctx.is_cancelled() {
        return Err(QueueError::Cancelled);
    }

    // Update status to describing
    db.update_source_status(source_id, "describing", None)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    tracing::info!(
        source_id,
        source_title,
        model,
        "Describing image with vision model"
    );

    // Call vision model
    let provider = OllamaProvider::new(ollama_url);
    let description_future =
        crate::ingestion::vision::describe_image(&image_base64, source_title, &provider, model);
    tokio::pin!(description_future);
    let description = loop {
        if ctx.is_cancelled() {
            let _ = db.update_source_status(source_id, "pending", None);
            return Err(QueueError::Cancelled);
        }

        tokio::select! {
            result = &mut description_future => {
                break result.map_err(|e| {
                    // Reset status on failure
                    let _ = db.update_source_status(source_id, "error", Some(&e.to_string()));
                    QueueError::Execution(format!("Vision description failed: {}", e))
                })?;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
        }
    };

    if ctx.is_cancelled() {
        let _ = db.update_source_status(source_id, "pending", None);
        return Err(QueueError::Cancelled);
    }

    // Store description as content_text
    let word_count = description.split_whitespace().count() as i32;
    db.update_source_content(source_id, &description, word_count)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Create chunks from the description
    let chunks = chunk_text_with_title(&description, source_id, source_title);
    for chunk_data in &chunks {
        let chunk = crate::db::notebook_db::Chunk {
            id: chunk_data.id.clone(),
            source_id: source_id.to_string(),
            chunk_index: chunk_data.chunk_index,
            content: chunk_data.content.clone(),
            token_count: chunk_data.token_count,
            start_offset: chunk_data.start_offset,
            end_offset: chunk_data.end_offset,
            metadata: chunk_data.metadata.clone(),
            embedding_id: None,
            embedding_model: None,
        };
        db.insert_chunk(&chunk)
            .map_err(|e| QueueError::Execution(e.to_string()))?;
    }

    // Mark ready immediately. Chat falls back to DB chunks for sources without
    // semantic embeddings, which avoids running more native indexing code
    // during large folder imports.
    db.update_source_status(source_id, "ready", None)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    tracing::info!(
        source_id,
        description_len = description.len(),
        chunks = chunks.len(),
        "Image described and chunked"
    );

    Ok(JobResult::success_with_output(
        serde_json::json!({
            "notebook_id": notebook_id,
            "source_id": source_id,
            "job_type": "DescribeImage"
        })
        .to_string(),
    ))
}

/// Maximum number of frames to extract from a video.
const MAX_VIDEO_FRAMES: usize = 10;

async fn execute_describe_video(
    ctx: &JobContext,
    notebook_id: &str,
    source_id: &str,
    source_title: &str,
    data_dir: &str,
    ollama_url: &str,
    model: &str,
) -> Result<JobResult, QueueError> {
    let nb_dir = PathBuf::from(data_dir).join("notebooks").join(notebook_id);
    let db_path = nb_dir.join("notebook.db");

    if !db_path.exists() {
        tracing::info!(
            notebook_id,
            source_id,
            "Notebook deleted, skipping video job"
        );
        return Ok(JobResult::success_with_output(
            serde_json::json!({ "notebook_id": notebook_id, "source_id": source_id, "skipped": true }).to_string(),
        ));
    }

    let db = NotebookDb::connect(&db_path).map_err(|e| QueueError::Execution(e.to_string()))?;
    let source = db
        .get_source(source_id)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Skip if already described
    if source.content_text.is_some() && source.status != "pending" {
        tracing::debug!(source_id, "Video already described, skipping");
        return Ok(JobResult::success_with_output(
            serde_json::json!({
                "notebook_id": notebook_id,
                "source_id": source_id,
                "skipped": true
            })
            .to_string(),
        ));
    }

    let file_path = source.file_path.as_deref().ok_or_else(|| {
        QueueError::Execution(format!("Video source {} has no file_path", source_id))
    })?;
    let full_path = nb_dir.join("sources").join(file_path);

    // Check that ffmpeg is available (async process)
    let ffmpeg_ok = tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await
        .map(|s| s.success())
        .unwrap_or(false);
    if !ffmpeg_ok {
        let msg = "ffmpeg not found — install ffmpeg to enable video frame analysis";
        let _ = db.update_source_status(source_id, "error", Some(msg));
        return Err(QueueError::Execution(msg.to_string()));
    }

    db.update_source_status(source_id, "describing", None)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    if ctx.is_cancelled() {
        let _ = db.update_source_status(source_id, "pending", None);
        return Err(QueueError::Cancelled);
    }

    // Get video duration with ffprobe
    let duration_secs = get_video_duration(&full_path).await.unwrap_or(60.0);
    let frame_interval = (duration_secs / MAX_VIDEO_FRAMES as f64).max(1.0);

    tracing::info!(
        source_id,
        source_title,
        model,
        duration_secs,
        frame_interval,
        "Extracting frames from video"
    );

    // Create temp directory for frames
    let temp_dir = nb_dir.join("_tmp_frames_").join(source_id);
    let temp_dir_clone = temp_dir.clone();
    tokio::task::spawn_blocking(move || std::fs::create_dir_all(&temp_dir_clone))
        .await
        .map_err(|e| QueueError::Execution(e.to_string()))?
        .map_err(|e| QueueError::Execution(format!("Failed to create temp dir: {}", e)))?;

    // Extract frames with ffmpeg (async process)
    let frame_pattern = temp_dir.join("frame_%04d.jpg");
    let ffmpeg_result = tokio::process::Command::new("ffmpeg")
        .args([
            "-i",
            &full_path.to_string_lossy(),
            "-vf",
            &format!("fps=1/{}", frame_interval as u32),
            "-frames:v",
            &MAX_VIDEO_FRAMES.to_string(),
            "-q:v",
            "2", // high quality JPEG
            &frame_pattern.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .status()
        .await;

    match ffmpeg_result {
        Ok(status) if !status.success() => {
            let td = temp_dir.clone();
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
            let msg = format!("ffmpeg exited with status {}", status);
            let _ = db.update_source_status(source_id, "error", Some(&msg));
            return Err(QueueError::Execution(msg));
        }
        Err(e) => {
            let td = temp_dir.clone();
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
            let msg = format!("Failed to run ffmpeg: {}", e);
            let _ = db.update_source_status(source_id, "error", Some(&msg));
            return Err(QueueError::Execution(msg));
        }
        _ => {}
    }

    // Collect extracted frame paths (sorted)
    let temp_dir_read = temp_dir.clone();
    let mut frame_paths: Vec<PathBuf> = tokio::task::spawn_blocking(move || {
        std::fs::read_dir(&temp_dir_read).map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| p.extension().and_then(|e| e.to_str()) == Some("jpg"))
                .collect::<Vec<_>>()
        })
    })
    .await
    .map_err(|e| QueueError::Execution(e.to_string()))?
    .map_err(|e| QueueError::Execution(format!("Failed to read temp dir: {}", e)))?;
    frame_paths.sort();

    if frame_paths.is_empty() {
        let td = temp_dir.clone();
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
        let msg = "ffmpeg extracted 0 frames from video";
        let _ = db.update_source_status(source_id, "error", Some(msg));
        return Err(QueueError::Execution(msg.to_string()));
    }

    tracing::info!(
        source_id,
        frames = frame_paths.len(),
        "Extracted frames, describing with vision model"
    );

    // Describe each frame
    let provider = OllamaProvider::new(ollama_url);
    let mut frame_descriptions = Vec::new();

    for (i, frame_path) in frame_paths.iter().enumerate() {
        if ctx.is_cancelled() {
            let _ = db.update_source_status(source_id, "pending", None);
            let td = temp_dir.clone();
            let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
            return Err(QueueError::Cancelled);
        }

        let timestamp = (i as f64 * frame_interval) as u32;
        let mins = timestamp / 60;
        let secs = timestamp % 60;

        let fp = frame_path.clone();
        let frame_bytes = tokio::task::spawn_blocking(move || std::fs::read(&fp))
            .await
            .map_err(|e| QueueError::Execution(e.to_string()))?
            .map_err(|e| QueueError::Execution(format!("Failed to read frame: {}", e)))?;
        let frame_base64 = BASE64_STANDARD.encode(&frame_bytes);

        let frame_title = format!("{} (frame at {}:{:02})", source_title, mins, secs);
        let frame_future =
            crate::ingestion::vision::describe_image(&frame_base64, &frame_title, &provider, model);
        tokio::pin!(frame_future);
        let frame_result = loop {
            if ctx.is_cancelled() {
                let _ = db.update_source_status(source_id, "pending", None);
                let td = temp_dir.clone();
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
                return Err(QueueError::Cancelled);
            }

            tokio::select! {
                result = &mut frame_future => break result,
                _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
            }
        };

        match frame_result {
            Ok(desc) => {
                frame_descriptions.push(format!("[{:02}:{:02}] {}", mins, secs, desc));
            }
            Err(e) => {
                tracing::warn!(source_id, frame = i, error = %e, "Failed to describe frame, skipping");
                frame_descriptions.push(format!(
                    "[{:02}:{:02}] (frame description failed)",
                    mins, secs
                ));
            }
        }
    }

    if ctx.is_cancelled() {
        let _ = db.update_source_status(source_id, "pending", None);
        let td = temp_dir.clone();
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
        return Err(QueueError::Cancelled);
    }

    // Cleanup temp frames
    let td = temp_dir.clone();
    let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;

    // Combine into full description
    let description = format!(
        "Video: {} (duration: {:.0}s, {} frames analyzed)\n\n{}",
        source_title,
        duration_secs,
        frame_descriptions.len(),
        frame_descriptions.join("\n\n")
    );

    // Store description
    let word_count = description.split_whitespace().count() as i32;
    db.update_source_content(source_id, &description, word_count)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Create chunks
    let chunks = chunk_text_with_title(&description, source_id, source_title);
    for chunk_data in &chunks {
        let chunk = crate::db::notebook_db::Chunk {
            id: chunk_data.id.clone(),
            source_id: source_id.to_string(),
            chunk_index: chunk_data.chunk_index,
            content: chunk_data.content.clone(),
            token_count: chunk_data.token_count,
            start_offset: chunk_data.start_offset,
            end_offset: chunk_data.end_offset,
            metadata: chunk_data.metadata.clone(),
            embedding_id: None,
            embedding_model: None,
        };
        db.insert_chunk(&chunk)
            .map_err(|e| QueueError::Execution(e.to_string()))?;
    }

    db.update_source_status(source_id, "ready", None)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    tracing::info!(
        source_id,
        description_len = description.len(),
        frames = frame_descriptions.len(),
        chunks = chunks.len(),
        "Video described and chunked"
    );

    Ok(JobResult::success_with_output(
        serde_json::json!({
            "notebook_id": notebook_id,
            "source_id": source_id,
            "job_type": "DescribeVideo"
        })
        .to_string(),
    ))
}

/// Get video duration in seconds using ffprobe (async).
async fn get_video_duration(path: &std::path::Path) -> Option<f64> {
    let output = tokio::process::Command::new("ffprobe")
        .args([
            "-v",
            "quiet",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            &path.to_string_lossy(),
        ])
        .output()
        .await
        .ok()?;

    if !output.status.success() {
        return None;
    }

    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .ok()
}
