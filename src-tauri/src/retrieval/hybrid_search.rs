use crate::db::notebook_db::{Chunk, NotebookDb};
use crate::error::GlossError;
use crate::ingestion::embed::{EmbeddingService, HnswIndex};
use std::collections::HashMap;

/// A search result with relevance score.
#[derive(Debug, Clone)]
pub struct SearchResult {
    pub chunk: Chunk,
    pub score: f64,
}

/// Compute dynamic top-K based on notebook source count.
pub fn compute_top_k(source_count: usize) -> usize {
    match source_count {
        0..=5 => 4,
        6..=20 => 6,
        21..=50 => 8,
        51..=100 => 10,
        _ => 12,
    }
}

/// Perform hybrid search: HNSW semantic + FTS5 keyword, fused with RRF.
pub fn hybrid_search(
    query: &str,
    nb_db: &NotebookDb,
    embedder: &EmbeddingService,
    index: &HnswIndex,
    selected_source_ids: &[String],
    top_k: usize,
) -> Result<Vec<SearchResult>, GlossError> {
    // Scale the pre-rerank pool proportionally to top_k
    let k_per_source = (top_k * 5).max(20);

    // 1. Semantic search via HNSW
    let query_embedding = embedder.embed_one(query)?;
    let hnsw_results = index.search(&query_embedding, k_per_source)?;

    let mut semantic_chunks: Vec<(Chunk, usize)> = Vec::new();
    for (rank, (label, _distance)) in hnsw_results.iter().enumerate() {
        match nb_db.get_chunk_by_embedding_id(*label as i64) {
            Ok(chunk) => {
                if selected_source_ids.is_empty() || selected_source_ids.contains(&chunk.source_id)
                {
                    semantic_chunks.push((chunk, rank));
                }
            }
            Err(_) => continue,
        }
    }

    // 2. Keyword search via FTS5
    // Escape FTS5 special characters
    let fts_query = sanitize_fts_query(query);
    let fts_results = match nb_db.fts_search(&fts_query, k_per_source) {
        Ok(r) => r,
        Err(e) => {
            tracing::warn!("FTS search failed (non-fatal): {}", e);
            Vec::new()
        }
    };

    let mut keyword_chunks: Vec<(Chunk, usize)> = Vec::new();
    for (rank, (rowid, _score)) in fts_results.iter().enumerate() {
        match nb_db.get_chunk_by_rowid(*rowid) {
            Ok(chunk) => {
                if selected_source_ids.is_empty() || selected_source_ids.contains(&chunk.source_id)
                {
                    keyword_chunks.push((chunk, rank));
                }
            }
            Err(_) => continue,
        }
    }

    // 3. Reciprocal Rank Fusion (RRF)
    let rrf_k = 60.0;
    let mut scores: HashMap<String, (f64, Chunk)> = HashMap::new();

    for (chunk, rank) in &semantic_chunks {
        let rrf_score = 1.0 / (rrf_k + *rank as f64);
        let entry = scores
            .entry(chunk.id.clone())
            .or_insert((0.0, chunk.clone()));
        entry.0 += rrf_score;
    }

    for (chunk, rank) in &keyword_chunks {
        let rrf_score = 1.0 / (rrf_k + *rank as f64);
        let entry = scores
            .entry(chunk.id.clone())
            .or_insert((0.0, chunk.clone()));
        entry.0 += rrf_score;
    }

    // Sort by RRF score descending
    let mut results: Vec<SearchResult> = scores
        .into_values()
        .map(|(score, chunk)| SearchResult { chunk, score })
        .collect();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // 4. Rerank: take top-N candidates from RRF, rerank with cross-encoder
    let rerank_pool_size = (top_k * 5).min(30).min(results.len());
    if rerank_pool_size > 0 && embedder.has_reranker() {
        let candidates: Vec<SearchResult> = results.drain(..rerank_pool_size).collect();
        let documents: Vec<String> = candidates.iter().map(|r| r.chunk.content.clone()).collect();

        tracing::debug!(
            pool_size = rerank_pool_size,
            top_k,
            "Reranking RRF candidates with cross-encoder"
        );

        match embedder.rerank(query, &documents, top_k) {
            Ok(reranked) => {
                results = reranked
                    .into_iter()
                    .filter_map(|(orig_idx, score)| {
                        candidates.get(orig_idx).map(|c| SearchResult {
                            chunk: c.chunk.clone(),
                            score: score as f64,
                        })
                    })
                    .collect();
            }
            Err(e) => {
                tracing::warn!("Reranking failed (falling back to RRF order): {}", e);
                // Put candidates back and truncate
                results = candidates;
                results.truncate(top_k);
            }
        }
    } else {
        results.truncate(top_k);
    }

    Ok(results)
}

/// Sanitize a query string for FTS5 MATCH syntax.
fn sanitize_fts_query(query: &str) -> String {
    // Split into words and join with spaces (implicit AND in FTS5)
    query
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| {
            // Remove FTS5 operators and special characters
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}
