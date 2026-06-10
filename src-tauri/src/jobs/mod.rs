use crate::db::notebook_db::NotebookDb;
use crate::error::GlossError;
use crate::ingestion::chunk::chunk_text_with_title;
use crate::providers::ollama::OllamaProvider;
use crate::providers::LlmProvider;
use crate::redaction::redact_path;
use crate::tool_invocation::{
    run_tool_output_receipt, run_tool_status_receipt, ToolInvocationReceiptV1,
};
use base64::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
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
        #[serde(default = "default_chunk_target_tokens")]
        chunk_target_tokens: usize,
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
        #[serde(default = "default_chunk_target_tokens")]
        chunk_target_tokens: usize,
    },
    /// Extract bounded audio metadata through ffprobe and cached Whisper transcript when available.
    ExtractAudioMetadata {
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        #[serde(default = "default_chunk_target_tokens")]
        chunk_target_tokens: usize,
    },
}

fn default_chunk_target_tokens() -> usize {
    1100
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
                chunk_target_tokens,
            } => {
                execute_describe_image(
                    ctx,
                    notebook_id,
                    source_id,
                    source_title,
                    data_dir,
                    ollama_url,
                    model,
                    *chunk_target_tokens,
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
                chunk_target_tokens,
            } => {
                execute_describe_video(
                    ctx,
                    notebook_id,
                    source_id,
                    source_title,
                    data_dir,
                    ollama_url,
                    model,
                    *chunk_target_tokens,
                )
                .await
            }
            GlossJob::ExtractAudioMetadata {
                epoch: _,
                notebook_id,
                source_id,
                source_title,
                data_dir,
                chunk_target_tokens,
            } => {
                execute_audio_metadata(
                    ctx,
                    notebook_id,
                    source_id,
                    source_title,
                    data_dir,
                    *chunk_target_tokens,
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
            GlossJob::ExtractAudioMetadata { .. } => "ExtractAudioMetadata",
        }
    }
}

impl GlossJob {
    pub fn notebook_id(&self) -> &str {
        match self {
            GlossJob::SummarizeSource { notebook_id, .. }
            | GlossJob::DescribeImage { notebook_id, .. }
            | GlossJob::DescribeVideo { notebook_id, .. }
            | GlossJob::ExtractAudioMetadata { notebook_id, .. } => notebook_id,
        }
    }

    pub fn source_id(&self) -> &str {
        match self {
            GlossJob::SummarizeSource { source_id, .. }
            | GlossJob::DescribeImage { source_id, .. }
            | GlossJob::DescribeVideo { source_id, .. }
            | GlossJob::ExtractAudioMetadata { source_id, .. } => source_id,
        }
    }

