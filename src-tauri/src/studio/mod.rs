use crate::db::notebook_db::{Chunk, Source};
use crate::error::GlossError;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashSet;

pub const STUDIO_ARTIFACT_SCHEMA: &str = "StudioArtifactV1";
pub const STUDIO_RECEIPT_SCHEMA: &str = "StudioArtifactReceiptV1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StudioOutputKind {
    Report,
    Summary,
    Outline,
    Faq,
    Flashcards,
    Quiz,
    MindMap,
    Timeline,
    CompareTable,
    ActionPlan,
}

impl StudioOutputKind {
    pub fn parse(value: &str) -> Result<Self, GlossError> {
        match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
            "report" => Ok(Self::Report),
            "summary" => Ok(Self::Summary),
            "outline" => Ok(Self::Outline),
            "faq" => Ok(Self::Faq),
            "flashcards" | "flashcard" => Ok(Self::Flashcards),
            "quiz" => Ok(Self::Quiz),
            "mind_map" | "mindmap" => Ok(Self::MindMap),
            "timeline" => Ok(Self::Timeline),
            "compare_table" | "compare" | "table" => Ok(Self::CompareTable),
            "action" | "actions" | "action_plan" => Ok(Self::ActionPlan),
            other => Err(GlossError::Studio {
                output_type: other.to_string(),
                message: "unsupported Studio output type".to_string(),
            }),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Report => "report",
            Self::Summary => "summary",
            Self::Outline => "outline",
            Self::Faq => "faq",
            Self::Flashcards => "flashcards",
            Self::Quiz => "quiz",
            Self::MindMap => "mind_map",
            Self::Timeline => "timeline",
            Self::CompareTable => "compare_table",
            Self::ActionPlan => "action_plan",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioCitation {
    pub source_id: String,
    pub source_title: String,
    pub chunk_id: Option<String>,
}

#[derive(Debug, Clone)]
pub struct StudioSnippet {
    pub source_id: String,
    pub source_title: String,
    pub chunk_id: Option<String>,
    pub text: String,
}

impl StudioSnippet {
    pub fn citation(&self) -> StudioCitation {
        StudioCitation {
            source_id: self.source_id.clone(),
            source_title: self.source_title.clone(),
            chunk_id: self.chunk_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioSourceScope {
    pub scope_kind: String,
    pub requested_source_ids: Vec<String>,
    pub effective_source_ids: Vec<String>,
    pub source_count: usize,
    pub snippet_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioValidation {
    pub schema_validated: bool,
    pub all_items_source_cited: bool,
    pub deterministic: bool,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioArtifact {
    pub schema: String,
    pub receipt_schema: String,
    pub receipt_id: String,
    pub output_type: String,
    pub title: String,
    pub prompt_used: String,
    pub source_scope: StudioSourceScope,
    pub content: Value,
    pub validation: StudioValidation,
}

pub fn build_snippets(
    sources: &[Source],
    chunks_by_source: &[(String, Vec<Chunk>)],
    requested_source_ids: Option<&[String]>,
    max_sources: usize,
    max_snippets: usize,
) -> Result<(StudioSourceScope, Vec<StudioSnippet>), GlossError> {
    let requested = requested_source_ids.unwrap_or(&[]);
    let requested_set = requested.iter().cloned().collect::<HashSet<_>>();
    let selected_sources = sources
        .iter()
        .filter(|source| {
            if requested.is_empty() {
                source.selected
            } else {
                requested_set.contains(&source.id)
            }
        })
        .collect::<Vec<_>>();
    let eligible_sources = if requested.is_empty() && selected_sources.is_empty() {
        sources.iter().collect::<Vec<_>>()
    } else {
        selected_sources
    };

    let effective_sources = eligible_sources
        .into_iter()
        .filter(|source| source.status == "ready")
        .take(max_sources.max(1))
        .collect::<Vec<_>>();

    if !requested.is_empty() {
        let effective_set = effective_sources
            .iter()
            .map(|source| source.id.as_str())
            .collect::<HashSet<_>>();
        let missing = requested
            .iter()
            .filter(|id| !sources.iter().any(|source| &source.id == *id))
            .cloned()
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            return Err(GlossError::Studio {
                output_type: "source_scope".to_string(),
                message: format!("requested source ids not found: {}", missing.join(", ")),
            });
        }
        let not_ready = requested
            .iter()
            .filter(|id| !effective_set.contains(id.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if !not_ready.is_empty() {
            // Log which sources are still processing but don't hard-fail —
            // proceed with the ready subset instead.
            tracing::info!(
                "[studio] {} of {} requested source(s) not yet ready (skipping): {:?}",
                not_ready.len(),
                requested.len(),
                not_ready,
            );
        }
    }

    let mut snippets = Vec::new();
    for source in &effective_sources {
        let chunks = chunks_by_source
            .iter()
            .find(|(source_id, _)| source_id == &source.id)
            .map(|(_, chunks)| chunks.as_slice())
            .unwrap_or(&[]);
        if chunks.is_empty() {
            if let Some(text) = source.content_text.as_deref() {
                push_snippet(&mut snippets, source, None, text, max_snippets);
            }
        } else {
            for chunk in chunks.iter().take(2) {
                push_snippet(
                    &mut snippets,
                    source,
                    Some(chunk.id.clone()),
                    &chunk.content,
                    max_snippets,
                );
            }
        }
        if snippets.len() >= max_snippets.max(1) {
            break;
        }
    }

    if snippets.is_empty() {
        let eligible_count = sources.iter().filter(|s| s.status == "ready").count();
        let total_count = sources.len();
        return Err(GlossError::Studio {
            output_type: "source_scope".to_string(),
            message: if total_count == 0 {
                "Add sources to this notebook first, then generate Studio outputs.".to_string()
            } else if eligible_count == 0 {
                format!(
                    "None of the {total_count} source(s) are ready yet. \
                     Wait for processing to finish or check for errors."
                )
            } else {
                "No ready source snippets available for Studio output.".to_string()
            },
        });
    }

    let effective_source_ids = effective_sources
        .iter()
        .map(|source| source.id.clone())
        .collect::<Vec<_>>();
    let scope_kind = if requested.is_empty() {
        if sources.iter().any(|source| source.selected) {
            "selected"
        } else {
            "all_ready"
        }
    } else {
        "explicit"
    };

    Ok((
        StudioSourceScope {
            scope_kind: scope_kind.to_string(),
            requested_source_ids: requested.to_vec(),
            source_count: effective_source_ids.len(),
            snippet_count: snippets.len(),
            effective_source_ids,
        },
        snippets,
    ))
}

fn push_snippet(
    snippets: &mut Vec<StudioSnippet>,
    source: &Source,
    chunk_id: Option<String>,
    text: &str,
    max_snippets: usize,
) {
    if snippets.len() >= max_snippets.max(1) {
        return;
    }
    let excerpt = normalize_excerpt(text, 360);
    if excerpt.is_empty() {
        return;
    }
    snippets.push(StudioSnippet {
        source_id: source.id.clone(),
        source_title: source.title.clone(),
        chunk_id,
        text: excerpt,
    });
}

fn normalize_excerpt(text: &str, max_chars: usize) -> String {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= max_chars {
        compact
    } else {
        let mut out = compact.chars().take(max_chars).collect::<String>();
        out.push_str("...");
        out
    }
}

pub fn generate_artifact(
    kind: StudioOutputKind,
    title: Option<String>,
    source_scope: StudioSourceScope,
    snippets: &[StudioSnippet],
) -> Result<StudioArtifact, GlossError> {
    if snippets.is_empty() {
        return Err(GlossError::Studio {
            output_type: kind.as_str().to_string(),
            message: "cannot generate Studio artifact without source snippets".to_string(),
        });
    }
    let title = title.unwrap_or_else(|| default_title(kind));
    let content = match kind {
        StudioOutputKind::Report => report_content(snippets),
        StudioOutputKind::Summary => summary_content(snippets),
        StudioOutputKind::Outline => outline_content(snippets),
        StudioOutputKind::Faq => faq_content(snippets),
        StudioOutputKind::Flashcards => flashcards_content(snippets),
        StudioOutputKind::Quiz => quiz_content(snippets),
        StudioOutputKind::MindMap => mind_map_content(&title, snippets),
        StudioOutputKind::Timeline => timeline_content(snippets),
        StudioOutputKind::CompareTable => compare_table_content(snippets),
        StudioOutputKind::ActionPlan => action_plan_content(snippets),
    };
    let mut artifact = StudioArtifact {
        schema: STUDIO_ARTIFACT_SCHEMA.to_string(),
        receipt_schema: STUDIO_RECEIPT_SCHEMA.to_string(),
        receipt_id: format!("studio-{}", uuid::Uuid::new_v4()),
        output_type: kind.as_str().to_string(),
        title,
        prompt_used: "deterministic_source_bound_template_v1".to_string(),
        source_scope,
        content,
        validation: StudioValidation {
            schema_validated: false,
            all_items_source_cited: false,
            deterministic: true,
            errors: Vec::new(),
        },
    };
    artifact.validation = validate_artifact(&artifact);
    if !artifact.validation.schema_validated || !artifact.validation.all_items_source_cited {
        return Err(GlossError::Studio {
            output_type: kind.as_str().to_string(),
            message: artifact.validation.errors.join("; "),
        });
    }
    Ok(artifact)
}

fn default_title(kind: StudioOutputKind) -> String {
    match kind {
        StudioOutputKind::Report => "Source-grounded report",
        StudioOutputKind::Summary => "Source-grounded summary",
        StudioOutputKind::Outline => "Source-grounded outline",
        StudioOutputKind::Faq => "Source-grounded FAQ",
        StudioOutputKind::Flashcards => "Source-grounded flashcards",
        StudioOutputKind::Quiz => "Source-grounded quiz",
        StudioOutputKind::MindMap => "Source-grounded mind map",
        StudioOutputKind::Timeline => "Source-grounded timeline",
        StudioOutputKind::CompareTable => "Source-grounded compare table",
        StudioOutputKind::ActionPlan => "Source-grounded action plan",
    }
    .to_string()
}

fn cited_item(snippet: &StudioSnippet, fields: Value) -> Value {
    let mut map = fields.as_object().cloned().unwrap_or_default();
    map.insert("citations".to_string(), json!([snippet.citation()]));
    Value::Object(map)
}

fn report_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "sections": snippets.iter().enumerate().map(|(index, snippet)| {
            cited_item(snippet, json!({
                "heading": format!("Finding {}", index + 1),
                "claim": snippet.text,
                "source_title": snippet.source_title,
            }))
        }).collect::<Vec<_>>()
    })
}

fn summary_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "bullets": snippets.iter().map(|snippet| {
            cited_item(snippet, json!({
                "text": snippet.text,
                "source_title": snippet.source_title,
            }))
        }).collect::<Vec<_>>()
    })
}

fn outline_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "nodes": snippets.iter().enumerate().map(|(index, snippet)| {
            cited_item(snippet, json!({
                "level": 1,
                "label": format!("{}. {}", index + 1, snippet.source_title),
                "detail": snippet.text,
            }))
        }).collect::<Vec<_>>()
    })
}

