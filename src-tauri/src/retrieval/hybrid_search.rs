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
///
/// Sized so that, combined with per-source diversity selection, every scoped
/// source with a relevant candidate can contribute at least one passage while
/// staying inside the chat context budget (~32K chars).
///
/// `DENSE_HARD_OVERFETCH_CAP_MULTIPLIER` (16) caps the dense HNSW overfetch
/// regardless of source count. Without this cap, with 100 sources and
/// top_k=24, the formula `top_k * 5 * sources` = 12,000 would walk the
/// entire HNSW index and do a single-row SQL per neighbor. 16x is generous
/// for chat latency while still preserving per-source diversity. (See
/// hostile audit B3.)
const DENSE_HARD_OVERFETCH_CAP_MULTIPLIER: usize = 16;

pub fn compute_top_k(source_count: usize) -> usize {
    match source_count {
        0..=5 => 8,
        6..=20 => 12,
        21..=50 => 16,
        51..=100 => 20,
        _ => 24,
    }
}

fn dense_scope_overfetch_limit(top_k: usize, coverage: &RetrievalCoverage) -> usize {
    let base = (top_k * 5).max(20);
    let hard_cap = top_k.saturating_mul(DENSE_HARD_OVERFETCH_CAP_MULTIPLIER);
    let source_scaled = base.saturating_mul(coverage.selected_sources.max(1));
    let computed_limit = coverage
        .embedded_chunks
        .min(source_scaled.max(base))
        .max(base.min(coverage.embedded_chunks.max(1)));
    computed_limit.min(hard_cap)
}

#[cfg(test)]
fn collect_scoped_dense_chunks_from_ranked_labels(
    ranked_labels: &[(i64, f32)],
    nb_db: &NotebookDb,
    scope: &ResolvedSourceScope,
    top_k: usize,
) -> Vec<(Chunk, usize)> {
    let mut dense_chunks = Vec::new();
    for (rank, (label, _distance)) in ranked_labels.iter().enumerate() {
        if let Ok(chunk) = nb_db.get_chunk_by_embedding_id(*label) {
            if scope_allows_chunk(scope, &chunk) {
                dense_chunks.push((chunk, rank));
                if dense_chunks.len() >= top_k {
                    break;
                }
            }
        }
    }
    dense_chunks
}

fn scope_allows_chunk(scope: &ResolvedSourceScope, chunk: &Chunk) -> bool {
    scope.allows(&chunk.source_id)
}

/// Convenience wrapper for tests: same signature as
/// `local_retrieval_outcome_with_query` minus the precomputed query
/// embedding, which lets tests assert on the auto-embed path.
#[cfg(test)]
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
    local_retrieval_outcome_with_query(
        query,
        nb_db,
        embedder,
        index,
        None,
        None,
        native_indexing_enabled,
        scope,
        top_k,
        trace_ref,
    )
}

