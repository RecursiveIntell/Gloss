use crate::db::notebook_db::{Chunk, NotebookDb};
use crate::error::GlossError;
use crate::ingestion::embed::{EmbeddingService, HnswIndex};
use crate::memory::types::{
    RetrievalCoverage, RetrievalEngineStatus, RetrievalMode, RetrievalOutcome, RetrievalReasonCode,
    RetrievalResult,
};
use crate::retrieval::source_scope::ResolvedSourceScope;
use std::collections::HashMap;
use std::time::Instant;

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

fn scope_allows_chunk(scope: &ResolvedSourceScope, chunk: &Chunk) -> bool {
    scope.allows(&chunk.source_id)
}

#[allow(clippy::too_many_arguments)]
pub fn local_retrieval_outcome(
    query: &str,
    nb_db: &NotebookDb,
    embedder: Option<&EmbeddingService>,
    index: Option<&HnswIndex>,
    native_indexing_enabled: bool,
    scope: &ResolvedSourceScope,
    top_k: usize,
    trace_ref: String,
) -> Result<RetrievalOutcome, GlossError> {
    let coverage = nb_db.retrieval_coverage(scope)?;
    if scope.is_none() {
        return Ok(unavailable_outcome(
            coverage,
            trace_ref,
            RetrievalReasonCode::NoRetrievalContext,
            "No retrieval context because source scope resolved to none",
        ));
    }

    let k_per_source = (top_k * 5).max(20);
    let mut fallback_chain = Vec::new();
    if coverage.embedded_chunks == 0 {
        fallback_chain.push(RetrievalReasonCode::NoEmbeddedChunks);
    } else if coverage.missing_embeddings > 0 {
        fallback_chain.push(RetrievalReasonCode::PartialEmbeddingCoverage);
        fallback_chain.push(RetrievalReasonCode::ScopeHasMissingEmbeddings);
    }

    let bm25_started = Instant::now();
    let fts_query = sanitize_fts_query(query);
    let mut bm25_chunks: Vec<(Chunk, usize)> = Vec::new();
    let mut bm25_reason = None;
    if fts_query.is_empty() {
        bm25_reason = Some(RetrievalReasonCode::Bm25QuerySanitizedEmpty);
        fallback_chain.push(RetrievalReasonCode::Bm25QuerySanitizedEmpty);
    } else {
        match nb_db.fts_search_chunks_in_sources(&fts_query, scope, k_per_source) {
            Ok(results) => {
                bm25_chunks = results
                    .into_iter()
                    .enumerate()
                    .map(|(rank, (chunk, _score))| (chunk, rank))
                    .collect();
                if bm25_chunks.is_empty() {
                    bm25_reason = Some(RetrievalReasonCode::Bm25NoMatches);
                    fallback_chain.push(RetrievalReasonCode::Bm25NoMatches);
                }
            }
            Err(err) => {
                bm25_reason = Some(RetrievalReasonCode::IndexMissing);
                fallback_chain.push(RetrievalReasonCode::IndexMissing);
                tracing::warn!("FTS5/BM25 retrieval failed: {}", err);
            }
        }
    }
    let bm25_elapsed = bm25_started.elapsed().as_millis();

    let dense_started = Instant::now();
    let mut dense_chunks: Vec<(Chunk, usize)> = Vec::new();
    let mut dense_attempted = false;
    let mut dense_available = false;
    let mut dense_reason = None;

    if !native_indexing_enabled {
        dense_reason = Some(RetrievalReasonCode::NativeIndexingDisabled);
        fallback_chain.push(RetrievalReasonCode::NativeIndexingDisabled);
    } else if coverage.embedded_chunks == 0 {
        dense_reason = Some(RetrievalReasonCode::NoEmbeddedChunks);
    } else {
        match (embedder, index) {
            (None, _) => {
                dense_reason = Some(RetrievalReasonCode::EmbedderUnavailable);
                fallback_chain.push(RetrievalReasonCode::EmbedderUnavailable);
            }
            (_, None) => {
                dense_reason = Some(RetrievalReasonCode::IndexMissing);
                fallback_chain.push(RetrievalReasonCode::IndexMissing);
            }
            (Some(embedder), Some(index)) => {
                dense_attempted = true;
                dense_available = true;
                let query_embedding = embedder.embed_one(query)?;
                let hnsw_results = index.search(&query_embedding, k_per_source)?;
                for (rank, (label, _distance)) in hnsw_results.iter().enumerate() {
                    if let Ok(chunk) = nb_db.get_chunk_by_embedding_id(*label as i64) {
                        if scope_allows_chunk(scope, &chunk) {
                            dense_chunks.push((chunk, rank));
                        }
                    }
                }
                if dense_chunks.is_empty() {
                    dense_reason = Some(RetrievalReasonCode::DenseEngineUnavailable);
                    fallback_chain.push(RetrievalReasonCode::DenseEngineUnavailable);
                }
            }
        }
    }
    let dense_elapsed = dense_started.elapsed().as_millis();

    let fused = rrf_fuse(&dense_chunks, &bm25_chunks, top_k);
    let mode = match (!dense_chunks.is_empty(), !bm25_chunks.is_empty()) {
        (true, true) => RetrievalMode::HybridRrf,
        (true, false) => RetrievalMode::DenseOnly,
        (false, true) => RetrievalMode::Bm25Only,
        (false, false) => RetrievalMode::Unavailable,
    };
    let results = fused
        .into_iter()
        .map(|result| RetrievalResult {
            chunk_id: Some(result.chunk.id.clone()),
            source_id: result.chunk.source_id.clone(),
            title: None,
            content: result.chunk.content,
            score: result.score,
            engine: mode.as_str().to_string(),
        })
        .collect::<Vec<_>>();

    let mut degraded = !fallback_chain.is_empty();
    if mode == RetrievalMode::Unavailable {
        degraded = true;
        if !fallback_chain
            .iter()
            .any(|reason| reason == &RetrievalReasonCode::NoRetrievalContext)
        {
            fallback_chain.push(RetrievalReasonCode::NoRetrievalContext);
        }
    }

    let summary = user_visible_summary(&mode, &coverage, &fallback_chain);
    Ok(RetrievalOutcome {
        mode,
        results,
        engines: vec![
            RetrievalEngineStatus {
                engine: "bm25_fts5".to_string(),
                attempted: !fts_query.is_empty(),
                available: true,
                contributed: !bm25_chunks.is_empty(),
                candidate_count: bm25_chunks.len(),
                elapsed_ms: bm25_elapsed,
                reason_code: bm25_reason,
                detail: Some("Stable local sparse retriever".to_string()),
            },
            RetrievalEngineStatus {
                engine: "native_dense_hnsw".to_string(),
                attempted: dense_attempted,
                available: dense_available,
                contributed: !dense_chunks.is_empty(),
                candidate_count: dense_chunks.len(),
                elapsed_ms: dense_elapsed,
                reason_code: dense_reason,
                detail: Some("Optional native dense retriever".to_string()),
            },
        ],
        coverage,
        degraded,
        fallback_chain: dedupe_reason_codes(fallback_chain),
        user_visible_summary: summary,
        trace_ref,
    })
}