fn faq_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "questions": snippets.iter().map(|snippet| {
            cited_item(snippet, json!({
                "question": format!("What does {} say?", snippet.source_title),
                "answer": snippet.text,
            }))
        }).collect::<Vec<_>>()
    })
}

fn flashcards_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "cards": snippets.iter().map(|snippet| {
            cited_item(snippet, json!({
                "front": format!("Recall the key point from {}", snippet.source_title),
                "back": snippet.text,
            }))
        }).collect::<Vec<_>>()
    })
}

fn quiz_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "questions": snippets.iter().map(|snippet| {
            cited_item(snippet, json!({
                "question": format!("Which statement is supported by {}?", snippet.source_title),
                "choices": [snippet.text.as_str(), "Not supported by the selected sources", "Contradicted by the selected sources"],
                "answer_index": 0,
            }))
        }).collect::<Vec<_>>()
    })
}

fn mind_map_content(title: &str, snippets: &[StudioSnippet]) -> Value {
    json!({
        "center": title,
        "branches": snippets.iter().map(|snippet| {
            cited_item(snippet, json!({
                "label": snippet.source_title,
                "children": [snippet.text.as_str()],
            }))
        }).collect::<Vec<_>>()
    })
}

fn timeline_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "entries": snippets.iter().enumerate().map(|(index, snippet)| {
            cited_item(snippet, json!({
                "sequence": index + 1,
                "label": snippet.source_title,
                "event": snippet.text,
            }))
        }).collect::<Vec<_>>()
    })
}