/// Variant of `local_retrieval_outcome` that accepts a precomputed query
/// embedding (e.g. from the LRU cache in `AppState::get_or_embed_query`).
/// When `query_embedding` is `Some`, the dense path uses it directly and
/// skips the `embed_one` call. When `None`, the function falls back to
/// calling `embedder.embed_one(query)` as before.
#[allow(clippy::too_many_arguments)]
pub fn local_retrieval_outcome_with_query(
    query: &str,
    nb_db: &NotebookDb,
    embedder: Option<&EmbeddingService>,
    index: Option<&HnswIndex>,
    query_embedding: Option<&[f32]>,
    dense_block_reason: Option<RetrievalReasonCode>,
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
        let scoped_source_ids: Vec<&str> = scope.source_ids().iter().map(String::as_str).collect();
        match nb_db.fts_search_chunks_in_sources_batched(
            &fts_query,
            &scoped_source_ids,
            k_per_source,
        ) {
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

    if let Some(reason) = dense_block_reason {
        dense_reason = Some(reason.clone());
        fallback_chain.push(reason);
    } else if !native_indexing_enabled {
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
                // Prefer the precomputed (cached) embedding if provided;
                // fall back to embed_one if not. This is the B4 win:
                // repeated identical queries no longer re-hit the embedder.
                let query_embedding_vec: Vec<f32> = match query_embedding {
                    Some(v) => v.to_vec(),
                    None => embedder.embed_one(query)?,
                };
                let dense_limit = dense_scope_overfetch_limit(top_k, &coverage);
                let hnsw_results = index.search(&query_embedding_vec, dense_limit)?;
                for (rank, (label, _distance)) in hnsw_results.iter().enumerate() {
                    if let Ok(chunk) = nb_db.get_chunk_by_embedding_id(*label as i64) {
                        if scope_allows_chunk(scope, &chunk) {
                            dense_chunks.push((chunk, rank));
                        }
                    }
                }
                if dense_chunks.is_empty() {
                    dense_reason = Some(RetrievalReasonCode::DenseNoQueryMatches);
                    fallback_chain.push(RetrievalReasonCode::DenseNoQueryMatches);
                }
            }
        }
    }
    let dense_elapsed = dense_started.elapsed().as_millis();

    // Fuse into an overfetched candidate pool, optionally rerank with the
    // cross-encoder, then apply per-source diversity so a single verbose
    // source cannot crowd every other relevant source out of the context.
    let candidate_pool = top_k.saturating_mul(3);
    let mut fused = rrf_fuse(query, &dense_chunks, &bm25_chunks, candidate_pool);
    if let Some(embedder) = embedder {
        if embedder.has_reranker() && !fused.is_empty() {
            let documents: Vec<String> = fused
                .iter()
                .map(|result| result.chunk.content.clone())
                .collect();
            match embedder.rerank(query, &documents, documents.len()) {
                Ok(reranked) => {
                    let mut out = Vec::with_capacity(fused.len());
                    for (orig_idx, score) in reranked {
                        if let Some(candidate) = fused.get(orig_idx) {
                            out.push(SearchResult {
                                chunk: candidate.chunk.clone(),
                                score: score as f64,
                            });
                        }
                    }
                    if !out.is_empty() {
                        fused = out;
                    }
                }
                Err(err) => {
                    tracing::warn!("Cross-encoder rerank failed (keeping RRF order): {err}");
                }
            }
        }
    }
    let fused = select_with_source_diversity(fused, top_k);
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
    query: &str,
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
        .map(|(score, chunk)| SearchResult {
            score: score + local_intent_rerank_boost(query, &chunk.content),
            chunk,
        })
        .collect::<Vec<_>>();
    results.sort_by(compare_results_desc);
    results.truncate(top_k);
    results
}

fn compare_results_desc(a: &SearchResult, b: &SearchResult) -> std::cmp::Ordering {
    b.score
        .partial_cmp(&a.score)
        .unwrap_or(std::cmp::Ordering::Equal)
        .then_with(|| a.chunk.source_id.cmp(&b.chunk.source_id))
        .then_with(|| a.chunk.chunk_index.cmp(&b.chunk.chunk_index))
        .then_with(|| a.chunk.id.cmp(&b.chunk.id))
}

/// Select the final top-K from a score-ordered candidate pool while
/// guaranteeing per-source representation: the best chunk of each distinct
/// source is admitted first (in score order), remaining slots are filled with
/// the best leftover candidates, and the final set is re-sorted by score.
fn select_with_source_diversity(candidates: Vec<SearchResult>, top_k: usize) -> Vec<SearchResult> {
    if candidates.len() <= top_k {
        return candidates;
    }
    let mut first_per_source = Vec::new();
    let mut deferred = Vec::new();
    let mut seen_sources = std::collections::HashSet::new();
    for candidate in candidates {
        if seen_sources.insert(candidate.chunk.source_id.clone()) {
            first_per_source.push(candidate);
        } else {
            deferred.push(candidate);
        }
    }
    first_per_source.truncate(top_k);
    let remaining = top_k - first_per_source.len();
    first_per_source.extend(deferred.into_iter().take(remaining));
    first_per_source.sort_by(compare_results_desc);
    first_per_source
}

pub(crate) fn local_intent_rerank_boost(query: &str, content: &str) -> f64 {
    let query_terms = normalized_terms(query);
    if query_terms.is_empty() {
        return 0.0;
    }
    let content_terms = normalized_terms(content);
    let overlap = query_terms
        .iter()
        .filter(|term| content_terms.iter().any(|candidate| candidate == *term))
        .count() as f64;
    let overlap_boost = (overlap * 0.002).min(0.012);
    let action_boost = if is_action_or_improvement_query(&query_terms)
        && content_terms
            .iter()
            .any(|term| is_actionable_content_marker(term))
    {
        0.025
    } else {
        0.0
    };
    overlap_boost + action_boost
}

