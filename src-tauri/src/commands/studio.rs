use crate::db::notebook_db::StudioOutput;
use crate::error::GlossError;
use crate::providers::{build_provider, ChatMessage, ChatRequest};
use crate::redaction::redact_path;
use crate::state::AppState;
use crate::studio::{build_snippets, generate_artifact, StudioOutputKind};
use futures::StreamExt;
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::time::Instant;
use tauri::State;

const DEFAULT_STUDIO_MAX_ITEMS: usize = 8;
const MAX_STUDIO_ITEMS: usize = 20;
/// Max chars of source text to feed the LLM per source (keeps context window manageable).
const MAX_SOURCE_CHARS_FOR_LLM: usize = 4000;

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
pub async fn generate_studio_output(
    notebook_id: String,
    output_type: String,
    source_ids: Option<Vec<String>>,
    title: Option<String>,
    max_items: Option<usize>,
    refine: Option<bool>,
    state: State<'_, AppState>,
) -> Result<StudioOutputView, GlossError> {
    let kind = StudioOutputKind::parse(&output_type)?;
    let max_items = max_items
        .unwrap_or(DEFAULT_STUDIO_MAX_ITEMS)
        .clamp(1, MAX_STUDIO_ITEMS);
    let should_refine = refine.unwrap_or(true);

    state.with_notebook_db_write(&notebook_id, |db| {
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
        let artifact = generate_artifact(kind, title, scope, &snippets)?;
        let raw_content = serde_json::to_string_pretty(&artifact)?;
        let config = serde_json::to_string(&json!({
            "schema": "StudioOutputConfigV1",
            "deterministic": true,
            "source_bound": true,
            "schema_validated": artifact.validation.schema_validated,
            "all_items_source_cited": artifact.validation.all_items_source_cited,
            "max_items": max_items,
            "receipt_id": artifact.receipt_id,
        }))?;
        let source_ids_json = serde_json::to_string(&artifact.source_scope.effective_source_ids)?;
        let now = chrono::Utc::now().to_rfc3339();

        // Collect full source texts for LLM refinement.
        // list_sources() excludes content_text (heavy field), so we must call
        // get_source() for each effective source to get the actual text.
        let mut full_texts: Vec<(String, String)> = Vec::new();
        for sid in &artifact.source_scope.effective_source_ids {
            if let Ok(full_source) = db.get_source(sid) {
                if let Some(text) = full_source.content_text.as_deref() {
                    if !text.trim().is_empty() {
                        let truncated = truncate_to_char_boundary(text, MAX_SOURCE_CHARS_FOR_LLM);
                        full_texts.push((full_source.title, truncated));
                    }
                }
            }
        }

        // Optional LLM refinement pass — now uses full source content
        let (prose_content, prompt_used) = if should_refine {
            let artifact_clone = artifact.clone();
            let nb = notebook_id.clone();
            let result = tokio::task::block_in_place(|| {
                let rt = tokio::runtime::Handle::current();
                rt.block_on(refine_studio_artifact(
                    &artifact_clone,
                    &full_texts,
                    &state,
                    &nb,
                ))
            });
            match result {
                Ok(prose) => (Some(prose), "source_grounded_refined_v1".to_string()),
                Err(e) => {
                    tracing::warn!(
                        "Studio LLM refinement failed, falling back: {e}"
                    );
                    (None, artifact.prompt_used.clone())
                }
            }
        } else {
            (None, artifact.prompt_used.clone())
        };

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
        db.insert_studio_output(&output)?;
        studio_output_view(output)
    })
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

/// Convert source-grounded data into natural-language prose via the LLM.
///
/// Now receives FULL source texts (not just 360-char snippets) and uses
/// type-specific system prompts so each Studio output button produces
/// genuinely differentiated content.
async fn refine_studio_artifact(
    artifact: &crate::studio::StudioArtifact,
    full_texts: &[(String, String)], // (title, text)
    state: &AppState,
    _notebook_id: &str,
) -> Result<String, GlossError> {
    let (config, model) = {
        let registry = state
            .model_registry
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let model = app_db
            .get_setting("default_model")?
            .unwrap_or_else(|| "qwen3.5:4b".to_string());
        let config =
            crate::providers::provider_config_from_db(&app_db, &state.secret_store, {
                let selected = app_db
                    .get_setting("default_provider")?
                    .and_then(|p| crate::providers::ProviderType::from_str(p.trim()));
                selected.unwrap_or(crate::providers::ProviderType::Ollama)
            })?;
        drop(app_db);
        drop(registry);
        (config, model)
    };

    let _llm_permit = state.llm_gate.acquire().await.map_err(|e| {
        GlossError::Other(format!("Failed to acquire LLM gate: {e}"))
    })?;
    let _gpu_permit = state.gpu_gate.acquire().await.map_err(|e| {
        GlossError::Other(format!("Failed to acquire GPU gate: {e}"))
    })?;

    let provider = build_provider(&config)?;
    let output_type_label = artifact.output_type.clone();
    let title = &artifact.title;

    // Build source material block from full texts
    let source_material = full_texts
        .iter()
        .map(|(src_title, text)| format!("# {src_title}\n\n{text}"))
        .collect::<Vec<_>>()
        .join("\n\n---\n\n");

    let (system_prompt, user_prompt) = studio_prompt_for_kind(
        &artifact.output_type,
        title,
        &output_type_label,
        &source_material,
    );

    let request = ChatRequest {
        model: model.clone(),
        system_prompt: Some(system_prompt),
        messages: vec![ChatMessage {
            role: "user".to_string(),
            content: user_prompt,
            images: None,
        }],
        max_tokens: 4096,
        temperature: 0.3,
        top_p: None,
        top_k: None,
        min_p: None,
        repeat_penalty: None,
        stream: false,
        num_ctx: Some(16384),
    };

    let start = Instant::now();
    let mut stream = provider.chat(request).await?;
    let mut response = String::new();
    while let Some(token_result) = stream.next().await {
        let token = token_result?;
        response.push_str(&token.token);
    }

    let elapsed = start.elapsed();
    tracing::info!(
        "Studio LLM refinement ({output_type_label}) completed in {}ms, output: {} chars",
        elapsed.as_millis(),
        response.trim().len()
    );

    Ok(response.trim().to_string())
}

/// Build type-specific prompts so each Studio button produces differentiated output.
fn studio_prompt_for_kind(
    kind: &str,
    title: &str,
    kind_label: &str,
    source_material: &str,
) -> (String, String) {
    match kind {
        "report" => (
            "You are an expert analyst writing a source-grounded report. \
             Synthesize the provided sources into a structured report. \
             Use clear section headers, cite specific claims from the sources, \
             and never fabricate information. Your tone is professional and objective."
                .to_string(),
            format!(
                "## Task: Source-Grounded Report\n\n\
                 Title: {title}\n\n\
                 Write a detailed report based EXCLUSIVELY on the following sources. \
                 Organize it with clear sections — e.g. Overview, Key Findings, Details, \
                 and Recommendations if applicable. \
                 Every claim must be traceable back to the sources.\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "summary" => (
            "You are a concise summarizer. Read the provided sources and produce \
             a tight, high-density summary covering only the most important points. \
             Be factual, don't editorialize. Use bullet points for key takeaways."
                .to_string(),
            format!(
                "## Task: Concise Summary\n\n\
                 Title: {title}\n\n\
                 Summarize ALL of the following sources into a concise overview. \
                 Capture the essential arguments, facts, and conclusions. \
                 Start with a 2-3 sentence overview, then use bullet points for \
                 the most important takeaways from each source. Keep it dense.\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "outline" => (
            "You are a structural editor. Build a hierarchical outline from the \
             provided source material. Use nested headings — main topics at the top, \
             subtopics indented underneath. Every heading must correspond to real content \
             in the sources. Don't invent structure where none exists."
                .to_string(),
            format!(
                "## Task: Hierarchical Outline\n\n\
                 Title: {title}\n\n\
                 Build a nested outline from the following sources. \
                 Use markdown headings (##, ###, ####) to show the hierarchy. \
                 Each heading must reflect real content — don't add fictional topics.\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "faq" => (
            "You are a knowledge curator writing a FAQ. Generate question-answer pairs \
             based on the source material. Questions should anticipate what a reader \
             would genuinely ask. Answers must be factual and cite the relevant source. \
             Format as Q&A."
                .to_string(),
            format!(
                "## Task: FAQ\n\n\
                 Title: {title}\n\n\
                 Generate 5-10 question-answer pairs based on the following sources. \
                 Questions should be natural and useful. Answers must be grounded in \
                 the source text. Format each as:\n\n\
                 **Q:** [question]\n\
                 **A:** [answer — cite the source]\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "flashcards" => (
            "You create study flashcards from source material. Each card has a front \
             (question or prompt) and a back (answer). Make them self-contained — \
             someone should be able to test themselves with just these cards."
                .to_string(),
            format!(
                "## Task: Flashcards\n\n\
                 Title: {title}\n\n\
                 Create 8-15 flashcards from the following sources. \
                 Format each card as:\n\n\
                 **Front:** [question / concept / term]\n\
                 **Back:** [answer / definition / explanation]\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "quiz" => (
            "You are a quiz designer. Write multiple-choice quiz questions \
             that test understanding of the source material. Each question needs \
             4 plausible choices with exactly one correct answer."
                .to_string(),
            format!(
                "## Task: Quiz\n\n\
                 Title: {title}\n\n\
                 Write 5-8 multiple-choice quiz questions based on these sources. \
                 For each question, provide 4 choices (A-D) and indicate the correct answer. \
                 Format as:\n\n\
                 **[N]. Question text**\n\
                 A. Choice one\n\
                 B. Choice two\n\
                 C. Choice three\n\
                 D. Choice four\n\n\
                 Answer: [letter]\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "mind_map" => (
            "You extract conceptual relationships from source material for mind map \
             visualization. Identify central concepts and their connections. \
             Output a hierarchical concept tree with labeled relationships."
                .to_string(),
            format!(
                "## Task: Mind Map / Concept Tree\n\n\
                 Title: {title}\n\n\
                 Extract the key concepts from these sources and map their relationships. \
                 Output as a hierarchical concept tree:\n\n\
                 - **Central concept** (root node)\n\
                   - Related concept with relationship label\n\
                     - Sub-concept\n\
                   - Another branch\n\n\
                 Keep relationships clear and labeled. Every concept must come from the sources.\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "timeline" => (
            "You reconstruct chronological sequences from source material. \
             Extract dated events and arrange them in temporal order. \
             If exact dates are missing, use relative ordering where the text indicates it."
                .to_string(),
            format!(
                "## Task: Timeline\n\n\
                 Title: {title}\n\n\
                 Extract all dated or time-sequenced events from these sources and \
                 arrange them chronologically. For each event include:\n\
                 - Date (approximate if exact unknown)\n\
                 - Event description\n\
                 - Source reference\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "compare_table" => (
            "You build comparison tables from source material. Identify comparable \
             entities, dimensions, or arguments and present them in a structured table. \
             Every cell must be source-grounded."
                .to_string(),
            format!(
                "## Task: Comparison Table\n\n\
                 Title: {title}\n\n\
                 Identify the key entities, approaches, or arguments in these sources \
                 and build a comparison table. Present as a markdown table with clear \
                 column headers. Every value must be traceable to the sources.\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        "action_plan" => (
            "You derive actionable plans from source material. Extract recommendations, \
             next steps, or implied actions and organize them into a concrete plan. \
             Each action must be justified by the source text."
                .to_string(),
            format!(
                "## Task: Action Plan\n\n\
                 Title: {title}\n\n\
                 Derive a concrete action plan from these sources. For each action include:\n\
                 - What needs to be done\n\
                 - Why (source justification)\n\
                 - Priority or suggested order\n\n\
                 ## Sources\n\n{source_material}"
            ),
        ),
        _ => (
            format!(
                "You produce a well-structured {kind_label} from source material. \
                 Be factual, source-grounded, and never invent information."
            ),
            format!(
                "## Task: {kind_label}\n\n\
                 Title: {title}\n\n\
                 Produce a {kind_label} based on the following sources:\n\n\
                 {source_material}"
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