fn unavailable_outcome(
    coverage: RetrievalCoverage,
    trace_ref: String,
    reason: RetrievalReasonCode,
    summary: &str,
) -> RetrievalOutcome {
    RetrievalOutcome {
        mode: RetrievalMode::Unavailable,
        results: Vec::new(),
        engines: Vec::new(),
        coverage,
        degraded: true,
        fallback_chain: vec![reason],
        user_visible_summary: summary.to_string(),
        trace_ref,
    }
}

fn rrf_fuse(
    dense_chunks: &[(Chunk, usize)],
    bm25_chunks: &[(Chunk, usize)],
    top_k: usize,
) -> Vec<SearchResult> {
    let rrf_k = 60.0;
    let mut scores: HashMap<String, (f64, Chunk)> = HashMap::new();
    for (chunk, rank) in dense_chunks {
        let entry = scores
            .entry(chunk.id.clone())
            .or_insert((0.0, chunk.clone()));
        entry.0 += 1.0 / (rrf_k + *rank as f64);
    }
    for (chunk, rank) in bm25_chunks {
        let entry = scores
            .entry(chunk.id.clone())
            .or_insert((0.0, chunk.clone()));
        entry.0 += 1.0 / (rrf_k + *rank as f64);
    }
    let mut results = scores
        .into_values()
        .map(|(score, chunk)| SearchResult { chunk, score })
        .collect::<Vec<_>>();
    results.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    results.truncate(top_k);
    results
}

fn dedupe_reason_codes(reasons: Vec<RetrievalReasonCode>) -> Vec<RetrievalReasonCode> {
    let mut out = Vec::new();
    for reason in reasons {
        if !out.contains(&reason) {
            out.push(reason);
        }
    }
    out
}