fn compare_table_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "columns": ["source", "supported_point"],
        "rows": snippets.iter().map(|snippet| {
            cited_item(snippet, json!({
                "source": snippet.source_title,
                "supported_point": snippet.text,
            }))
        }).collect::<Vec<_>>()
    })
}

fn action_plan_content(snippets: &[StudioSnippet]) -> Value {
    json!({
        "actions": snippets.iter().enumerate().map(|(index, snippet)| {
            cited_item(snippet, json!({
                "step": index + 1,
                "action": format!("Review source-grounded point from {}", snippet.source_title),
                "rationale": snippet.text,
            }))
        }).collect::<Vec<_>>()
    })
}

pub fn validate_artifact(artifact: &StudioArtifact) -> StudioValidation {
    let mut errors = Vec::new();
    if artifact.schema != STUDIO_ARTIFACT_SCHEMA {
        errors.push("schema mismatch".to_string());
    }
    if artifact.receipt_schema != STUDIO_RECEIPT_SCHEMA {
        errors.push("receipt schema mismatch".to_string());
    }
    if artifact.source_scope.snippet_count == 0
        || artifact.source_scope.effective_source_ids.is_empty()
    {
        errors.push("artifact has no effective source scope".to_string());
    }
    let cited = content_items_have_citations(&artifact.content);
    if !cited {
        errors.push("one or more Studio content items are missing citations".to_string());
    }
    StudioValidation {
        schema_validated: errors.is_empty(),
        all_items_source_cited: cited,
        deterministic: artifact.prompt_used == "deterministic_source_bound_template_v1",
        errors,
    }
}