    pub fn epoch(&self) -> u64 {
        match self {
            GlossJob::SummarizeSource { epoch, .. }
            | GlossJob::DescribeImage { epoch, .. }
            | GlossJob::DescribeVideo { epoch, .. }
            | GlossJob::ExtractAudioMetadata { epoch, .. } => *epoch,
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

pub(crate) fn max_pending_epoch_for_notebook(
    queue: &Arc<QueueManager>,
    notebook_id: &str,
) -> Option<u64> {
    match queue.list_jobs_with_data() {
        Ok(jobs) => jobs
            .into_iter()
            .filter(|(_job_id, status, _data_json)| {
                matches!(status.as_str(), "pending" | "processing")
            })
            .filter_map(|(_job_id, _status, data_json)| {
                serde_json::from_str::<GlossJob>(&data_json).ok()
            })
            .filter(|job| job.notebook_id() == notebook_id)
            .map(|job| job.epoch())
            .max(),
        Err(e) => {
            tracing::warn!(error = %e, "Failed to inspect queue jobs for epoch resume");
            None
        }
    }
}

fn is_deleted_source_error(err: &GlossError, source_id: &str) -> bool {
    matches!(err, GlossError::NotFound(message) if message.contains(&format!("Source {source_id} not found")))
}

fn skipped_source_job(
    notebook_id: &str,
    source_id: &str,
    reason: &str,
) -> Result<JobResult, QueueError> {
    tracing::info!(notebook_id, source_id, "{reason}");
    Ok(JobResult::success_with_output(
        serde_json::json!({ "notebook_id": notebook_id, "source_id": source_id, "skipped": true })
            .to_string(),
    ))
}

async fn execute_audio_metadata(
    ctx: &JobContext,
    notebook_id: &str,
    source_id: &str,
    source_title: &str,
    data_dir: &str,
    chunk_target_tokens: usize,
) -> Result<JobResult, QueueError> {
    let nb_dir = PathBuf::from(data_dir).join("notebooks").join(notebook_id);
    let db_path = nb_dir.join("notebook.db");

    if !db_path.exists() {
        tracing::info!(
            notebook_id,
            source_id,
            "Notebook deleted, skipping audio metadata job"
        );
        return Ok(JobResult::success_with_output(
            serde_json::json!({ "notebook_id": notebook_id, "source_id": source_id, "skipped": true }).to_string(),
        ));
    }

    let db = NotebookDb::connect(&db_path).map_err(|e| QueueError::Execution(e.to_string()))?;
    let source = match db.get_source(source_id) {
        Ok(source) => source,
        Err(e) if is_deleted_source_error(&e, source_id) => {
            return skipped_source_job(
                notebook_id,
                source_id,
                "Source deleted, skipping audio metadata job",
            );
        }
        Err(e) => return Err(QueueError::Execution(e.to_string())),
    };

    if source.content_text.is_some() && source.status != "pending" {
        tracing::debug!(source_id, "Audio metadata already extracted, skipping");
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
        QueueError::Execution(format!("Audio source {} has no file_path", source_id))
    })?;
    let sources_dir = nb_dir.join("sources");
    let full_path = crate::redaction::safe_join_under(&sources_dir, file_path)
        .map_err(QueueError::Execution)?;

    db.update_source_status(source_id, "describing", None)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    if ctx.is_cancelled() {
        if let Err(e) = db.update_source_status(source_id, "pending", None) {
            tracing::warn!("failed to update source status to pending: {e}");
        }
        return Err(QueueError::Cancelled);
    }

    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-print_format".to_string(),
        "json".to_string(),
        "-show_format".to_string(),
        "-show_streams".to_string(),
        full_path.to_string_lossy().to_string(),
    ];
    let output = run_tool_output_receipt(
        "ffprobe",
        "audio_metadata_probe",
        &args,
        vec![
            "-v".to_string(),
            "quiet".to_string(),
            "-print_format".to_string(),
            "json".to_string(),
            "-show_format".to_string(),
            "-show_streams".to_string(),
            "[source_audio_path]".to_string(),
        ],
        std::time::Duration::from_secs(10),
    )
    .await
    .map_err(|e| QueueError::Execution(e.to_string()))?;

    let receipt = output.receipt;
    if !receipt.success {
        let msg = if receipt.timed_out {
            "ffprobe timed out while extracting audio metadata".to_string()
        } else {
            "ffprobe failed while extracting audio metadata".to_string()
        };
        if let Err(e) = db.update_source_status(source_id, "error", Some(&msg)) {
            tracing::warn!("failed to update source status to error: {e}");
        }
        return Err(QueueError::Execution(msg));
    }

    let metadata = serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .map_err(|e| QueueError::Execution(format!("ffprobe audio metadata was not JSON: {e}")))?;
    let duration = audio_duration_seconds(&metadata);
    let transcription = maybe_transcribe_audio(source_title, &full_path, &nb_dir, duration).await?;
    let mut tool_receipts = vec![receipt];
    if let Some(receipt) = transcription.tool_receipt.clone() {
        tool_receipts.push(receipt);
    }
    let description = audio_metadata_description(source_title, &metadata, &transcription);
    let word_count = description.split_whitespace().count() as i32;
    db.update_source_content(source_id, &description, word_count)
        .map_err(|e| QueueError::Execution(e.to_string()))?;
    let metadata_json =
        merge_audio_receipt_metadata(source.metadata.as_deref(), &transcription, &tool_receipts)
            .map_err(|e| QueueError::Execution(e.to_string()))?;
    db.update_source_metadata(source_id, Some(&metadata_json))
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    let chunks = chunk_text_with_title(
        &description,
        source_id,
        source_title,
        Some(chunk_target_tokens),
    );
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

    Ok(JobResult::success_with_output(
        serde_json::json!({
            "notebook_id": notebook_id,
            "source_id": source_id,
            "job_type": "ExtractAudioMetadata",
            "audio_transcription_status": transcription.status,
            "tool_invocation_receipts": tool_receipts,
            "needs_finalization": true
        })
        .to_string(),
    ))
}

#[derive(Debug, Clone)]
struct AudioTranscriptionAttempt {
    status: &'static str,
    model: String,
    reason: Option<String>,
    transcript_text: Option<String>,
    segment_count: usize,
    tool_receipt: Option<ToolInvocationReceiptV1>,
}

const DEFAULT_WHISPER_MODEL: &str = "small.en";
const MAX_AUDIO_TRANSCRIPTION_DURATION_SECS: f64 = 30.0 * 60.0;
const AUDIO_TRANSCRIPTION_TIMEOUT_SECS: u64 = 10 * 60;
const MAX_AUDIO_TRANSCRIPT_CHARS: usize = 1_000_000;
const MAX_AUDIO_TRANSCRIPT_SEGMENTS: usize = 20_000;

fn audio_duration_seconds(metadata: &serde_json::Value) -> Option<f64> {
    metadata
        .get("format")
        .and_then(|format| format.get("duration"))
        .and_then(|v| v.as_str())
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn whisper_model_name() -> String {
    std::env::var("GLOSS_WHISPER_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| {
            !value.is_empty()
                && value
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        })
        .unwrap_or_else(|| DEFAULT_WHISPER_MODEL.to_string())
}

fn whisper_model_dir() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("GLOSS_WHISPER_MODEL_DIR") {
        let trimmed = path.trim();
        if !trimmed.is_empty() {
            return Some(PathBuf::from(trimmed));
        }
    }
    std::env::var("HOME")
        .ok()
        .filter(|home| !home.trim().is_empty())
        .map(|home| PathBuf::from(home).join(".cache").join("whisper"))
}

