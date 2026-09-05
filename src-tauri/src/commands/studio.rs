use crate::db::notebook_db::{Chunk, Source, StudioOutput};
use crate::error::GlossError;
use crate::providers::{
    build_provider, ChatMessage, ChatRequest, LlmExecutionContext, LlmPhaseTimeouts,
};
use crate::redaction::redact_path;
use crate::state::{ActiveStudioAttempt, AppState};
use crate::studio::{
    build_snippets, generate_artifact, validate_artifact, StudioCitation, StudioOutputKind,
};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Instant;
use tauri::State;
use tokio_util::sync::CancellationToken;

const DEFAULT_STUDIO_MAX_ITEMS: usize = 8;
const MAX_STUDIO_ITEMS: usize = 20;
/// Max chars of source text to feed the LLM per source (keeps context window manageable).
const MAX_SOURCE_CHARS_FOR_LLM: usize = 4000;
const STUDIO_PROVIDER_START_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const STUDIO_FIRST_TOKEN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);
const STUDIO_STREAM_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum StudioFallbackReason {
    ProviderStartTimeout,
    FirstTokenTimeout,
    StreamIdleTimeout,
    ProviderError,
    InvalidStructuredOutput,
}

impl StudioFallbackReason {
    fn as_str(self) -> &'static str {
        match self {
            Self::ProviderStartTimeout => "provider_start_timeout",
            Self::FirstTokenTimeout => "first_token_timeout",
            Self::StreamIdleTimeout => "stream_idle_timeout",
            Self::ProviderError => "provider_error",
            Self::InvalidStructuredOutput => "invalid_structured_output",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StudioTerminalFailureReason {
    Cancelled,
}

#[derive(Debug, Clone)]
struct StudioGenerationFailure {
    reason: StudioGenerationFailureReason,
    message: String,
    elapsed_ms: u128,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StudioGenerationFailureReason {
    Fallback(StudioFallbackReason),
    Terminal(StudioTerminalFailureReason),
}

impl StudioGenerationFailure {
    fn fallback(reason: StudioFallbackReason, message: String, elapsed_ms: u128) -> Self {
        Self {
            reason: StudioGenerationFailureReason::Fallback(reason),
            message,
            elapsed_ms,
        }
    }

    fn terminal(reason: StudioTerminalFailureReason, message: String, elapsed_ms: u128) -> Self {
        Self {
            reason: StudioGenerationFailureReason::Terminal(reason),
            message,
            elapsed_ms,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StudioFallbackReceipt {
    schema: &'static str,
    receipt_id: String,
    attempt_id: String,
    reason: StudioFallbackReason,
    reason_code: &'static str,
    provider_cancelled: bool,
    elapsed_ms: u128,
    detail: String,
    recorded_utc: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StudioProviderRuntimeReceipt {
    schema: &'static str,
    attempt_id: String,
    purpose: String,
    phase: &'static str,
    provider: String,
    model: String,
    elapsed_ms: u128,
    provider_cancelled: bool,
    recorded_utc: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StudioSkippedSource {
    source_id: String,
    title: String,
    status: String,
    reason: &'static str,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct StudioSourceReadiness {
    schema: &'static str,
    requested_source_ids: Vec<String>,
    effective_source_ids: Vec<String>,
    ready_source_count: usize,
    skipped_source_count: usize,
    skipped_sources: Vec<StudioSkippedSource>,
}

struct StudioAttemptGuard<'a> {
    state: &'a AppState,
    notebook_id: String,
    attempt_id: String,
    cancellation: CancellationToken,
}

impl Drop for StudioAttemptGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut active) = self.state.active_studio_attempts.lock() {
            let should_clear = active
                .get(&self.notebook_id)
                .is_some_and(|attempt| attempt.attempt_id == self.attempt_id);
            if should_clear {
                active.remove(&self.notebook_id);
            }
        }
    }
}

impl StudioAttemptGuard<'_> {
    fn cancellation(&self) -> CancellationToken {
        self.cancellation.clone()
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct StudioOutputView {
    pub id: String,
    pub output_type: String,
    pub title: Option<String>,
    pub prompt_used: String,
    pub raw_content: Option<String>,
    pub prose_content: Option<String>,
    pub config: Option<serde_json::Value>,
    pub source_ids: Vec<String>,
    pub file_path: Option<String>,
    pub status: String,
    pub error_message: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct StudioExportReceipt {
    pub schema: &'static str,
    pub receipt_id: String,
    pub output_id: String,
    pub output_type: String,
    pub notebook_id: String,
    pub format: &'static str,
    pub file_path: String,
    pub file_path_redacted: String,
    pub bytes_written: u64,
    pub sha256: String,
    pub recorded_utc: String,
}

#[tauri::command]
pub async fn list_studio_outputs(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<StudioOutputView>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| {
        db.list_studio_outputs()?
            .into_iter()
            .map(studio_output_view)
            .collect()
    })
}

#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn generate_studio_output(
    notebook_id: String,
    output_type: String,
    source_ids: Option<Vec<String>>,
    title: Option<String>,
    max_items: Option<usize>,
    refine: Option<bool>,
    attempt_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<StudioOutputView, GlossError> {
    let kind = StudioOutputKind::parse(&output_type)?;
    let attempt_id = normalize_studio_attempt_id(attempt_id);
    let attempt_guard = register_studio_attempt(&state, &notebook_id, &attempt_id)?;
    let max_items = max_items
        .unwrap_or(DEFAULT_STUDIO_MAX_ITEMS)
        .clamp(1, MAX_STUDIO_ITEMS);
    let should_refine = refine.unwrap_or(true);

    // Phase 1 (read): build the deterministic artifact and collect full source
    // texts. No write lock is held during the (potentially long) LLM phase.
    let (mut artifact, full_texts, citation_pool, source_readiness) =
        state.with_notebook_db(&notebook_id, |db| {
            let mut sources = db.list_sources()?;
            let mut chunks_by_source = Vec::new();
            for source in &mut sources {
                let chunks = db.get_chunks_for_source(&source.id)?;
                if chunks.is_empty() {
                    *source = db.get_source(&source.id)?;
                }
                chunks_by_source.push((source.id.clone(), chunks));
            }

            let requested = source_ids.as_deref();
            let (scope, snippets) =
                build_snippets(&sources, &chunks_by_source, requested, max_items, max_items)?;
            let source_readiness = studio_source_readiness(
                &sources,
                &chunks_by_source,
                requested,
                &scope.effective_source_ids,
            );
            let citation_pool = snippets
                .iter()
                .map(|snippet| snippet.citation())
                .collect::<Vec<_>>();
            let artifact = generate_artifact(kind, title, scope, &snippets)?;

            // list_sources() excludes content_text (heavy field), so we must
            // call get_source() for each effective source to get the text.
            let mut full_texts: Vec<(String, String)> = Vec::new();
            for sid in &artifact.source_scope.effective_source_ids {
                if let Ok(full_source) = db.get_source(sid) {
                    if let Some(text) = full_source.content_text.as_deref() {
                        if !text.trim().is_empty() {
                            let truncated =
                                truncate_to_char_boundary(text, MAX_SOURCE_CHARS_FOR_LLM);
                            full_texts.push((full_source.title, truncated));
                        }
                    }
                }
            }
            Ok((artifact, full_texts, citation_pool, source_readiness))
        })?;

    // Phase 2 (no DB lock): LLM generation. Widget kinds (flashcards, quiz,
    // mind map) get structured JSON content the interactive widgets can
    // render; prose kinds get refined markdown. Both fall back to the
    // deterministic template artifact on failure.
    let mut fallback_receipt: Option<StudioFallbackReceipt> = None;
    let mut provider_runtime_receipt: Option<StudioProviderRuntimeReceipt> = None;
    let (prose_content, prompt_used) = if should_refine && !full_texts.is_empty() {
        if is_widget_kind(kind) {
            match generate_structured_widget_content(
                kind,
                &artifact,
                &full_texts,
                &citation_pool,
                max_items,
                &state,
                attempt_guard.cancellation(),
                &attempt_id,
            )
            .await
            {
                Ok((content, receipt)) => {
                    provider_runtime_receipt = Some(receipt);
                    artifact.content = content;
                    artifact.prompt_used = "llm_structured_source_grounded_v1".to_string();
                    artifact.validation = validate_artifact(&artifact);
                    if !artifact.validation.schema_validated {
                        tracing::warn!(
                            errors = ?artifact.validation.errors,
                            "Structured Studio content failed validation; this should not happen \
                             after citation injection"
                        );
                    }
                    (None, artifact.prompt_used.clone())
                }
                Err(failure) => handle_studio_generation_failure(
                    failure,
                    &attempt_id,
                    &artifact.prompt_used,
                    &mut fallback_receipt,
                )?,
            }
        } else {
            match refine_studio_artifact(
                &artifact,
                &full_texts,
                &state,
                attempt_guard.cancellation(),
                &attempt_id,
            )
            .await
            {
                Ok((prose, receipt)) => {
                    provider_runtime_receipt = Some(receipt);
                    (Some(prose), "source_grounded_refined_v1".to_string())
                }
                Err(failure) => handle_studio_generation_failure(
                    failure,
                    &attempt_id,
                    &artifact.prompt_used,
                    &mut fallback_receipt,
                )?,
            }
        }
    } else {
        (None, artifact.prompt_used.clone())
    };

    if attempt_guard.cancellation().is_cancelled() {
        return Err(GlossError::Studio {
            output_type: output_type.clone(),
            message: format!("Studio generation cancelled: attempt_id={attempt_id}"),
        });
    }

    let raw_content = serde_json::to_string_pretty(&artifact)?;
    let config = serde_json::to_string(&json!({
        "schema": "StudioOutputConfigV1",
        "attempt_id": attempt_id,
        "deterministic": artifact.validation.deterministic,
        "source_bound": true,
        "schema_validated": artifact.validation.schema_validated,
        "all_items_source_cited": artifact.validation.all_items_source_cited,
        "max_items": max_items,
        "receipt_id": artifact.receipt_id,
        "source_readiness": source_readiness,
        "provider_runtime_receipt": provider_runtime_receipt,
        "fallback_receipt": fallback_receipt,
    }))?;
    let source_ids_json = serde_json::to_string(&artifact.source_scope.effective_source_ids)?;
    let now = chrono::Utc::now().to_rfc3339();

    let output = StudioOutput {
        id: artifact.receipt_id.clone(),
        output_type: artifact.output_type.clone(),
        title: Some(artifact.title.clone()),
        prompt_used,
        raw_content: Some(raw_content),
        prose_content,
        config: Some(config),
        source_ids: Some(source_ids_json),
        file_path: None,
        status: "ready".to_string(),
        error_message: None,
        created_at: now,
    };

    // Phase 3 (short write): persist the finished output.
    state.with_notebook_db_write(&notebook_id, |db| db.insert_studio_output(&output))?;
    studio_output_view(output)
}

#[tauri::command]
pub async fn cancel_studio_generation(
    notebook_id: String,
    attempt_id: Option<String>,
    state: State<'_, AppState>,
) -> Result<bool, GlossError> {
    let active = {
        let active = state
            .active_studio_attempts
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        active.get(&notebook_id).cloned()
    };
    let Some(active) = active else {
        return Ok(false);
    };
    if let Some(expected) = attempt_id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
    {
        if active.attempt_id != expected {
            return Ok(false);
        }
    }
    active.cancellation.cancel();
    tracing::info!(
        notebook_id = %active.notebook_id,
        attempt_id = %active.attempt_id,
        "Studio generation cancellation requested"
    );
    Ok(true)
}

fn is_widget_kind(kind: StudioOutputKind) -> bool {
    matches!(
        kind,
        StudioOutputKind::Flashcards | StudioOutputKind::Quiz | StudioOutputKind::MindMap
    )
}

fn normalize_studio_attempt_id(attempt_id: Option<String>) -> String {
    attempt_id
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
        .unwrap_or_else(|| format!("studio-attempt-{}", uuid::Uuid::new_v4()))
}

fn register_studio_attempt<'a>(
    state: &'a AppState,
    notebook_id: &str,
    attempt_id: &str,
) -> Result<StudioAttemptGuard<'a>, GlossError> {
    let cancellation = CancellationToken::new();
    let mut active = state
        .active_studio_attempts
        .lock()
        .map_err(|e| GlossError::Other(e.to_string()))?;
    if let Some(existing) = active.get(notebook_id) {
        return Err(GlossError::Studio {
            output_type: "runtime".to_string(),
            message: format!(
                "studio_generation_in_flight: active_attempt_id={}; repeated clicks reuse the active attempt until it completes or is cancelled",
                existing.attempt_id
            ),
        });
    }
    active.insert(
        notebook_id.to_string(),
        ActiveStudioAttempt {
            notebook_id: notebook_id.to_string(),
            attempt_id: attempt_id.to_string(),
            cancellation: cancellation.clone(),
        },
    );
    Ok(StudioAttemptGuard {
        state,
        notebook_id: notebook_id.to_string(),
        attempt_id: attempt_id.to_string(),
        cancellation,
    })
}

fn studio_source_readiness(
    sources: &[Source],
    chunks_by_source: &[(String, Vec<Chunk>)],
    requested_source_ids: Option<&[String]>,
    effective_source_ids: &[String],
) -> StudioSourceReadiness {
    let effective = effective_source_ids.iter().cloned().collect::<HashSet<_>>();
    let requested = requested_source_ids.unwrap_or(&[]);
    let requested_set = requested.iter().cloned().collect::<HashSet<_>>();
    let any_selected = sources.iter().any(|source| source.selected);
    let scoped_sources = sources.iter().filter(|source| {
        if !requested.is_empty() {
            requested_set.contains(&source.id)
        } else if any_selected {
            source.selected
        } else {
            true
        }
    });
    let skipped_sources = scoped_sources
        .filter(|source| !effective.contains(&source.id))
        .map(|source| {
            let chunks = chunks_by_source
                .iter()
                .find(|(source_id, _)| source_id == &source.id)
                .map(|(_, chunks)| chunks.as_slice())
                .unwrap_or(&[]);
            let has_text = source
                .content_text
                .as_deref()
                .is_some_and(|text| !text.trim().is_empty());
            let reason = if source.status != "ready" {
                "source_not_ready"
            } else if chunks.is_empty() && !has_text {
                "no_text_or_chunks"
            } else {
                "source_or_snippet_limit"
            };
            StudioSkippedSource {
                source_id: source.id.clone(),
                title: source.title.clone(),
                status: source.status.clone(),
                reason,
            }
        })
        .collect::<Vec<_>>();
    StudioSourceReadiness {
        schema: "StudioSourceReadinessV1",
        requested_source_ids: requested.to_vec(),
        effective_source_ids: effective_source_ids.to_vec(),
        ready_source_count: effective_source_ids.len(),
        skipped_source_count: skipped_sources.len(),
        skipped_sources,
    }
}

fn handle_studio_generation_failure(
    failure: StudioGenerationFailure,
    attempt_id: &str,
    deterministic_prompt: &str,
    fallback_receipt: &mut Option<StudioFallbackReceipt>,
) -> Result<(Option<String>, String), GlossError> {
    match failure.reason {
        StudioGenerationFailureReason::Terminal(StudioTerminalFailureReason::Cancelled) => {
            Err(GlossError::Studio {
                output_type: "runtime".to_string(),
                message: failure.message,
            })
        }
        StudioGenerationFailureReason::Fallback(reason) => {
            tracing::warn!(
                reason = reason.as_str(),
                attempt_id = %attempt_id,
                "Studio provider generation failed; using deterministic source-bound template"
            );
            *fallback_receipt = Some(StudioFallbackReceipt {
                schema: "StudioFallbackReceiptV1",
                receipt_id: format!("studio-fallback-{}", uuid::Uuid::new_v4()),
                attempt_id: attempt_id.to_string(),
                reason,
                reason_code: reason.as_str(),
                provider_cancelled: matches!(
                    reason,
                    StudioFallbackReason::ProviderStartTimeout
                        | StudioFallbackReason::FirstTokenTimeout
                        | StudioFallbackReason::StreamIdleTimeout
                ),
                elapsed_ms: failure.elapsed_ms,
                detail: failure.message,
                recorded_utc: chrono::Utc::now().to_rfc3339(),
            });
            Ok((None, deterministic_prompt.to_string()))
        }
    }
}

#[tauri::command]
pub async fn export_studio_output(
    notebook_id: String,
    output_id: String,
    state: State<'_, AppState>,
) -> Result<StudioExportReceipt, GlossError> {
    let nb_dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let notebook = app_db.get_notebook(&notebook_id)?;
        PathBuf::from(notebook.directory)
    };
    let receipt_notebook_id = notebook_id.clone();
    state.with_notebook_db_write(&notebook_id, |db| {
        let output = db.get_studio_output(&output_id)?;
        let raw_content = output
            .raw_content
            .as_deref()
            .ok_or_else(|| GlossError::Studio {
                output_type: output.output_type.clone(),
                message: "Studio output has no exportable content".to_string(),
            })?;
        let export_dir = nb_dir.join("exports").join("studio");
        std::fs::create_dir_all(&export_dir)?;
        let filename = studio_export_filename(&output);
        let export_path = export_dir.join(&filename);
        let payload = studio_export_payload(&output, raw_content)?;
        std::fs::write(&export_path, payload.as_bytes())?;
        let bytes_written = std::fs::metadata(&export_path)?.len();
        let digest = sha256_hex(payload.as_bytes());
        let relative_path = Path::new("exports")
            .join("studio")
            .join(filename)
            .to_string_lossy()
            .to_string();
        db.update_studio_output_file_path(&output.id, &relative_path)?;
        Ok(StudioExportReceipt {
            schema: "StudioExportReceiptV1",
            receipt_id: format!("studio-export-{}", uuid::Uuid::new_v4()),
            output_id: output.id,
            output_type: output.output_type,
            notebook_id: receipt_notebook_id,
            format: "json",
            file_path: relative_path,
            file_path_redacted: redact_path(&export_path),
            bytes_written,
            sha256: digest,
            recorded_utc: chrono::Utc::now().to_rfc3339(),
        })
    })
}

fn studio_output_view(output: StudioOutput) -> Result<StudioOutputView, GlossError> {
    let source_ids = match output.source_ids.as_deref() {
        Some(raw) => serde_json::from_str(raw)?,
        None => Vec::new(),
    };
    let config = match output.config.as_deref() {
        Some(raw) => Some(serde_json::from_str(raw)?),
        None => None,
    };
    Ok(StudioOutputView {
        id: output.id,
        output_type: output.output_type,
        title: output.title,
        prompt_used: output.prompt_used,
        raw_content: output.raw_content,
        prose_content: output.prose_content,
        config,
        source_ids,
        file_path: output.file_path,
        status: output.status,
        error_message: output.error_message,
        created_at: output.created_at,
    })
}

fn studio_export_filename(output: &StudioOutput) -> String {
    let short_id = output.id.chars().take(12).collect::<String>();
    format!(
        "{}-{}.studio.json",
        sanitize_export_component(&output.output_type),
        short_id
    )
}

fn sanitize_export_component(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn studio_export_payload(output: &StudioOutput, raw_content: &str) -> Result<String, GlossError> {
    let artifact: serde_json::Value = serde_json::from_str(raw_content)?;
    serde_json::to_string_pretty(&json!({
        "schema": "StudioExportPackageV1",
        "output_id": output.id,
        "output_type": output.output_type,
        "title": output.title,
        "prompt_used": output.prompt_used,
        "source_ids": output.source_ids,
        "created_at": output.created_at,
        "artifact": artifact,
    }))
    .map_err(GlossError::from)
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn truncate_to_char_boundary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let truncated: String = text.chars().take(max_chars).collect();
    format!("{truncated}\n[...truncated]")
}

/// Use the same GPU -> LLM order as chat and background work. Cancellation
/// while queued drops any acquired permit before returning to the caller.
async fn acquire_studio_permits<'a>(
    gpu: &'a tokio::sync::Semaphore,
    llm: &'a tokio::sync::Semaphore,
    cancellation: &CancellationToken,
    attempt_id: &str,
) -> Result<
    (
        tokio::sync::SemaphorePermit<'a>,
        tokio::sync::SemaphorePermit<'a>,
    ),
    StudioGenerationFailure,
> {
    let gpu_permit = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(studio_cancelled(attempt_id, 0)),
        permit = gpu.acquire() => permit.map_err(|e| {
            studio_provider_error(attempt_id, &format!("Failed to acquire GPU gate: {e}"), 0)
        })?,
    };
    let llm_permit = tokio::select! {
        biased;
        _ = cancellation.cancelled() => return Err(studio_cancelled(attempt_id, 0)),
        permit = llm.acquire() => permit.map_err(|e| {
            studio_provider_error(attempt_id, &format!("Failed to acquire LLM gate: {e}"), 0)
        })?,
    };
    Ok((gpu_permit, llm_permit))
}

/// Run a single non-streaming Studio LLM call behind the LLM/GPU gates.
async fn run_studio_llm(
    state: &AppState,
    system_prompt: String,
    user_prompt: String,
    temperature: f32,
    purpose: &str,
    cancellation: CancellationToken,
    attempt_id: &str,
) -> Result<(String, StudioProviderRuntimeReceipt), StudioGenerationFailure> {
    // Resolve provider config through the shared path — same as chat uses.
    // This ensures both paths agree on model selection, provider routing,
    // and context window detection.
    let resolved = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| studio_provider_error(attempt_id, &e.to_string(), 0))?;
        let registry = state
            .model_registry
            .lock()
            .map_err(|e| studio_provider_error(attempt_id, &e.to_string(), 0))?;
        crate::providers::resolve_llm_config(
            &app_db,
            &state.secret_store,
            Some(&*registry),
            None, // use default_model from settings — no hardcoded fallback
        )
        .map_err(|e| studio_provider_error(attempt_id, &e.to_string(), 0))?
    };
    let config = resolved.config;
    let model = resolved.model;
    let model_context_window = resolved.model_context_window;
    let provider_name = config.provider_type.as_str().to_string();

    let (_gpu_permit, _llm_permit) =
        acquire_studio_permits(&state.gpu_gate, &state.llm_gate, &cancellation, attempt_id).await?;

    let provider = build_provider(&config)
        .map_err(|e| studio_provider_error(attempt_id, &e.to_string(), 0))?;
    let request = ChatRequest {
        model: model.clone(),
        system_prompt: Some(system_prompt),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: user_prompt,
            images: None,
        }],
        max_tokens: 4096, // Studio generates structured content — keep a generous output window
        temperature,
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        stream: false, // Studio collects full structured JSON, not token-by-token chat
        num_ctx: Some(model_context_window.unwrap_or(16384) as u32),
    };

    let start = Instant::now();
    tracing::info!(
        attempt_id = %attempt_id,
        purpose = %purpose,
        provider = %provider_name,
        model = %model,
        provider_start_timeout_s = STUDIO_PROVIDER_START_TIMEOUT.as_secs(),
        first_token_timeout_s = STUDIO_FIRST_TOKEN_TIMEOUT.as_secs(),
        stream_idle_timeout_s = STUDIO_STREAM_IDLE_TIMEOUT.as_secs(),
        "Studio provider call starting"
    );
    let studio_context = LlmExecutionContext::new(
        cancellation.clone(),
        LlmPhaseTimeouts {
            provider_start: STUDIO_PROVIDER_START_TIMEOUT,
            first_token: STUDIO_FIRST_TOKEN_TIMEOUT,
            stream_idle: STUDIO_STREAM_IDLE_TIMEOUT,
        },
    )
    .with_attempt_id(attempt_id.to_string());
    let chat_future = provider.chat(request, studio_context.clone());
    tokio::pin!(chat_future);
    let mut stream = tokio::select! {
        result = &mut chat_future => {
            match result {
                Ok(stream) => stream,
                Err(_error) if cancellation.is_cancelled() => {
                    return Err(studio_cancelled(attempt_id, start.elapsed().as_millis()));
                }
                Err(error) => {
                    return Err(studio_provider_error(attempt_id, &error.to_string(), start.elapsed().as_millis()));
                }
            }
        }
        _ = cancellation.cancelled() => {
            return Err(studio_cancelled(attempt_id, start.elapsed().as_millis()));
        }
        _ = tokio::time::sleep(studio_context.timeouts.provider_start) => {
            studio_context.cancellation.cancel();
            return Err(StudioGenerationFailure::fallback(
                StudioFallbackReason::ProviderStartTimeout,
                format!(
                    "Studio provider start timed out after {}s",
                    studio_context.timeouts.provider_start.as_secs()
                ),
                start.elapsed().as_millis(),
            ));
        }
    };
    let mut response = String::new();
    let mut saw_token = false;
    loop {
        let phase_timeout = if saw_token {
            studio_context.timeouts.stream_idle
        } else {
            studio_context.timeouts.first_token
        };
        let timeout_reason = if saw_token {
            StudioFallbackReason::StreamIdleTimeout
        } else {
            StudioFallbackReason::FirstTokenTimeout
        };
        let timeout_phase = if saw_token {
            "stream idle"
        } else {
            "first token"
        };
        let next = tokio::select! {
            token_result = stream.next() => token_result,
            _ = cancellation.cancelled() => {
                return Err(studio_cancelled(attempt_id, start.elapsed().as_millis()));
            }
            _ = tokio::time::sleep(phase_timeout) => {
                studio_context.cancellation.cancel();
                return Err(StudioGenerationFailure::fallback(
                    timeout_reason,
                    format!(
                        "Studio provider {timeout_phase} timed out after {}s",
                        phase_timeout.as_secs()
                    ),
                    start.elapsed().as_millis(),
                ));
            }
        };
        let Some(token_result) = next else {
            break;
        };
        let token = match token_result {
            Ok(token) => token,
            Err(_error) if cancellation.is_cancelled() => {
                return Err(studio_cancelled(attempt_id, start.elapsed().as_millis()));
            }
            Err(error) => {
                return Err(studio_provider_error(
                    attempt_id,
                    &error.to_string(),
                    start.elapsed().as_millis(),
                ));
            }
        };
        saw_token = true;
        response.push_str(&token.token);
    }
    if response.trim().is_empty() {
        return Err(studio_provider_error(
            attempt_id,
            "Provider completed without usable Studio content",
            start.elapsed().as_millis(),
        ));
    }
    tracing::info!(
        attempt_id = %attempt_id,
        purpose = %purpose,
        elapsed_ms = start.elapsed().as_millis(),
        chars = response.trim().len(),
        "Studio provider call completed"
    );
    Ok((
        response.trim().to_string(),
        StudioProviderRuntimeReceipt {
            schema: "StudioProviderRuntimeReceiptV1",
            attempt_id: attempt_id.to_string(),
            purpose: purpose.to_string(),
            phase: "completed",
            provider: provider_name,
            model,
            elapsed_ms: start.elapsed().as_millis(),
            provider_cancelled: false,
            recorded_utc: chrono::Utc::now().to_rfc3339(),
        },
    ))
}