fn content_items_have_citations(content: &Value) -> bool {
    let arrays = match content.as_object() {
        Some(map) => map
            .iter()
            // Mind-map "edges" are relations between already-cited nodes,
            // not claims of their own — they carry no citations.
            .filter(|(key, _)| key.as_str() != "edges")
            .filter_map(|(_, value)| value.as_array())
            .filter(|items| items.iter().all(Value::is_object))
            .collect::<Vec<_>>(),
        None => return false,
    };
    if arrays.is_empty() {
        return false;
    }
    arrays.iter().all(|items| {
        !items.is_empty()
            && items.iter().all(|item| {
                item.get("citations")
                    .and_then(Value::as_array)
                    .is_some_and(|citations| !citations.is_empty())
            })
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(id: &str, title: &str, selected: bool) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: title.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: Some(format!("{title} body text with a source-grounded claim.")),
            word_count: Some(8),
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected,
            created_at: "now".to_string(),
            updated_at: "now".to_string(),
            processing_state: None,
        }
    }

    fn chunk(source_id: &str, id: &str, content: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            source_id: source_id.to_string(),
            chunk_index: 0,
            content: content.to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        }
    }

    #[test]
    fn studio_artifacts_cover_required_output_types_with_citations() {
        let sources = vec![source("s1", "Alpha", true), source("s2", "Beta", false)];
        let chunks = vec![(
            "s1".to_string(),
            vec![chunk(
                "s1",
                "c1",
                "Alpha says local-first source evidence matters.",
            )],
        )];
        let (scope, snippets) = build_snippets(&sources, &chunks, None, 4, 8).unwrap();
        assert_eq!(scope.scope_kind, "selected");
        assert_eq!(snippets.len(), 1);

        for kind in [
            StudioOutputKind::Report,
            StudioOutputKind::Summary,
            StudioOutputKind::Outline,
            StudioOutputKind::Faq,
            StudioOutputKind::Flashcards,
            StudioOutputKind::Quiz,
            StudioOutputKind::MindMap,
            StudioOutputKind::Timeline,
            StudioOutputKind::CompareTable,
            StudioOutputKind::ActionPlan,
        ] {
            let artifact = generate_artifact(kind, None, scope.clone(), &snippets).unwrap();
            assert_eq!(artifact.schema, STUDIO_ARTIFACT_SCHEMA);
            assert!(artifact.validation.schema_validated, "{kind:?}");
            assert!(artifact.validation.all_items_source_cited, "{kind:?}");
            assert!(artifact
                .content
                .to_string()
                .contains("\"source_id\":\"s1\""));
        }
    }

    #[test]
    fn explicit_studio_scope_rejects_missing_source_ids() {
        let sources = vec![source("s1", "Alpha", true)];
        let err = build_snippets(&sources, &[], Some(&["missing".to_string()]), 4, 8)
            .expect_err("missing explicit source id must fail");
        assert!(err.to_string().contains("requested source ids not found"));
    }

    #[test]
    fn explicit_studio_scope_skips_not_ready_sources_instead_of_failing() {
        // Two sources: one ready, one still processing.  Explicitly requesting both
        // should succeed using the ready one rather than erroring out.
        let ready_src = source("s1", "Ready", true);
        let mut processing_src = source("s2", "Processing", true);
        processing_src.status = "processing".to_string();

        let sources = vec![ready_src, processing_src];
        let result = build_snippets(
            &sources,
            &[],
            Some(&["s1".to_string(), "s2".to_string()]),
            4,
            8,
        );
        // Should succeed by using the ready source, not error on the processing one.
        assert!(
            result.is_ok(),
            "expected Ok with partial ready sources, got Err: {:?}",
            result
        );
        let (scope, snippets) = result.unwrap();
        assert_eq!(scope.effective_source_ids, vec!["s1"]);
        assert!(!snippets.is_empty());
    }
}