fn cached_whisper_model_path(model: &str, model_dir: &Path) -> PathBuf {
    model_dir.join(format!("{model}.pt"))
}

async fn maybe_transcribe_audio(
    source_title: &str,
    full_path: &Path,
    nb_dir: &Path,
    duration: Option<f64>,
) -> Result<AudioTranscriptionAttempt, QueueError> {
    let model = whisper_model_name();
    let Some(model_dir) = whisper_model_dir() else {
        return Ok(AudioTranscriptionAttempt {
            status: "unavailable",
            model,
            reason: Some(
                "no HOME or GLOSS_WHISPER_MODEL_DIR for cached Whisper model lookup".to_string(),
            ),
            transcript_text: None,
            segment_count: 0,
            tool_receipt: None,
        });
    };
    let model_path = cached_whisper_model_path(&model, &model_dir);
    if !model_path.exists() {
        return Ok(AudioTranscriptionAttempt {
            status: "unavailable",
            model,
            reason: Some(format!(
                "cached Whisper model is missing; expected {}",
                crate::redaction::redact_path(&model_path)
            )),
            transcript_text: None,
            segment_count: 0,
            tool_receipt: None,
        });
    }
    if duration.is_some_and(|value| value > MAX_AUDIO_TRANSCRIPTION_DURATION_SECS) {
        return Ok(AudioTranscriptionAttempt {
            status: "skipped_duration_limit",
            model,
            reason: Some(format!(
                "audio duration exceeds transcription limit of {} seconds",
                MAX_AUDIO_TRANSCRIPTION_DURATION_SECS as u64
            )),
            transcript_text: None,
            segment_count: 0,
            tool_receipt: None,
        });
    }

    let output_dir = nb_dir
        .join("tmp")
        .join("audio_transcripts")
        .join(uuid::Uuid::new_v4().to_string());
    std::fs::create_dir_all(&output_dir)
        .map_err(|e| QueueError::Execution(format!("failed to create transcript temp dir: {e}")))?;
    let args = vec![
        full_path.to_string_lossy().to_string(),
        "--model".to_string(),
        model.clone(),
        "--model_dir".to_string(),
        model_dir.to_string_lossy().to_string(),
        "--device".to_string(),
        "cpu".to_string(),
        "--output_dir".to_string(),
        output_dir.to_string_lossy().to_string(),
        "--output_format".to_string(),
        "json".to_string(),
        "--verbose".to_string(),
        "False".to_string(),
        "--task".to_string(),
        "transcribe".to_string(),
        "--fp16".to_string(),
        "False".to_string(),
    ];
    let redacted_args = vec![
        "[source_audio_path]".to_string(),
        "--model".to_string(),
        model.clone(),
        "--model_dir".to_string(),
        "[whisper_model_dir]".to_string(),
        "--device".to_string(),
        "cpu".to_string(),
        "--output_dir".to_string(),
        "[transcript_output_dir]".to_string(),
        "--output_format".to_string(),
        "json".to_string(),
        "--verbose".to_string(),
        "False".to_string(),
        "--task".to_string(),
        "transcribe".to_string(),
        "--fp16".to_string(),
        "False".to_string(),
    ];
    let output = run_tool_output_receipt(
        "whisper",
        "audio_transcription_whisper",
        &args,
        redacted_args,
        std::time::Duration::from_secs(AUDIO_TRANSCRIPTION_TIMEOUT_SECS),
    )
    .await
    .map_err(|e| QueueError::Execution(e.to_string()))?;
    let receipt = output.receipt;
    if !receipt.success {
        let _ = std::fs::remove_dir_all(&output_dir);
        return Ok(AudioTranscriptionAttempt {
            status: if receipt.timed_out {
                "timeout"
            } else {
                "failed"
            },
            model,
            reason: Some("whisper CLI did not produce a successful transcript".to_string()),
            transcript_text: None,
            segment_count: 0,
            tool_receipt: Some(receipt),
        });
    }

    let transcript_path = whisper_json_output_path(&output_dir, full_path).ok_or_else(|| {
        QueueError::Execution("whisper CLI succeeded but no transcript JSON was found".to_string())
    })?;
    let raw = std::fs::read_to_string(&transcript_path).map_err(|e| {
        QueueError::Execution(format!("failed to read whisper transcript JSON: {e}"))
    })?;
    let (transcript_text, segment_count) = whisper_transcript_text(source_title, &model, &raw)
        .map_err(|e| QueueError::Execution(e.to_string()))?;
    let _ = std::fs::remove_dir_all(&output_dir);
    Ok(AudioTranscriptionAttempt {
        status: "transcribed",
        model,
        reason: None,
        transcript_text: Some(transcript_text),
        segment_count,
        tool_receipt: Some(receipt),
    })
}

