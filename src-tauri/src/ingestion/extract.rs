use crate::db::notebook_db::Source;
use crate::error::GlossError;
use std::path::Path;

/// Extract text content from a source based on its type.
/// Phase 1 supports: text, markdown, paste.
pub fn extract_text(source: &Source, notebook_dir: &Path) -> Result<String, GlossError> {
    match source.source_type.as_str() {
        "text" | "markdown" | "code" => {
            // Read from file (code files are treated as UTF-8 text)
            if let Some(ref file_path) = source.file_path {
                let full_path = notebook_dir.join("sources").join(file_path);
                std::fs::read_to_string(&full_path).map_err(|e| GlossError::Ingestion {
                    source_id: source.id.clone(),
                    message: format!("Failed to read file: {}", e),
                })
            } else if let Some(ref content) = source.content_text {
                Ok(content.clone())
            } else {
                Err(GlossError::Ingestion {
                    source_id: source.id.clone(),
                    message: "No file_path or content_text for text source".into(),
                })
            }
        }
        "paste" => {
            // Paste sources have content_text set directly
            source
                .content_text
                .clone()
                .ok_or_else(|| GlossError::Ingestion {
                    source_id: source.id.clone(),
                    message: "No content_text for paste source".into(),
                })
        }
        "image" => {
            // Images cannot be extracted as text yet — requires vision model
            Ok(format!("[Image file: {}]", source.title))
        }
        "video" => {
            // Videos cannot be extracted as text yet — requires processing pipeline
            Ok(format!("[Video file: {}]", source.title))
        }
        _ => Err(GlossError::Ingestion {
            source_id: source.id.clone(),
            message: format!("Unsupported source type: {}", source.source_type),
        }),
    }
}