fn studio_cancelled(attempt_id: &str, elapsed_ms: u128) -> StudioGenerationFailure {
    StudioGenerationFailure::terminal(
        StudioTerminalFailureReason::Cancelled,
        format!("Studio generation cancelled: attempt_id={attempt_id}"),
        elapsed_ms,
    )
}

fn studio_provider_error(
    attempt_id: &str,
    message: &str,
    elapsed_ms: u128,
) -> StudioGenerationFailure {
    StudioGenerationFailure::fallback(
        StudioFallbackReason::ProviderError,
        format!("Studio provider error for attempt_id={attempt_id}: {message}"),
        elapsed_ms,
    )
}

fn studio_source_material(full_texts: &[(String, String)]) -> String {
    full_texts
        .iter()
        .map(|(src_title, text)| format!("# {src_title}\n\n{text}"))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n")
}

/// Convert source-grounded data into natural-language prose via the LLM.
///
/// Receives FULL source texts (not just 360-char snippets) and uses
/// type-specific system prompts so each Studio output button produces
/// genuinely differentiated content.
async fn refine_studio_artifact(
    artifact: &crate::studio::StudioArtifact,
    full_texts: &[(String, String)], // (title, text)
    state: &AppState,
    cancellation: CancellationToken,
    attempt_id: &str,
) -> Result<(String, StudioProviderRuntimeReceipt), StudioGenerationFailure> {
    let source_material = studio_source_material(full_texts);
    let (system_prompt, user_prompt) = studio_prompt_for_kind(
        &artifact.output_type,
        &artifact.title,
        &artifact.output_type,
        &source_material,
    );
    run_studio_llm(
        state,
        system_prompt,
        user_prompt,
        0.3,
        &artifact.output_type,
        cancellation,
        attempt_id,
    )
    .await
}