fn normalized_terms(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|word| {
            word.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                .collect::<String>()
                .to_ascii_lowercase()
        })
        .filter(|word| !word.is_empty() && !is_query_stopword(word))
        .collect()
}

fn is_action_or_improvement_query(terms: &[String]) -> bool {
    terms.iter().any(|term| {
        matches!(
            term.as_str(),
            "action"
                | "actionable"
                | "act"
                | "do"
                | "fix"
                | "improve"
                | "improvement"
                | "implement"
                | "next"
                | "recommend"
                | "recommendation"
                | "should"
                | "todo"
        )
    })
}

fn is_actionable_content_marker(term: &str) -> bool {
    matches!(
        term,
        "action"
            | "actionable"
            | "blocker"
            | "fix"
            | "implement"
            | "must"
            | "next"
            | "plan"
            | "priority"
            | "recommend"
            | "recommended"
            | "should"
            | "step"
            | "todo"
            | "verify"
    )
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
    let mut expanded_terms = Vec::new();
    for term in selected_terms {
        if !expanded_terms.contains(&term) {
            expanded_terms.push(term.clone());
        }
        for expanded in action_intent_expansions(&term) {
            let expanded = expanded.to_string();
            if !expanded_terms.contains(&expanded) {
                expanded_terms.push(expanded);
            }
        }
    }
    expanded_terms
        .into_iter()
        .map(|term| format!("\"{}\"", term))
        .collect::<Vec<_>>()
        .join(" OR ")
}

