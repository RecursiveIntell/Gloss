use crate::db::notebook_db::StudioOutput;
use crate::error::GlossError;
use crate::redaction::redact_path;
use crate::state::AppState;
use crate::studio::{build_snippets, generate_artifact, StudioOutputKind};
use serde::Serialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::State;

const DEFAULT_STUDIO_MAX_ITEMS: usize = 8;
const MAX_STUDIO_ITEMS: usize = 20;

#[derive(Debug, Clone, Serialize)]
pub struct StudioOutputView {
    pub id: String,
    pub output_type: String,
    pub title: Option<String>,
    pub prompt_used: String,
    pub raw_content: Option<String>,
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
    state: State<'_, AppState>,
) -> Result<StudioOutputView, GlossError> {
    let kind = StudioOutputKind::parse(&output_type)?;
    let max_items = max_items
        .unwrap_or(DEFAULT_STUDIO_MAX_ITEMS)
        .clamp(1, MAX_STUDIO_ITEMS);

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
        let output = StudioOutput {
            id: artifact.receipt_id.clone(),
            output_type: artifact.output_type.clone(),
            title: Some(artifact.title.clone()),
            prompt_used: artifact.prompt_used.clone(),
            raw_content: Some(raw_content),
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