fn user_visible_summary(
    mode: &RetrievalMode,
    coverage: &RetrievalCoverage,
    fallback_chain: &[RetrievalReasonCode],
) -> String {
    let reasons = fallback_chain
        .iter()
        .map(RetrievalReasonCode::as_str)
        .collect::<Vec<_>>()
        .join(", ");
    match mode {
        RetrievalMode::HybridRrf => format!(
            "Hybrid retrieval used BM25 plus dense candidates. Dense coverage: {:.0}%.",
            coverage.dense_coverage_ratio * 100.0
        ),
        RetrievalMode::Bm25Only => {
            if reasons.is_empty() {
                "BM25/FTS5 retrieval used as the stable local retriever.".to_string()
            } else {
                format!("BM25/FTS5 retrieval used. Dense retrieval did not contribute: {reasons}.")
            }
        }
        RetrievalMode::DenseOnly => {
            format!("Dense retrieval used; BM25 did not contribute: {reasons}.")
        }
        RetrievalMode::Unavailable => {
            format!("No indexed retrieval candidates were available: {reasons}.")
        }
        RetrievalMode::SemanticMemory => "semantic-memory preview retrieval used.".to_string(),
        RetrievalMode::SourceOrderFallback => {
            format!("Source-order fallback used after indexed retrieval degraded: {reasons}.")
        }
        RetrievalMode::RawContentFallback => {
            format!("Raw source content fallback used after indexed retrieval degraded: {reasons}.")
        }
    }
}