fn action_intent_expansions(term: &str) -> &'static [&'static str] {
    match term.to_ascii_lowercase().as_str() {
        "improve" | "improvement" => &[
            "action",
            "actionable",
            "fix",
            "implement",
            "improve",
            "improvement",
            "next",
            "plan",
            "recommend",
        ],
        "should" | "do" | "next" | "recommend" | "recommendation" | "action" | "actionable" => &[
            "action",
            "actionable",
            "fix",
            "implement",
            "next",
            "plan",
            "recommend",
        ],
        _ => &[],
    }
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
        let fused = rrf_fuse("needle", &dense, &bm25, 10);
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
    fn stale_embedding_index_metadata_blocks_dense_with_typed_reason() {
        let dir = tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let selected = source("selected");
        db.insert_source(&selected).unwrap();
        let mut record = chunk("c1", "selected");
        record.content = "needle lexical content".to_string();
        record.embedding_id = Some(7);
        record.embedding_model = Some("old-384d".to_string());
        db.insert_chunk(&record).unwrap();
        let scope = SourceScope::Explicit(vec!["selected".to_string()]).resolve(&[selected]);

        let outcome = local_retrieval_outcome_with_query(
            "needle",
            &db,
            None,
            None,
            None,
            Some(RetrievalReasonCode::EmbeddingIndexMetadataStale),
            true,
            &scope,
            4,
            "trace-stale-index".to_string(),
        )
        .unwrap();

        assert_eq!(outcome.mode, RetrievalMode::Bm25Only);
        assert!(outcome
            .fallback_chain
            .contains(&RetrievalReasonCode::EmbeddingIndexMetadataStale));
        let dense = outcome
            .engines
            .iter()
            .find(|engine| engine.engine == "native_dense_hnsw")
            .unwrap();
        assert!(!dense.attempted);
        assert_eq!(
            dense.reason_code,
            Some(RetrievalReasonCode::EmbeddingIndexMetadataStale)
        );
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
    fn actionable_later_source_beats_first_source_noise_for_improvement_query() {
        let dir = tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let first = source("aaa-first");
        let later = source("zzz-actionable");
        db.insert_source(&first).unwrap();
        db.insert_source(&later).unwrap();

        for idx in 0..25 {
            let mut record = chunk(&format!("first-{idx}"), "aaa-first");
            record.chunk_index = idx;
            record.content = format!(
                "General improve background note {idx}. This is descriptive historical context only."
            );
            db.insert_chunk(&record).unwrap();
        }
        let mut actionable = chunk("later-action", "zzz-actionable");
        actionable.content = "Actionable improvement plan: fix retrieval ranking, implement fair source candidate pooling, and verify with a regression test.".to_string();
        db.insert_chunk(&actionable).unwrap();

        let scope = SourceScope::All.resolve(&[first, later]);
        let outcome = local_retrieval_outcome(
            "How should I improve this?",
            &db,
            None,
            None,
            false,
            &scope,
            4,
            "trace-actionable-later".to_string(),
        )
        .unwrap();

        assert_eq!(outcome.mode, RetrievalMode::Bm25Only);
        assert_eq!(outcome.results[0].source_id, "zzz-actionable");
        assert!(outcome.results[0]
            .content
            .contains("Actionable improvement plan"));
    }

    #[test]
    fn dense_scope_overfetch_scales_by_selected_sources_to_avoid_global_crowding() {
        let coverage = RetrievalCoverage {
            selected_sources: 3,
            total_chunks: 90,
            fts_indexed_chunks: 0,
            embedded_chunks: 90,
            missing_embeddings: 0,
            semantic_links_total: 0,
            semantic_links_healthy: 0,
            semantic_links_degraded: 0,
            dense_coverage_ratio: 1.0,
        };

        assert_eq!(dense_scope_overfetch_limit(4, &coverage), 60);
    }

    #[test]
    fn dense_scope_overfetch_limit_is_capped_by_source_count() {
        let coverage = RetrievalCoverage {
            selected_sources: 100,
            total_chunks: 20_000,
            fts_indexed_chunks: 0,
            embedded_chunks: 10_000,
            missing_embeddings: 0,
            semantic_links_total: 0,
            semantic_links_healthy: 0,
            semantic_links_degraded: 0,
            dense_coverage_ratio: 1.0,
        };

        // Without the cap, the formula would give top_k*5*sources = 12000.
        // With the new cap of DENSE_HARD_OVERFETCH_CAP_MULTIPLIER=16, the
        // result is min(12000, 24*16) = 384. (See hostile audit B3.)
        assert_eq!(dense_scope_overfetch_limit(24, &coverage), 384);
    }

    #[test]
    fn scoped_dense_collection_recovers_selected_hit_after_out_of_scope_prefix() {
        let dir = tempdir().unwrap();
        let db = NotebookDb::open(&dir.path().join("notebook.db")).unwrap();
        let first = source("aaa-first");
        let selected = source("selected-answer");
        db.insert_source(&first).unwrap();
        db.insert_source(&selected).unwrap();

        let mut ranked = Vec::new();
        for idx in 0..20 {
            let mut record = chunk(&format!("first-{idx}"), "aaa-first");
            record.embedding_id = Some(idx + 1);
            db.insert_chunk(&record).unwrap();
            ranked.push((idx + 1, 0.01));
        }
        let mut answer = chunk("selected-answer-chunk", "selected-answer");
        answer.embedding_id = Some(21);
        answer.content = "Actionable selected dense answer".to_string();
        db.insert_chunk(&answer).unwrap();
        ranked.push((21, 0.02));

        let scope =
            SourceScope::Explicit(vec!["selected-answer".to_string()]).resolve(&[first, selected]);
        let collected = collect_scoped_dense_chunks_from_ranked_labels(&ranked, &db, &scope, 4);

        assert_eq!(collected.len(), 1);
        assert_eq!(collected[0].0.source_id, "selected-answer");
    }

    #[test]
    fn source_diversity_guarantees_each_source_a_slot_before_filling_by_score() {
        let mut candidates = Vec::new();
        // Source A dominates the score-ordered pool with 5 chunks…
        for idx in 0..5 {
            candidates.push(SearchResult {
                chunk: {
                    let mut record = chunk(&format!("a{idx}"), "source-a");
                    record.chunk_index = idx;
                    record
                },
                score: 1.0 - idx as f64 * 0.01,
            });
        }
        // …while sources B and C each have one weaker but relevant chunk.
        candidates.push(SearchResult {
            chunk: chunk("b0", "source-b"),
            score: 0.5,
        });
        candidates.push(SearchResult {
            chunk: chunk("c0", "source-c"),
            score: 0.4,
        });

        let selected = select_with_source_diversity(candidates, 4);
        let sources = selected
            .iter()
            .map(|result| result.chunk.source_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(selected.len(), 4);
        assert!(sources.contains(&"source-b"), "{sources:?}");
        assert!(sources.contains(&"source-c"), "{sources:?}");
        // Order remains score-descending after diversity selection.
        assert_eq!(selected[0].chunk.source_id, "source-a");
        assert!(selected.windows(2).all(|w| w[0].score >= w[1].score));
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
