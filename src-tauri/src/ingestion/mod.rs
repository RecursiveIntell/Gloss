pub mod chunk;
pub mod embed;
pub mod extract;
pub mod summarize;

use crate::db::notebook_db::{Chunk, NotebookDb, Source};
use crate::error::GlossError;
use crate::providers::LlmProvider;
use std::path::Path;

/// Run the full ingestion pipeline for a source: extract → chunk → embed → summarize.
pub async fn ingest_source(
    nb_db: &NotebookDb,
    source: &Source,
    embedder: &embed::EmbeddingService,
    index: &mut embed::HnswIndex,
    provider: Option<&dyn LlmProvider>,
    model: &str,
    notebook_dir: &Path,
) -> Result<(), GlossError> {
    let source_id = &source.id;

    // 1. Extract
    nb_db.update_source_status(source_id, "extracting", None)?;
    let content = extract::extract_text(source, notebook_dir)?;
    let word_count = content.split_whitespace().count() as i32;
    nb_db.update_source_content(source_id, &content, word_count)?;

    // 2. Chunk
    nb_db.update_source_status(source_id, "chunking", None)?;
    let chunks = chunk::chunk_text(&content, source_id);

    // 3. Store chunks and embed
    nb_db.update_source_status(source_id, "embedding", None)?;
    let chunk_texts: Vec<&str> = chunks.iter().map(|c| c.content.as_str()).collect();
    let embeddings = embedder.embed_batch(&chunk_texts)?;

    for (i, chunk_data) in chunks.iter().enumerate() {
        let chunk = Chunk {
            id: chunk_data.id.clone(),
            source_id: source_id.to_string(),
            chunk_index: chunk_data.chunk_index,
            content: chunk_data.content.clone(),
            token_count: chunk_data.token_count,
            start_offset: chunk_data.start_offset,
            end_offset: chunk_data.end_offset,
            metadata: chunk_data.metadata.clone(),
            embedding_id: None,
            embedding_model: Some("NomicEmbedTextV15".to_string()),
        };
        let _rowid = nb_db.insert_chunk(&chunk)?;

        // Add to HNSW index
        if let Some(embedding) = embeddings.get(i) {
            let label = index.add(embedding)?;
            nb_db.update_chunk_embedding(&chunk.id, label as i64, "NomicEmbedTextV15")?;
        }
    }

    // 4. Summarize (if provider available)
    if let Some(provider) = provider {
        nb_db.update_source_status(source_id, "summarizing", None)?;
        match summarize::summarize_source(&content, &source.title, provider, model).await {
            Ok(summary) => {
                nb_db.update_source_summary(source_id, &summary, model)?;
            }
            Err(e) => {
                tracing::warn!(source_id, "Summarization failed: {}", e);
            }
        }
    }

    nb_db.update_source_status(source_id, "ready", None)?;
    Ok(())
}