/// Perform hybrid search: HNSW semantic + FTS5 keyword, fused with RRF.
#[allow(dead_code)]
pub fn hybrid_search(
    query: &str,
    nb_db: &NotebookDb,
    embedder: &EmbeddingService,
    index: &HnswIndex,
    scope: &ResolvedSourceScope,
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
                if scope_allows_chunk(scope, &chunk) {
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
                if scope_allows_chunk(scope, &chunk) {
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
pub fn sanitize_fts_query(query: &str) -> String {
    let terms = query
        .split_whitespace()
        .filter(|w| !w.is_empty())
        .map(|w| {
            // Remove FTS5 operators and special characters
            w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect::<String>()
        })
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>();
    let content_terms = terms
        .iter()
        .filter(|term| !is_query_stopword(term))
        .cloned()
        .collect::<Vec<_>>();
    let selected_terms = if content_terms.is_empty() {
        terms
    } else {
        content_terms
    };
    selected_terms
        .into_iter()
        .map(|term| format!("\"{}\"", term))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn is_query_stopword(term: &str) -> bool {
    matches!(
        term.to_ascii_lowercase().as_str(),
        "a" | "an"
            | "and"
            | "are"
            | "as"
            | "cite"
            | "does"
            | "for"
            | "from"
            | "in"
            | "is"
            | "it"
            | "of"
            | "on"
            | "or"
            | "source"
            | "sources"
            | "the"
            | "this"
            | "to"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "with"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::notebook_db::Source;
    use crate::retrieval::source_scope::SourceScope;
    use tempfile::tempdir;

    fn source(id: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: id.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: None,
            word_count: None,
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
            processing_state: None,
        }
    }

    fn chunk(id: &str, source_id: &str) -> Chunk {
        Chunk {
            id: id.to_string(),
            source_id: source_id.to_string(),
            chunk_index: 0,
            content: "content".to_string(),
            token_count: None,
            start_offset: None,
            end_offset: None,
            metadata: None,
            embedding_id: None,
            embedding_model: None,
        }
    }

    #[test]
    fn shared_scope_filter_excludes_unselected_semantic_and_fts_chunks() {
        let all_sources = vec![source("selected"), source("unselected")];
        let scope = SourceScope::Explicit(vec!["selected".to_string()]).resolve(&all_sources);

        assert!(scope_allows_chunk(&scope, &chunk("c1", "selected")));
        assert!(!scope_allows_chunk(&scope, &chunk("c2", "unselected")));
    }

    #[test]
    fn invalid_explicit_scope_filters_every_chunk() {
        let all_sources = vec![source("selected")];
        let scope = SourceScope::Explicit(vec!["missing".to_string()]).resolve(&all_sources);

        assert!(!scope_allows_chunk(&scope, &chunk("c1", "selected")));
    }

    #[test]
    fn partial_coverage_is_degraded_but_not_disqualifying() {
        let coverage = RetrievalCoverage {
            selected_sources: 1,
            total_chunks: 10,
            fts_indexed_chunks: 10,
            embedded_chunks: 4,
            missing_embeddings: 6,
            semantic_links_total: 0,
            semantic_links_healthy: 0,
            semantic_links_degraded: 0,
            dense_coverage_ratio: 0.4,
        };
        let summary = user_visible_summary(
            &RetrievalMode::HybridRrf,
            &coverage,
            &[
                RetrievalReasonCode::PartialEmbeddingCoverage,
                RetrievalReasonCode::ScopeHasMissingEmbeddings,
            ],
        );
        assert!(summary.contains("Hybrid retrieval"));
        assert_eq!(coverage.dense_coverage_ratio, 0.4);
    }

    #[test]
    fn rrf_fuses_dense_and_bm25_without_requiring_identical_sets() {
        let dense = vec![(chunk("dense-only", "selected"), 0)];
        let bm25 = vec![(chunk("bm25-only", "selected"), 0)];
        let fused = rrf_fuse(&dense, &bm25, 10);
        let ids = fused
            .iter()
            .map(|result| result.chunk.id.as_str())
            .collect::<Vec<_>>();
        assert!(ids.contains(&"dense-only"));
        assert!(ids.contains(&"bm25-only"));
    }

    #[test]
    fn bm25_outcome_records_partial_dense_coverage_without_silent_fallback() {
        let dir = tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let selected = source("selected");
        db.insert_source(&selected).unwrap();
        for idx in 0..10 {
            let mut record = chunk(&format!("c{idx}"), "selected");
            record.chunk_index = idx;
            record.content = format!("needle retrieval content {idx}");
            if idx < 4 {
                record.embedding_id = Some(idx as i64 + 1);
                record.embedding_model = Some("test-embedder".to_string());
            }
            db.insert_chunk(&record).unwrap();
        }
        let scope = SourceScope::Explicit(vec!["selected".to_string()]).resolve(&[selected]);

        let outcome = local_retrieval_outcome(
            "needle",
            &db,
            None,
            None,
            false,
            &scope,
            4,
            "trace-partial".to_string(),
        )
        .unwrap();

        assert_eq!(outcome.mode, RetrievalMode::Bm25Only);
        assert!(outcome.degraded);
        assert_eq!(outcome.coverage.total_chunks, 10);
        assert_eq!(outcome.coverage.embedded_chunks, 4);
        assert_eq!(outcome.coverage.missing_embeddings, 6);
        assert_eq!(outcome.coverage.dense_coverage_ratio, 0.4);
        assert!(outcome
            .fallback_chain
            .contains(&RetrievalReasonCode::PartialEmbeddingCoverage));
        assert!(outcome
            .fallback_chain
            .contains(&RetrievalReasonCode::NativeIndexingDisabled));

        let serialized = serde_json::to_value(&outcome).unwrap();
        assert_eq!(serialized["mode"], "bm25_only");
        assert!(serialized["fallback_chain"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|value| value.as_str())
            .collect::<Vec<_>>()
            .contains(&"partial_embedding_coverage"));
    }

    #[test]
    fn natural_question_query_matches_answer_terms_without_fallback() {
        let dir = tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let selected = source("selected");
        db.insert_source(&selected).unwrap();
        let mut record = chunk("c1", "selected");
        record.content =
            "The P33 desktop smoke answer is ORCHID-913. Cite this source as [1].".to_string();
        db.insert_chunk(&record).unwrap();
        let scope = SourceScope::Explicit(vec!["selected".to_string()]).resolve(&[selected]);

        let outcome = local_retrieval_outcome(
            "What is the P33 desktop smoke answer? Cite the source.",
            &db,
            None,
            None,
            false,
            &scope,
            4,
            "trace-natural-question".to_string(),
        )
        .unwrap();

        assert_eq!(outcome.mode, RetrievalMode::Bm25Only);
        assert_eq!(outcome.results.len(), 1);
        assert!(outcome.results[0].content.contains("ORCHID-913"));
    }

    #[test]
    fn zero_embedded_chunks_reports_no_embedded_chunks_reason() {
        let dir = tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let selected = source("selected");
        db.insert_source(&selected).unwrap();
        let mut record = chunk("c1", "selected");
        record.content = "needle lexical content".to_string();
        db.insert_chunk(&record).unwrap();
        let scope = SourceScope::Explicit(vec!["selected".to_string()]).resolve(&[selected]);

        let outcome = local_retrieval_outcome(
            "needle",
            &db,
            None,
            None,
            true,
            &scope,
            4,
            "trace-zero".to_string(),
        )
        .unwrap();

        assert_eq!(outcome.mode, RetrievalMode::Bm25Only);
        assert_eq!(outcome.coverage.embedded_chunks, 0);
        assert!(outcome
            .fallback_chain
            .contains(&RetrievalReasonCode::NoEmbeddedChunks));
    }
}