fn whisper_json_output_path(output_dir: &Path, source_path: &Path) -> Option<PathBuf> {
    let expected = source_path
        .file_stem()
        .and_then(|stem| stem.to_str())
        .map(|stem| output_dir.join(format!("{stem}.json")))?;
    if expected.exists() {
        return Some(expected);
    }
    std::fs::read_dir(output_dir)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .find(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
}

fn whisper_transcript_text(
    source_title: &str,
    model: &str,
    raw_json: &str,
) -> Result<(String, usize), GlossError> {
    let value = serde_json::from_str::<serde_json::Value>(raw_json)
        .map_err(|e| GlossError::Other(format!("failed to parse whisper transcript JSON: {e}")))?;
    let segments = value
        .get("segments")
        .and_then(|value| value.as_array())
        .ok_or_else(|| {
            GlossError::Other("whisper transcript JSON has no segments array".to_string())
        })?;
    if segments.len() > MAX_AUDIO_TRANSCRIPT_SEGMENTS {
        return Err(GlossError::Other(
            "whisper transcript exceeded segment limit".to_string(),
        ));
    }
    let mut out = format!("Audio transcript: {source_title}\nTranscription model: {model}\n\n");
    let mut count = 0usize;
    for segment in segments {
        let text = segment
            .get("text")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim();
        if text.is_empty() {
            continue;
        }
        let start_ms = segment
            .get("start")
            .and_then(|value| value.as_f64())
            .filter(|value| value.is_finite() && *value >= 0.0)
            .map(|value| (value * 1000.0) as u64)
            .unwrap_or(0);
        out.push_str(&format!("[{}] {}\n", format_timestamp_ms(start_ms), text));
        count += 1;
        if out.len() > MAX_AUDIO_TRANSCRIPT_CHARS {
            return Err(GlossError::Other(
                "whisper transcript exceeded text output limit".to_string(),
            ));
        }
    }
    if count == 0 {
        return Err(GlossError::Other(
            "whisper transcript contained no readable text".to_string(),
        ));
    }
    Ok((out, count))
}

fn format_timestamp_ms(ms: u64) -> String {
    let total_seconds = ms / 1000;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

fn merge_audio_receipt_metadata(
    existing: Option<&str>,
    transcription: &AudioTranscriptionAttempt,
    receipts: &[ToolInvocationReceiptV1],
) -> Result<String, GlossError> {
    let mut metadata = existing
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .filter(|value| value.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    metadata["audio_processing"] = serde_json::json!({
        "schema": "AudioProcessingMetadataV1",
        "transcription_status": transcription.status,
        "transcription_model": transcription.model,
        "transcription_segment_count": transcription.segment_count,
        "transcription_reason": transcription.reason,
        "tool_invocation_receipts": receipts,
    });
    serde_json::to_string(&metadata).map_err(|e| GlossError::Other(e.to_string()))
}

fn audio_metadata_description(
    source_title: &str,
    metadata: &serde_json::Value,
    transcription: &AudioTranscriptionAttempt,
) -> String {
    let format = metadata.get("format").unwrap_or(&serde_json::Value::Null);
    let duration = format
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|value| value.parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| format!("{value:.2}s"))
        .unwrap_or_else(|| "unknown".to_string());
    let format_name = format
        .get("format_name")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown");
    let bit_rate = format
        .get("bit_rate")
        .and_then(|v| v.as_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("unknown");
    let streams = metadata
        .get("streams")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let audio_streams = streams
        .iter()
        .filter(|stream| stream.get("codec_type").and_then(|v| v.as_str()) == Some("audio"))
        .collect::<Vec<_>>();
    let stream_lines = audio_streams
        .iter()
        .enumerate()
        .map(|(index, stream)| {
            let codec = stream
                .get("codec_name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let channels = stream
                .get("channels")
                .and_then(|v| v.as_i64())
                .map(|value| value.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let sample_rate = stream
                .get("sample_rate")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            format!(
                "Stream {}: codec={}, channels={}, sample_rate={}",
                index + 1,
                codec,
                channels,
                sample_rate
            )
        })
        .collect::<Vec<_>>();
    let stream_text = if stream_lines.is_empty() {
        "No audio stream metadata found.".to_string()
    } else {
        stream_lines.join("\n")
    };

    let transcription_text = match transcription.status {
        "transcribed" => transcription
            .transcript_text
            .as_deref()
            .unwrap_or("Audio transcript unavailable."),
        status => {
            let reason = transcription.reason.as_deref().unwrap_or("not available");
            return format!(
                "Audio: {source_title}\nDuration: {duration}\nFormat: {format_name}\nBit rate: {bit_rate}\nTranscription: {status} ({reason}).\n\n{stream_text}"
            );
        }
    };

    format!(
        "Audio: {source_title}\nDuration: {duration}\nFormat: {format_name}\nBit rate: {bit_rate}\nTranscription: transcribed with cached Whisper model {}.\n\n{stream_text}\n\n{transcription_text}",
        transcription.model
    )
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
    let source = match db.get_source(source_id) {
        Ok(source) => source,
        Err(e) if is_deleted_source_error(&e, source_id) => {
            return skipped_source_job(
                notebook_id,
                source_id,
                "Source deleted, skipping summary job",
            );
        }
        Err(e) => return Err(QueueError::Execution(e.to_string())),
    };

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
    let (summary, call_receipt) = loop {
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

    // Build and persist batch receipt for the summary LLM call
    {
        let mut batch_receipt = crate::commands::chat::receipts::BatchReceiptV1::new(
            "summary_generation",
            Some(notebook_id),
            Some(source_id),
        );
        batch_receipt.record_call(
            0,
            &call_receipt.call_purpose,
            &call_receipt.model,
            &call_receipt.provider,
            None, // prompt_tokens not available from stream
            None, // completion_tokens not available from stream
            std::time::Duration::from_millis(call_receipt.duration_ms as u64),
            call_receipt.success,
            call_receipt.error_message.as_deref(),
        );
        batch_receipt.finalize();
        if let Err(err) = crate::commands::chat::receipts::persist_batch_receipt(
            PathBuf::from(data_dir).as_path(),
            &batch_receipt,
        ) {
            tracing::warn!(error = %err, "Failed to persist summary batch receipt");
        }
    }

    if ctx.is_cancelled() {
        return Err(QueueError::Cancelled);
    }

    // Store the summary
    match db.update_source_summary(source_id, &summary, model) {
        Ok(()) => {}
        Err(e) if is_deleted_source_error(&e, source_id) => {
            return skipped_source_job(
                notebook_id,
                source_id,
                "Source deleted before summary save",
            );
        }
        Err(e) => return Err(QueueError::Execution(e.to_string())),
    }

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
    chunk_target_tokens: usize,
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
    let source = match db.get_source(source_id) {
        Ok(source) => source,
        Err(e) if is_deleted_source_error(&e, source_id) => {
            return skipped_source_job(
                notebook_id,
                source_id,
                "Source deleted, skipping describe job",
            );
        }
        Err(e) => return Err(QueueError::Execution(e.to_string())),
    };

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
    let sources_dir = nb_dir.join("sources");
    let full_path = crate::redaction::safe_join_under(&sources_dir, file_path)
        .map_err(QueueError::Execution)?;
    let full_path_clone = full_path.clone();
    let image_bytes = tokio::task::spawn_blocking(move || std::fs::read(&full_path_clone))
        .await
        .map_err(|e| QueueError::Execution(e.to_string()))?
        .map_err(|e| {
            QueueError::Execution(format!(
                "Failed to read image {}: {}",
                redact_path(&full_path),
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
    let (description, vision_call_receipt) = loop {
        if ctx.is_cancelled() {
            if let Err(e) = db.update_source_status(source_id, "pending", None) {
                tracing::warn!("failed to update source status to pending: {e}");
            }
            return Err(QueueError::Cancelled);
        }

        tokio::select! {
            result = &mut description_future => {
                break result.map_err(|e| {
                    // Reset status on failure
                    if let Err(db_err) = db.update_source_status(source_id, "error", Some(&e.to_string())) {
                        tracing::warn!("failed to update source status to error: {db_err}");
                    }
                    QueueError::Execution(format!("Vision description failed: {}", e))
                })?;
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(250)) => {}
        }
    };

    // Build and persist batch receipt for the vision LLM call
    {
        let mut batch_receipt = crate::commands::chat::receipts::BatchReceiptV1::new(
            "image_description",
            Some(notebook_id),
            Some(source_id),
        );
        batch_receipt.record_call(
            0,
            &vision_call_receipt.call_purpose,
            &vision_call_receipt.model,
            &vision_call_receipt.provider,
            None, // prompt_tokens not available from stream
            None, // completion_tokens not available from stream
            std::time::Duration::from_millis(vision_call_receipt.duration_ms as u64),
            vision_call_receipt.success,
            vision_call_receipt.error_message.as_deref(),
        );
        batch_receipt.finalize();
        let receipt_data_dir = PathBuf::from(data_dir);
        if let Err(err) = crate::commands::chat::receipts::persist_batch_receipt(
            receipt_data_dir.as_path(),
            &batch_receipt,
        ) {
            tracing::warn!(error = %err, "Failed to persist vision batch receipt");
        }
    }

    if ctx.is_cancelled() {
        if let Err(e) = db.update_source_status(source_id, "pending", None) {
            tracing::warn!("failed to update source status to pending: {e}");
        }
        return Err(QueueError::Cancelled);
    }

    // Store description as content_text
    let word_count = description.split_whitespace().count() as i32;
    db.update_source_content(source_id, &description, word_count)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Create chunks from the description
    let chunks = chunk_text_with_title(
        &description,
        source_id,
        source_title,
        Some(chunk_target_tokens),
    );
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
            "job_type": "DescribeImage",
            "needs_finalization": true
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
    chunk_target_tokens: usize,
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
    let source = match db.get_source(source_id) {
        Ok(source) => source,
        Err(e) if is_deleted_source_error(&e, source_id) => {
            return skipped_source_job(
                notebook_id,
                source_id,
                "Source deleted, skipping video job",
            );
        }
        Err(e) => return Err(QueueError::Execution(e.to_string())),
    };

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
    let sources_dir = nb_dir.join("sources");
    let full_path = crate::redaction::safe_join_under(&sources_dir, file_path)
        .map_err(QueueError::Execution)?;

    let mut tool_receipts: Vec<ToolInvocationReceiptV1> = Vec::new();
    let ffmpeg_probe_receipt = run_tool_status_receipt(
        "ffmpeg",
        "video_frame_analysis_availability_probe",
        &["-version"],
        std::time::Duration::from_secs(5),
    )
    .await
    .map_err(|e| QueueError::Execution(e.to_string()))?;
    let ffmpeg_ok = ffmpeg_probe_receipt.success;
    tool_receipts.push(ffmpeg_probe_receipt);
    if !ffmpeg_ok {
        let msg = "ffmpeg not found — install ffmpeg to enable video frame analysis";
        if let Err(e) = db.update_source_status(source_id, "error", Some(msg)) {
            tracing::warn!("failed to update source status to error: {e}");
        }
        return Err(QueueError::Execution(msg.to_string()));
    }

    db.update_source_status(source_id, "describing", None)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    if ctx.is_cancelled() {
        if let Err(e) = db.update_source_status(source_id, "pending", None) {
            tracing::warn!("failed to update source status to pending: {e}");
        }
        return Err(QueueError::Cancelled);
    }

    // Get video duration with ffprobe
    let (duration_secs, ffprobe_receipt) = get_video_duration(&full_path).await;
    tool_receipts.push(ffprobe_receipt);
    let frame_interval = duration_secs
        .map(|duration_secs| (duration_secs / MAX_VIDEO_FRAMES as f64).max(1.0))
        .unwrap_or(10.0);

    tracing::info!(
        source_id,
        source_title,
        model,
        duration_secs = ?duration_secs,
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
    let ffmpeg_args = vec![
        "-i".to_string(),
        full_path.to_string_lossy().to_string(),
        "-vf".to_string(),
        format!("fps=1/{}", frame_interval as u32),
        "-frames:v".to_string(),
        MAX_VIDEO_FRAMES.to_string(),
        "-q:v".to_string(),
        "2".to_string(),
        frame_pattern.to_string_lossy().to_string(),
    ];
    let ffmpeg_receipt = run_tool_output_receipt(
        "ffmpeg",
        "video_frame_extract",
        &ffmpeg_args,
        vec![
            "-i".to_string(),
            "[source_video_path]".to_string(),
            "-vf".to_string(),
            format!("fps=1/{}", frame_interval as u32),
            "-frames:v".to_string(),
            MAX_VIDEO_FRAMES.to_string(),
            "-q:v".to_string(),
            "2".to_string(),
            "[frame_output_pattern]".to_string(),
        ],
        std::time::Duration::from_secs(120),
    )
    .await
    .map_err(|e| QueueError::Execution(e.to_string()))?
    .receipt;

    if !ffmpeg_receipt.success {
        let timed_out = ffmpeg_receipt.timed_out;
        let exit_code = ffmpeg_receipt.exit_code;
        tool_receipts.push(ffmpeg_receipt);
        match (timed_out, exit_code) {
            (false, Some(code)) => {
                let td = temp_dir.clone();
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
                let msg = format!("ffmpeg exited with status {code}");
                if let Err(e) = db.update_source_status(source_id, "error", Some(&msg)) {
                    tracing::warn!("failed to update source status to error: {e}");
                }
                return Err(QueueError::Execution(msg));
            }
            (false, None) => {
                let td = temp_dir.clone();
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
                let msg = "Failed to run ffmpeg";
                if let Err(e) = db.update_source_status(source_id, "error", Some(msg)) {
                    tracing::warn!("failed to update source status to error: {e}");
                }
                return Err(QueueError::Execution(msg.to_string()));
            }
            (true, _) => {
                let td = temp_dir.clone();
                let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
                let msg = "ffmpeg timed out while extracting video frames";
                if let Err(e) = db.update_source_status(source_id, "error", Some(msg)) {
                    tracing::warn!("failed to update source status to error: {e}");
                }
                return Err(QueueError::Execution(msg.to_string()));
            }
        }
    } else {
        tool_receipts.push(ffmpeg_receipt);
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
        if let Err(e) = db.update_source_status(source_id, "error", Some(msg)) {
            tracing::warn!("failed to update source status to error: {e}");
        }
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
    let mut video_batch_receipt = crate::commands::chat::receipts::BatchReceiptV1::new(
        "video_frame_description",
        Some(notebook_id),
        Some(source_id),
    );

    for (i, frame_path) in frame_paths.iter().enumerate() {
        if ctx.is_cancelled() {
            if let Err(e) = db.update_source_status(source_id, "pending", None) {
                tracing::warn!("failed to update source status to pending: {e}");
            }
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
                if let Err(e) = db.update_source_status(source_id, "pending", None) {
                    tracing::warn!("failed to update source status to pending: {e}");
                }
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
            Ok((desc, call_receipt)) => {
                video_batch_receipt.record_call(
                    i,
                    &call_receipt.call_purpose,
                    &call_receipt.model,
                    &call_receipt.provider,
                    None,
                    None,
                    std::time::Duration::from_millis(call_receipt.duration_ms as u64),
                    true,
                    None,
                );
                frame_descriptions.push(format!("[{:02}:{:02}] {}", mins, secs, desc));
            }
            Err(e) => {
                tracing::warn!(source_id, frame = i, error = %e, "Failed to describe frame, skipping");
                video_batch_receipt.record_call(
                    i,
                    "describe_image",
                    model,
                    provider.provider_type().as_str(),
                    None,
                    None,
                    std::time::Duration::from_millis(0),
                    false,
                    Some(&e.to_string()),
                );
                frame_descriptions.push(format!(
                    "[{:02}:{:02}] (frame description failed)",
                    mins, secs
                ));
            }
        }
    }

    // Finalize and persist the video batch receipt
    video_batch_receipt.finalize();
    let video_receipt_data_dir = PathBuf::from(data_dir);
    if let Err(err) = crate::commands::chat::receipts::persist_batch_receipt(
        video_receipt_data_dir.as_path(),
        &video_batch_receipt,
    ) {
        tracing::warn!(error = %err, "Failed to persist video frame batch receipt");
    }

    if ctx.is_cancelled() {
        if let Err(e) = db.update_source_status(source_id, "pending", None) {
            tracing::warn!("failed to update source status to pending: {e}");
        }
        let td = temp_dir.clone();
        let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;
        return Err(QueueError::Cancelled);
    }

    // Cleanup temp frames
    let td = temp_dir.clone();
    let _ = tokio::task::spawn_blocking(move || std::fs::remove_dir_all(&td)).await;

    // Combine into full description
    let duration_label = duration_secs
        .map(|duration_secs| format!("duration: {:.0}s", duration_secs))
        .unwrap_or_else(|| "duration: unknown".to_string());
    let description = format!(
        "Video: {} ({}, {} frames analyzed)\n\n{}",
        source_title,
        duration_label,
        frame_descriptions.len(),
        frame_descriptions.join("\n\n")
    );

    // Store description
    let word_count = description.split_whitespace().count() as i32;
    db.update_source_content(source_id, &description, word_count)
        .map_err(|e| QueueError::Execution(e.to_string()))?;

    // Create chunks
    let chunks = chunk_text_with_title(
        &description,
        source_id,
        source_title,
        Some(chunk_target_tokens),
    );
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
            "job_type": "DescribeVideo",
            "tool_invocation_receipts": tool_receipts,
            "needs_finalization": true
        })
        .to_string(),
    ))
}

/// Get video duration in seconds using ffprobe (async).
async fn get_video_duration(path: &std::path::Path) -> (Option<f64>, ToolInvocationReceiptV1) {
    let args = vec![
        "-v".to_string(),
        "quiet".to_string(),
        "-show_entries".to_string(),
        "format=duration".to_string(),
        "-of".to_string(),
        "default=noprint_wrappers=1:nokey=1".to_string(),
        path.to_string_lossy().to_string(),
    ];
    let result = run_tool_output_receipt(
        "ffprobe",
        "video_duration_probe",
        &args,
        vec![
            "-v".to_string(),
            "quiet".to_string(),
            "-show_entries".to_string(),
            "format=duration".to_string(),
            "-of".to_string(),
            "default=noprint_wrappers=1:nokey=1".to_string(),
            "[source_video_path]".to_string(),
        ],
        std::time::Duration::from_secs(5),
    )
    .await
    .unwrap_or_else(|err| crate::tool_invocation::ToolInvocationOutput {
        receipt: ToolInvocationReceiptV1 {
            schema: "ToolInvocationReceiptV1".to_string(),
            receipt_id: uuid::Uuid::new_v4().to_string(),
            tool: "ffprobe".to_string(),
            action: "video_duration_probe".to_string(),
            args_redacted: vec!["[receipt-construction-error]".to_string()],
            timeout_ms: 5000,
            elapsed_ms: 0,
            exit_code: None,
            success: false,
            timed_out: false,
            stderr_sha256: Some(format!("{:x}", Sha256::digest(err.to_string().as_bytes()))),
            stderr_len: err.to_string().len(),
            stderr_preview: Some("[receipt-construction-error]".to_string()),
            stdout_sha256: None,
            stdout_len: 0,
        },
        stdout: Vec::new(),
    });

    let duration = if result.receipt.success {
        String::from_utf8_lossy(&result.stdout)
            .trim()
            .parse::<f64>()
            .ok()
    } else {
        None
    }
    .filter(|value| value.is_finite() && *value >= 0.0);
    (duration, result.receipt)
}

#[cfg(test)]
mod tests {
    use super::{
        audio_metadata_description, whisper_model_name, whisper_transcript_text,
        AudioTranscriptionAttempt,
    };

    #[test]
    fn audio_metadata_description_is_searchable_and_discloses_transcription_status() {
        let metadata = serde_json::json!({
            "format": {
                "duration": "12.3456",
                "format_name": "mp3",
                "bit_rate": "128000"
            },
            "streams": [
                {
                    "codec_type": "audio",
                    "codec_name": "mp3",
                    "channels": 2,
                    "sample_rate": "44100"
                }
            ]
        });
        let attempt = AudioTranscriptionAttempt {
            status: "unavailable",
            model: "small.en".to_string(),
            reason: Some("cached model missing".to_string()),
            transcript_text: None,
            segment_count: 0,
            tool_receipt: None,
        };
        let description = audio_metadata_description("fixture.mp3", &metadata, &attempt);
        assert!(description.contains("Audio: fixture.mp3"));
        assert!(description.contains("Duration: 12.35s"));
        assert!(description.contains("codec=mp3"));
        assert!(description.contains("Transcription: unavailable"));
    }

    #[test]
    fn whisper_transcript_json_formats_timestamped_text() {
        let raw = r#"{
          "segments": [
            {"start": 1.25, "end": 2.5, "text": " Hello audio "},
            {"start": 65.0, "end": 66.0, "text": "Second line"}
          ],
          "text": "Hello audio Second line"
        }"#;
        let (text, count) = whisper_transcript_text("fixture.wav", "small.en", raw).unwrap();
        assert_eq!(count, 2);
        assert!(text.contains("Audio transcript: fixture.wav"));
        assert!(text.contains("[00:01] Hello audio"));
        assert!(text.contains("[01:05] Second line"));
    }

    #[test]
    fn whisper_model_name_rejects_path_like_values() {
        std::env::set_var("GLOSS_WHISPER_MODEL", "../secret");
        assert_eq!(whisper_model_name(), "small.en");
        std::env::remove_var("GLOSS_WHISPER_MODEL");
    }
}