// ---------------------------------------------------------------------------
// Structured widget generation (flashcards / quiz / mind map)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
struct LlmFlashcards {
    cards: Vec<LlmFlashcard>,
}

#[derive(Debug, Deserialize)]
struct LlmFlashcard {
    front: String,
    back: String,
    #[serde(default)]
    difficulty: Option<String>,
    #[serde(default)]
    source_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmQuiz {
    questions: Vec<LlmQuizItem>,
}

#[derive(Debug, Deserialize)]
struct LlmQuizItem {
    question: String,
    choices: Vec<String>,
    answer_index: usize,
    #[serde(default)]
    explanation: Option<String>,
    #[serde(default)]
    source_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmMindMap {
    nodes: Vec<LlmMindMapNode>,
    #[serde(default)]
    edges: Vec<LlmMindMapEdge>,
}

#[derive(Debug, Deserialize)]
struct LlmMindMapNode {
    id: String,
    label: String,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    source_title: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LlmMindMapEdge {
    from: String,
    to: String,
    #[serde(default)]
    label: Option<String>,
}

/// Find the citation whose source title matches the LLM-reported title;
/// fall back to the first citation so every item stays source-anchored.
fn citation_for_title<'a>(
    pool: &'a [StudioCitation],
    source_title: Option<&str>,
) -> Option<&'a StudioCitation> {
    let by_title = source_title.and_then(|wanted| {
        pool.iter()
            .find(|citation| citation.source_title.eq_ignore_ascii_case(wanted.trim()))
    });
    by_title.or_else(|| pool.first())
}

fn citations_json(citation: Option<&StudioCitation>) -> serde_json::Value {
    match citation {
        Some(citation) => json!([citation]),
        None => json!([]),
    }
}

/// Generate real, LLM-authored structured content for the interactive Studio
/// widgets, with per-item citations injected from the source scope.
#[allow(clippy::too_many_arguments)]
async fn generate_structured_widget_content(
    kind: StudioOutputKind,
    artifact: &crate::studio::StudioArtifact,
    full_texts: &[(String, String)],
    citation_pool: &[StudioCitation],
    max_items: usize,
    state: &AppState,
    cancellation: CancellationToken,
    attempt_id: &str,
) -> Result<(serde_json::Value, StudioProviderRuntimeReceipt), StudioGenerationFailure> {
    let source_material = studio_source_material(full_texts);
    let title = &artifact.title;
    let structured_err = |message: String| {
        StudioGenerationFailure::fallback(StudioFallbackReason::InvalidStructuredOutput, message, 0)
    };

    match kind {
        StudioOutputKind::Flashcards => {
            let system = "You create study flashcards from source material. Respond with ONLY a \
                          valid JSON object — no markdown fences, no commentary."
                .to_string();
            let user = format!(
                "Create up to {max_items} study flashcards from the sources below.\n\
                 Each card must test one self-contained fact or concept from the sources.\n\
                 Respond with JSON exactly in this shape:\n\
                 {{\"cards\":[{{\"front\":\"question or prompt\",\"back\":\"answer\",\
                 \"difficulty\":\"easy|medium|hard\",\
                 \"source_title\":\"exact title of the supporting source\"}}]}}\n\n\
                 Title: {title}\n\n## Sources\n\n{source_material}"
            );
            let (response, receipt) = run_studio_llm(
                state,
                system,
                user,
                0.2,
                "flashcards_structured",
                cancellation,
                attempt_id,
            )
            .await?;
            let parsed: LlmFlashcards = llm_pipeline::parsing::parse_as(&response)
                .map_err(|e| structured_err(format!("flashcards JSON parse failed: {e}")))?;
            let cards = parsed
                .cards
                .into_iter()
                .filter(|card| !card.front.trim().is_empty() && !card.back.trim().is_empty())
                .take(max_items)
                .map(|card| {
                    let citation = citation_for_title(citation_pool, card.source_title.as_deref());
                    json!({
                        "front": card.front.trim(),
                        "back": card.back.trim(),
                        "difficulty": card.difficulty,
                        "citations": citations_json(citation),
                    })
                })
                .collect::<Vec<_>>();
            if cards.is_empty() {
                return Err(structured_err("LLM produced no usable flashcards".into()));
            }
            Ok((json!({ "cards": cards }), receipt))
        }
        StudioOutputKind::Quiz => {
            let system = "You design multiple-choice quizzes from source material. Respond with \
                          ONLY a valid JSON object — no markdown fences, no commentary."
                .to_string();
            let user = format!(
                "Write up to {max_items} multiple-choice questions testing understanding of the \
                 sources below. Each question needs exactly 4 plausible choices with one correct \
                 answer, and a short explanation grounded in the sources.\n\
                 Respond with JSON exactly in this shape:\n\
                 {{\"questions\":[{{\"question\":\"...\",\"choices\":[\"a\",\"b\",\"c\",\"d\"],\
                 \"answer_index\":0,\"explanation\":\"why the answer is correct\",\
                 \"source_title\":\"exact title of the supporting source\"}}]}}\n\n\
                 Title: {title}\n\n## Sources\n\n{source_material}"
            );
            let (response, receipt) = run_studio_llm(
                state,
                system,
                user,
                0.2,
                "quiz_structured",
                cancellation,
                attempt_id,
            )
            .await?;
            let parsed: LlmQuiz = llm_pipeline::parsing::parse_as(&response)
                .map_err(|e| structured_err(format!("quiz JSON parse failed: {e}")))?;
            let questions = parsed
                .questions
                .into_iter()
                .filter(|item| {
                    !item.question.trim().is_empty()
                        && item.choices.len() >= 2
                        && item.answer_index < item.choices.len()
                })
                .take(max_items)
                .map(|item| {
                    let citation = citation_for_title(citation_pool, item.source_title.as_deref());
                    json!({
                        "question": item.question.trim(),
                        "choices": item.choices,
                        "answer_index": item.answer_index,
                        "explanation": item.explanation,
                        "citations": citations_json(citation),
                    })
                })
                .collect::<Vec<_>>();
            if questions.is_empty() {
                return Err(structured_err(
                    "LLM produced no usable quiz questions".into(),
                ));
            }
            Ok((json!({ "questions": questions }), receipt))
        }
        StudioOutputKind::MindMap => {
            let system = "You extract concept graphs from source material for mind map \
                          visualization. Respond with ONLY a valid JSON object — no markdown \
                          fences, no commentary."
                .to_string();
            let user = format!(
                "Extract the key concepts from the sources below and map their relationships as \
                 a graph with up to {max_items} concept nodes plus one central topic node. Use \
                 short labels and connect every node to the graph.\n\
                 Respond with JSON exactly in this shape:\n\
                 {{\"nodes\":[{{\"id\":\"n1\",\"label\":\"concept\",\"summary\":\"one sentence\",\
                 \"source_title\":\"exact title of the supporting source\"}}],\
                 \"edges\":[{{\"from\":\"n1\",\"to\":\"n2\",\"label\":\"relationship\"}}]}}\n\n\
                 Title: {title}\n\n## Sources\n\n{source_material}"
            );
            let (response, receipt) = run_studio_llm(
                state,
                system,
                user,
                0.2,
                "mind_map_structured",
                cancellation,
                attempt_id,
            )
            .await?;
            let parsed: LlmMindMap = llm_pipeline::parsing::parse_as(&response)
                .map_err(|e| structured_err(format!("mind map JSON parse failed: {e}")))?;
            let node_ids = parsed
                .nodes
                .iter()
                .map(|node| node.id.clone())
                .collect::<std::collections::HashSet<_>>();
            let nodes = parsed
                .nodes
                .iter()
                .filter(|node| !node.label.trim().is_empty())
                .map(|node| {
                    let citation = citation_for_title(citation_pool, node.source_title.as_deref());
                    json!({
                        "id": node.id,
                        "label": node.label.trim(),
                        "summary": node.summary,
                        "citations": citations_json(citation),
                    })
                })
                .collect::<Vec<_>>();
            let edges = parsed
                .edges
                .iter()
                .filter(|edge| node_ids.contains(&edge.from) && node_ids.contains(&edge.to))
                .map(|edge| {
                    json!({
                        "from": edge.from,
                        "to": edge.to,
                        "label": edge.label,
                    })
                })
                .collect::<Vec<_>>();
            if nodes.is_empty() {
                return Err(structured_err(
                    "LLM produced no usable mind map nodes".into(),
                ));
            }
            Ok((json!({ "nodes": nodes, "edges": edges }), receipt))
        }
        _ => Err(structured_err(
            "structured generation only supports widget output kinds".into(),
        )),
    }
}

/// Build type-specific prompts so each Studio button produces differentiated output.
fn studio_prompt_for_kind(
    kind: &str,
    title: &str,
    _kind_label: &str,
    source_material: &str,
) -> (String, String) {
    let prompts = studio_prompts();
    if let Some(tmpl) = prompts.get(kind) {
        return (
            tmpl.system_prompt.clone(),
            tmpl.user_prompt
                .replace("{title}", title)
                .replace("{source_material}", source_material),
        );
    }
    // Generic fallback for unknown output types
    let label = kind.replace('_', " ");
    (
        format!(
            "You produce a well-structured {label} from source material. \
             Be factual, source-grounded, and never invent information."
        ),
        format!(
            "## Task: {label}\n\n\
             Title: {title}\n\n\
             Produce a {label} based on the following sources:\n\n\
             {source_material}"
        ),
    )
}

#[derive(Debug, Deserialize, Clone)]
struct StudioPromptTemplate {
    system_prompt: String,
    user_prompt: String,
}

static STUDIO_PROMPTS_TOML: &str = include_str!("../../../prompts/studio_prompts.toml");

fn studio_prompts() -> &'static HashMap<String, StudioPromptTemplate> {
    static PROMPTS: OnceLock<HashMap<String, StudioPromptTemplate>> = OnceLock::new();
    PROMPTS.get_or_init(|| {
        toml::from_str(STUDIO_PROMPTS_TOML)
            .expect("prompts/studio_prompts.toml is malformed — fix at compile time")
    })
}
#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn studio_gate_wait_uses_gpu_first_and_releases_on_cancellation() {
        for block_gpu in [true, false] {
            let gpu = tokio::sync::Semaphore::new(1);
            let llm = tokio::sync::Semaphore::new(1);
            let held = if block_gpu {
                gpu.acquire().await.unwrap()
            } else {
                llm.acquire().await.unwrap()
            };
            let cancellation = CancellationToken::new();
            {
                let waiting = acquire_studio_permits(&gpu, &llm, &cancellation, "fixture");
                tokio::pin!(waiting);
                assert!(
                    tokio::time::timeout(std::time::Duration::from_millis(20), &mut waiting)
                        .await
                        .is_err()
                );
                if block_gpu {
                    assert_eq!(
                        llm.available_permits(),
                        1,
                        "Studio must not hold LLM while waiting for GPU"
                    );
                } else {
                    assert_eq!(gpu.available_permits(), 0);
                }
                cancellation.cancel();
                assert!(
                    tokio::time::timeout(std::time::Duration::from_secs(1), &mut waiting)
                        .await
                        .unwrap()
                        .is_err()
                );
            }
            drop(held);
            assert_eq!(gpu.available_permits(), 1);
            assert_eq!(llm.available_permits(), 1);
        }
    }

    fn source(id: &str, title: &str, status: &str, selected: bool) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: title.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: Some(format!("{title} body text")),
            word_count: Some(3),
            metadata: None,
            summary: None,
            summary_model: None,
            status: status.to_string(),
            error_message: None,
            selected,
            created_at: "2026-06-12T00:00:00Z".to_string(),
            updated_at: "2026-06-12T00:00:00Z".to_string(),
            processing_state: None,
        }
    }

    fn chunk(source_id: &str, id: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            source_id: source_id.to_string(),
            chunk_index: 0,
            content: "chunk text".to_string(),
            token_count: Some(2),
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        }
    }

    fn output_for_export() -> StudioOutput {
        StudioOutput {
            id: "studio-artifact-abcdef1234567890".to_string(),
            output_type: "compare table".to_string(),
            title: Some("Compare".to_string()),
            prompt_used: "deterministic_source_bound_template_v1".to_string(),
            raw_content: Some(
                serde_json::json!({
                    "schema": "StudioArtifactV1",
                    "content": {"rows": []},
                    "validation": {
                        "schema_validated": true,
                        "all_items_source_cited": true
                    }
                })
                .to_string(),
            ),
            config: None,
            source_ids: Some("[\"source-1\"]".to_string()),
            file_path: None,
            status: "ready".to_string(),
            error_message: None,
            prose_content: None,
            created_at: "2026-05-26T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn studio_export_filename_is_bounded_and_safe() {
        let output = output_for_export();
        assert_eq!(
            studio_export_filename(&output),
            "compare-table-studio-artif.studio.json"
        );
    }

    #[test]
    fn studio_export_payload_wraps_artifact_with_manifest_fields() {
        let output = output_for_export();
        let payload =
            studio_export_payload(&output, output.raw_content.as_deref().unwrap()).unwrap();
        let value: serde_json::Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(value["schema"], "StudioExportPackageV1");
        assert_eq!(value["output_id"], output.id);
        assert_eq!(value["artifact"]["schema"], "StudioArtifactV1");
        assert_eq!(
            value["artifact"]["validation"]["all_items_source_cited"],
            true
        );
    }

    #[test]
    fn studio_attempt_id_is_normalized_or_generated() {
        assert_eq!(
            normalize_studio_attempt_id(Some("  studio-attempt-client  ".to_string())),
            "studio-attempt-client"
        );
        assert!(normalize_studio_attempt_id(Some(" ".to_string())).starts_with("studio-attempt-"));
        assert!(normalize_studio_attempt_id(None).starts_with("studio-attempt-"));
    }

    #[test]
    fn studio_source_readiness_records_skipped_sources() {
        let sources = vec![
            source("src-ready", "Ready", "ready", true),
            source("src-processing", "Processing", "processing", true),
        ];
        let chunks = vec![
            ("src-ready".to_string(), vec![chunk("src-ready", "chunk-1")]),
            ("src-processing".to_string(), Vec::new()),
        ];
        let readiness =
            studio_source_readiness(&sources, &chunks, None, &["src-ready".to_string()]);

        assert_eq!(readiness.schema, "StudioSourceReadinessV1");
        assert_eq!(readiness.ready_source_count, 1);
        assert_eq!(readiness.skipped_source_count, 1);
        assert_eq!(readiness.skipped_sources[0].reason, "source_not_ready");
    }

    #[test]
    fn studio_fallback_receipt_uses_typed_reason() {
        let failure = StudioGenerationFailure::fallback(
            StudioFallbackReason::FirstTokenTimeout,
            "timed out".to_string(),
            61000,
        );
        let mut receipt = None;
        let (prose, prompt) =
            handle_studio_generation_failure(failure, "attempt-1", "deterministic", &mut receipt)
                .unwrap();

        assert!(prose.is_none());
        assert_eq!(prompt, "deterministic");
        let receipt = receipt.unwrap();
        assert_eq!(receipt.schema, "StudioFallbackReceiptV1");
        assert_eq!(receipt.reason, StudioFallbackReason::FirstTokenTimeout);
        assert_eq!(receipt.reason_code, "first_token_timeout");
        assert!(receipt.provider_cancelled);
    }
}
