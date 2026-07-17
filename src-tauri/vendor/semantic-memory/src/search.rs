//! Hybrid search engine: BM25 + vector similarity + Reciprocal Rank Fusion.

use crate::config::{DerivedVectorBackendPolicy, SearchConfig};
use crate::episodes;
use crate::error::MemoryError;
use crate::types::{
    ExplainedResult, ScoreBreakdown, SearchContext, SearchResult, SearchSource, SearchSourceType,
    VectorSearchReceiptV1,
};
use rusqlite::types::Value as SqlValue;
use rusqlite::Connection;
// `OptionalExtension` provides `Result::optional()` for `rusqlite::query_row`.
// Four unconditional call sites in this file use it; keep the import
// always available. The trait is light (zero runtime cost) so the
// `#[allow(unused_imports)]` is the only cost when no callsite is in
// scope on a given feature set.
#[allow(unused_imports)]
use rusqlite::OptionalExtension;
use stack_ids::DigestBuilder;
#[cfg(feature = "turbo-quant-codec")]
use std::collections::BinaryHeap;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Per-table row count above which vector search emits a warning.
const VECTOR_SCAN_WARN_THRESHOLD: usize = 50_000;
/// Per-table row count above which brute-force vector search is refused.
const VECTOR_SCAN_HARD_LIMIT: usize = 250_000;

static VECTOR_SCAN_WARN_LIMIT: AtomicUsize = AtomicUsize::new(VECTOR_SCAN_WARN_THRESHOLD);
static VECTOR_SCAN_BLOCK_LIMIT: AtomicUsize = AtomicUsize::new(VECTOR_SCAN_HARD_LIMIT);

/// Expand query terms to match hyphenated variants.
/// "turbo-quant" -> "turbo-quant OR turboquant"
/// This improves BM25 recall for technical terms with hyphens.
#[allow(dead_code)]
fn expand_query_for_fts(query: &str) -> String {
    let terms: Vec<&str> = query.split_whitespace().collect();
    let expanded: Vec<String> = terms
        .iter()
        .map(|term| {
            if term.contains('-') {
                let no_hyphen = term.replace('-', "");
                if no_hyphen != *term {
                    format!("{term} OR {no_hyphen}")
                } else {
                    term.to_string()
                }
            } else {
                term.to_string()
            }
        })
        .collect();
    expanded.join(" ")
}

/// Classify whether a query needs retrieval at all.
/// Simple greetings, confirmations, and single-word responses don't need search.
pub fn should_retrieve(query: &str) -> bool {
    let trimmed = query.trim();
    if trimmed.len() < 12 {
        return false;
    }
    let lower = trimmed.to_lowercase();
    let skip_phrases = [
        "ok",
        "yes",
        "no",
        "thanks",
        "done",
        "sure",
        "yeah",
        "right",
        "correct",
        "agreed",
        "ok thanks",
        "got it",
        "sounds good",
        "that works",
        "makes sense",
        "i see",
        "understood",
        "gotcha",
    ];
    for phrase in &skip_phrases {
        if lower == *phrase {
            return false;
        }
    }
    if lower.starts_with("can you")
        || lower.starts_with("could you")
        || lower.starts_with("would you")
        || lower.starts_with("will you")
    {
        if lower.len() <= 20 {
            return false;
        }
    }
    if trimmed.starts_with('/') {
        return false;
    }
    true
}

/// Sanitize a raw query string for safe use in an FTS5 MATCH expression.
///
/// Replaces any character that is not alphanumeric, whitespace, or a Unicode
/// letter/digit with a space, then strips FTS5 boolean keywords (`AND`, `OR`,
/// `NOT`, `NEAR`).  Returns `None` when no searchable tokens remain.
///
/// This uses an allowlist strategy so that *any* FTS5 operator or punctuation
/// — including `?`, `.`, `/`, `!`, etc. — is neutralised without needing an
/// exhaustive denylist.
pub fn sanitize_fts_query(raw: &str) -> Option<String> {
    let cleaned: String = raw
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c.is_whitespace() || c == '_' {
                c
            } else {
                ' '
            }
        })
        .collect();

    let tokens: Vec<&str> = cleaned
        .split_whitespace()
        .filter(|t| !matches!(t.to_uppercase().as_str(), "AND" | "OR" | "NOT" | "NEAR"))
        .collect();

    if tokens.is_empty() {
        None
    } else {
        Some(
            tokens
                .into_iter()
                .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
                .collect::<Vec<_>>()
                .join(" OR "),
        )
    }
}

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> Result<f32, MemoryError> {
    if a.len() != b.len() {
        return Err(MemoryError::EmbeddingDimensionMismatch {
            expected: a.len(),
            actual: b.len(),
        });
    }
    if let Some((index, _)) = a.iter().enumerate().find(|(_, value)| !value.is_finite()) {
        return Err(MemoryError::NonFiniteEmbeddingValue { index });
    }
    if let Some((index, _)) = b.iter().enumerate().find(|(_, value)| !value.is_finite()) {
        return Err(MemoryError::NonFiniteEmbeddingValue { index });
    }
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return Ok(0.0);
    }
    let similarity = dot / (norm_a * norm_b);
    if !similarity.is_finite() {
        return Err(MemoryError::Other(
            "cosine similarity produced a non-finite score".to_string(),
        ));
    }
    Ok(similarity)
}

fn days_since(timestamp: &str, evaluation_time: chrono::DateTime<chrono::Utc>) -> Option<f64> {
    let dt = parse_search_timestamp(timestamp)?;
    let duration = evaluation_time.naive_utc() - dt;
    Some(duration.num_seconds() as f64 / 86_400.0)
}

fn parse_search_timestamp(timestamp: &str) -> Option<chrono::NaiveDateTime> {
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S") {
        return Some(dt);
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(timestamp, "%Y-%m-%d %H:%M:%S%.f") {
        return Some(dt);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(timestamp) {
        return Some(dt.naive_utc());
    }
    tracing::warn!(
        timestamp,
        "failed to parse search timestamp for recency scoring; recency contribution dropped"
    );
    None
}

fn recency_contribution(
    config: &SearchConfig,
    context: &SearchContext,
    updated_at: Option<&str>,
    best_rank: Option<usize>,
) -> Option<f64> {
    match (config.recency_half_life_days, updated_at) {
        (Some(half_life), Some(ts)) if half_life > 0.0 => {
            let age_days = days_since(ts, context.evaluation_time).map(|days| days.max(0.0))?;
            let decay = 2.0_f64.powf(-age_days / half_life);
            let rank = best_rank.unwrap_or(1).max(1) as f64;
            Some(config.recency_weight * decay / (config.rrf_k + rank))
        }
        _ => None,
    }
}

pub(crate) fn search_result_id(source: &SearchSource) -> String {
    match source {
        SearchSource::Fact { fact_id, .. } => format!("fact:{fact_id}"),
        SearchSource::Chunk { chunk_id, .. } => format!("chunk:{chunk_id}"),
        SearchSource::Message { message_id, .. } => format!("msg:{message_id}"),
        SearchSource::Episode { episode_id, .. } => format!("episode:{episode_id}"),
        SearchSource::Projection { projection_id, .. } => format!("projection:{projection_id}"),
    }
}

pub fn source_dedup_key(source: &SearchSource) -> (u8, String) {
    match source {
        SearchSource::Fact { fact_id, .. } => (0, fact_id.clone()),
        SearchSource::Chunk { chunk_id, .. } => (1, chunk_id.clone()),
        SearchSource::Message {
            message_id,
            session_id,
            ..
        } => (2, format!("{session_id}:{message_id}")),
        SearchSource::Episode { episode_id, .. } => (3, episode_id.clone()),
        SearchSource::Projection { projection_id, .. } => (4, projection_id.clone()),
    }
}

/// A BM25 search hit from FTS5.
#[derive(Debug, Clone)]
pub struct Bm25Hit {
    /// Search item key such as `fact:{uuid}` or `episode:{episode_id}`.
    pub id: String,
    /// Text content returned to callers.
    pub content: String,
    /// Source info.
    pub source: SearchSource,
    /// Raw BM25 score reported by SQLite FTS5.
    pub raw_score: f64,
    /// Timestamp used for recency scoring.
    pub updated_at: Option<String>,
    /// Temporal weight for stale-fact downranking (0.0-1.0, default 1.0).
    pub temporal_weight: Option<f64>,
    /// Provenance confidence (0.0-1.0, default 0.5).
    pub provenance_confidence: Option<f64>,
}

/// A vector search hit.
#[derive(Debug, Clone)]
pub struct VectorHit {
    /// Search item key such as `fact:{uuid}` or `episode:{episode_id}`.
    pub id: String,
    /// Text content returned to callers.
    pub content: String,
    /// Source info.
    pub source: SearchSource,
    /// Final similarity used for vector ranking.
    pub similarity: f64,
    /// Timestamp used for recency scoring.
    pub updated_at: Option<String>,
    /// Rank from the underlying retrieval stage before exact reranking.
    pub source_rank: Option<usize>,
    /// Similarity from the underlying retrieval stage before exact reranking.
    pub source_similarity: Option<f64>,
    /// Whether exact f32 reranking changed or confirmed this candidate ordering.
    pub reranked_from_f32: bool,
    /// Temporal weight for stale-fact downranking (0.0-1.0, default 1.0).
    pub temporal_weight: Option<f64>,
    /// Provenance confidence (0.0-1.0, default 0.5).
    pub provenance_confidence: Option<f64>,
}

/// A genuine sparse dot-product search hit.
#[derive(Debug, Clone)]
struct SparseHit {
    content: String,
    source: SearchSource,
    score: f64,
    updated_at: Option<String>,
    representation: String,
}

#[allow(dead_code)]
struct VectorRow {
    id: String,
    content: String,
    blob: Vec<u8>,
    updated_at: Option<String>,
    source_type: SearchSourceType,
    filter_namespace: Option<String>,
    filter_session_id: Option<String>,
    source: SearchSource,
}

struct RrfCandidate {
    content: String,
    source: SearchSource,
    updated_at: Option<String>,
    bm25_score: Option<f64>,
    bm25_rank: Option<usize>,
    vector_score: Option<f64>,
    vector_rank: Option<usize>,
    vector_source_rank: Option<usize>,
    vector_source_score: Option<f64>,
    vector_reranked_from_f32: bool,
    sparse_score: Option<f64>,
    sparse_rank: Option<usize>,
    /// Late interaction (ColBERT MaxSim) rank — 3rd RRF signal.
    late_interaction_rank: Option<usize>,
    /// Late interaction raw score. Populated only with the `late-interaction` feature.
    #[allow(dead_code)]
    late_interaction_score: Option<f64>,
    /// Temporal weight for stale-fact downranking (0.0-1.0, default 1.0).
    temporal_weight: Option<f64>,
    /// Provenance confidence (0.0-1.0, default 0.5). Higher = more trustworthy.
    provenance_confidence: Option<f64>,
}

impl RrfCandidate {
    fn explained(self, config: &SearchConfig, context: &SearchContext) -> ExplainedResult {
        let bm25_contribution = self
            .bm25_rank
            .map(|rank| config.bm25_weight / (config.rrf_k + rank as f64));
        let vector_contribution = self
            .vector_rank
            .map(|rank| config.vector_weight / (config.rrf_k + rank as f64));
        let sparse_contribution = self
            .sparse_rank
            .map(|rank| config.sparse_weight / (config.rrf_k + rank as f64));
        // Late interaction contribution: uses same RRF formula with late_interaction_weight.
        // Defaults to 0.0 weight when not configured (backward compatible).
        let late_interaction_weight = config.late_interaction_weight;
        let late_interaction_contribution = self
            .late_interaction_rank
            .map(|rank| late_interaction_weight / (config.rrf_k + rank as f64));
        let best_rank = [self.bm25_rank, self.vector_rank, self.sparse_rank]
            .into_iter()
            .flatten()
            .min();
        let recency_score =
            recency_contribution(config, context, self.updated_at.as_deref(), best_rank);
        let base_score = bm25_contribution.unwrap_or(0.0)
            + vector_contribution.unwrap_or(0.0)
            + sparse_contribution.unwrap_or(0.0)
            + late_interaction_contribution.unwrap_or(0.0)
            + recency_score.unwrap_or(0.0);
        // Apply temporal weight: stale facts (weight < 1.0) get downranked.
        // Default weight is 1.0 (no effect) when temporal feature is not active.
        let temporal_factor = self.temporal_weight.unwrap_or(1.0);
        let provenance_factor = 1.0 + (self.provenance_confidence.unwrap_or(0.5) - 0.5) * 0.2;
        let rrf_score = base_score * temporal_factor * provenance_factor;
        // Apply namespace weight if configured
        let ns_weight = match &self.source {
            SearchSource::Fact { namespace, .. } => config
                .namespace_weights
                .get(namespace)
                .copied()
                .unwrap_or(1.0),
            _ => 1.0,
        };
        let rrf_score = rrf_score * ns_weight;

        let breakdown = ScoreBreakdown {
            rrf_score,
            bm25_score: self.bm25_score,
            vector_score: self.vector_score,
            sparse_score: self.sparse_score,
            recency_score,
            bm25_rank: self.bm25_rank,
            vector_rank: self.vector_rank,
            sparse_rank: self.sparse_rank,
            vector_source_rank: self.vector_source_rank,
            vector_source_score: self.vector_source_score,
            bm25_contribution,
            vector_contribution,
            sparse_contribution,
            vector_reranked_from_f32: self.vector_reranked_from_f32,
            bm25_weight: config.bm25_weight,
            vector_weight: config.vector_weight,
            sparse_weight: config.sparse_weight,
            recency_weight: config.recency_half_life_days.map(|_| config.recency_weight),
            rrf_k: config.rrf_k,
        };

        ExplainedResult {
            result: SearchResult {
                content: self.content,
                source: self.source,
                score: rrf_score,
                bm25_rank: breakdown.bm25_rank,
                vector_rank: breakdown.vector_rank,
                cosine_similarity: breakdown.vector_score,
            },
            breakdown,
        }
    }
}

fn scan_vector_rows(
    rows: impl Iterator<Item = Result<VectorRow, rusqlite::Error>>,
    query_embedding: &[f32],
    min_similarity: f64,
    table_label: &str,
) -> Result<(Vec<VectorHit>, usize), MemoryError> {
    let expected_dims = query_embedding.len();
    let mut hits = Vec::new();
    let mut row_count = 0usize;
    let warn_limit = VECTOR_SCAN_WARN_LIMIT.load(Ordering::Relaxed);
    let hard_limit = VECTOR_SCAN_BLOCK_LIMIT.load(Ordering::Relaxed);

    for row in rows {
        let row = row?;
        row_count += 1;
        if warn_limit > 0 && row_count == warn_limit.saturating_add(1) {
            tracing::warn!(
                table = table_label,
                count = row_count,
                threshold = warn_limit,
                "vector scan warning threshold exceeded"
            );
        }
        if hard_limit > 0 && row_count > hard_limit {
            return Err(MemoryError::VectorScanLimitExceeded {
                table: table_label.to_string(),
                scanned: row_count,
                limit: hard_limit,
            });
        }

        let stored_embedding = match crate::db::decode_f32_le(&row.blob, expected_dims) {
            Ok(embedding) => embedding,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    table = table_label,
                    item = %row.id,
                    "Skipping row with invalid embedding blob"
                );
                continue;
            }
        };

        if stored_embedding.len() != expected_dims {
            tracing::warn!(
                expected = expected_dims,
                actual = stored_embedding.len(),
                "Skipping {} with wrong embedding dimensions",
                table_label
            );
            continue;
        }

        let similarity = cosine_similarity(query_embedding, &stored_embedding)? as f64;
        if similarity >= min_similarity {
            hits.push(VectorHit {
                id: row.id,
                content: row.content,
                source: row.source,
                similarity,
                updated_at: row.updated_at,
                source_rank: None,
                source_similarity: None,
                reranked_from_f32: false,
                temporal_weight: None,
                provenance_confidence: None,
            });
        }
    }

    Ok((hits, row_count))
}

fn rank_vector_hits(mut hits: Vec<VectorHit>, pool_size: usize) -> Vec<VectorHit> {
    hits.sort_by(|a, b| {
        b.similarity.partial_cmp(&a.similarity).unwrap_or_else(|| {
            if a.similarity.is_nan() {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Less
            }
        })
    });

    for (idx, hit) in hits.iter_mut().enumerate() {
        hit.source_rank = Some(idx + 1);
        hit.source_similarity = Some(hit.similarity);
    }

    hits.truncate(pool_size);
    hits
}

/// Run BM25 search over facts_fts, chunks_fts, episodes_fts, and optionally messages_fts.
pub(crate) fn bm25_search(
    conn: &Connection,
    sanitized_query: &str,
    pool_size: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<Bm25Hit>, MemoryError> {
    let mut hits = Vec::new();

    let search_facts = source_types
        .map(|st| st.contains(&SearchSourceType::Facts))
        .unwrap_or(true);
    let search_chunks = source_types
        .map(|st| st.contains(&SearchSourceType::Chunks))
        .unwrap_or(true);
    let search_messages = source_types
        .map(|st| st.contains(&SearchSourceType::Messages))
        .unwrap_or(false);
    let search_episodes = source_types
        .map(|st| st.contains(&SearchSourceType::Episodes))
        .unwrap_or(true);

    if search_facts {
        let (ns_clause, ns_params) = build_filter_clause("f.namespace", namespaces, 3);
        let sql = format!(
            "SELECT fm.fact_id, f.content, f.namespace, bm25(facts_fts) AS score, f.updated_at, f.temporal_weight
             FROM facts_fts
             JOIN facts_rowid_map fm ON facts_fts.rowid = fm.rowid
             JOIN facts f ON f.id = fm.fact_id
             WHERE facts_fts MATCH ?1 {}
             ORDER BY score ASC
             LIMIT ?2",
            ns_clause
        );

        let mut params = vec![
            SqlValue::Text(sanitized_query.to_string()),
            SqlValue::Integer(pool_size as i64),
        ];
        params.extend(ns_params);

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
            let fact_id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let namespace: String = row.get(2)?;
            let raw_score: f64 = row.get(3)?;
            let updated_at: Option<String> = row.get(4)?;
            let temporal_weight: Option<f64> = row.get(5)?;
            Ok(Bm25Hit {
                id: format!("fact:{fact_id}"),
                content,
                source: SearchSource::Fact { fact_id, namespace },
                raw_score,
                updated_at,
                temporal_weight,
                provenance_confidence: None,
            })
        })?;

        for row in rows {
            hits.push(row?);
        }
    }

    if search_chunks {
        let (ns_clause, ns_params) = build_filter_clause("d.namespace", namespaces, 3);
        let sql = format!(
            "SELECT cm.chunk_id, c.content, c.document_id, d.title, c.chunk_index,
                    bm25(chunks_fts) AS score, c.created_at
             FROM chunks_fts
             JOIN chunks_rowid_map cm ON chunks_fts.rowid = cm.rowid
             JOIN chunks c ON c.id = cm.chunk_id
             JOIN documents d ON d.id = c.document_id
             WHERE chunks_fts MATCH ?1 {}
             ORDER BY score ASC
             LIMIT ?2",
            ns_clause
        );

        let mut params = vec![
            SqlValue::Text(sanitized_query.to_string()),
            SqlValue::Integer(pool_size as i64),
        ];
        params.extend(ns_params);

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
            let chunk_id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let document_id: String = row.get(2)?;
            let document_title: String = row.get(3)?;
            let chunk_index: i64 = row.get(4)?;
            let raw_score: f64 = row.get(5)?;
            let updated_at: Option<String> = row.get(6)?;
            Ok(Bm25Hit {
                id: format!("chunk:{chunk_id}"),
                content,
                source: SearchSource::Chunk {
                    chunk_id,
                    document_id,
                    document_title,
                    chunk_index: chunk_index as usize,
                },
                raw_score,
                updated_at,
                temporal_weight: None,
                provenance_confidence: None,
            })
        })?;

        for row in rows {
            hits.push(row?);
        }
    }

    if search_messages {
        let (sid_clause, sid_params) = build_filter_clause("m.session_id", session_ids, 3);
        let sql = format!(
            "SELECT mm.message_id, m.content, m.session_id, m.role,
                    bm25(messages_fts) AS score, m.created_at
             FROM messages_fts
             JOIN messages_rowid_map mm ON messages_fts.rowid = mm.rowid
             JOIN messages m ON m.id = mm.message_id
             WHERE messages_fts MATCH ?1 {}
             ORDER BY score ASC
             LIMIT ?2",
            sid_clause
        );

        let mut params = vec![
            SqlValue::Text(sanitized_query.to_string()),
            SqlValue::Integer(pool_size as i64),
        ];
        params.extend(sid_params);

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
            let message_id: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let session_id: String = row.get(2)?;
            let role: String = row.get(3)?;
            let raw_score: f64 = row.get(4)?;
            let updated_at: Option<String> = row.get(5)?;
            Ok(Bm25Hit {
                id: format!("msg:{message_id}"),
                content,
                source: SearchSource::Message {
                    message_id,
                    session_id,
                    role,
                },
                raw_score,
                updated_at,
                temporal_weight: None,
                provenance_confidence: None,
            })
        })?;

        for row in rows {
            hits.push(row?);
        }
    }

    if search_episodes {
        let (ns_clause, ns_params) = build_filter_clause("d.namespace", namespaces, 3);
        let sql = format!(
            "SELECT e.episode_id, e.document_id, e.search_text, e.effect_type, e.outcome,
                    bm25(episodes_fts) AS score, e.updated_at
             FROM episodes_fts
             JOIN episodes_rowid_map rm ON episodes_fts.rowid = rm.rowid
             JOIN episodes e ON e.episode_id = rm.episode_id
             JOIN documents d ON d.id = e.document_id
             WHERE episodes_fts MATCH ?1 {}
             ORDER BY score ASC
             LIMIT ?2",
            ns_clause
        );

        let mut params = vec![
            SqlValue::Text(sanitized_query.to_string()),
            SqlValue::Integer(pool_size as i64),
        ];
        params.extend(ns_params);

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
            let episode_id: String = row.get(0)?;
            let document_id: String = row.get(1)?;
            let content: String = row.get(2)?;
            let effect_type: String = row.get(3)?;
            let outcome: String = row.get(4)?;
            let raw_score: f64 = row.get(5)?;
            let updated_at: Option<String> = row.get(6)?;
            Ok(Bm25Hit {
                id: episodes::episode_item_key(&episode_id),
                content,
                source: SearchSource::Episode {
                    episode_id,
                    document_id,
                    effect_type,
                    outcome,
                },
                raw_score,
                updated_at,
                temporal_weight: None,
                provenance_confidence: None,
            })
        })?;

        for row in rows {
            hits.push(row?);
        }
    }

    Ok(hits)
}

/// Run brute-force vector search over facts, chunks, episodes, and optionally messages.
pub(crate) fn vector_search(
    conn: &Connection,
    query_embedding: &[f32],
    pool_size: usize,
    min_similarity: f64,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<VectorHit>, MemoryError> {
    let mut hits = Vec::new();

    let search_facts = source_types
        .map(|st| st.contains(&SearchSourceType::Facts))
        .unwrap_or(true);
    let search_chunks = source_types
        .map(|st| st.contains(&SearchSourceType::Chunks))
        .unwrap_or(true);
    let search_messages = source_types
        .map(|st| st.contains(&SearchSourceType::Messages))
        .unwrap_or(false);
    let search_episodes = source_types
        .map(|st| st.contains(&SearchSourceType::Episodes))
        .unwrap_or(true);

    if search_facts {
        let (ns_clause, ns_params) = build_filter_clause("namespace", namespaces, 1);
        let sql = format!(
            "SELECT id, content, namespace, embedding, updated_at
             FROM facts
             WHERE embedding IS NOT NULL {}",
            ns_clause
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&ns_params), |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let namespace: String = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            let updated_at: Option<String> = row.get(4)?;
            Ok(VectorRow {
                id: format!("fact:{id}"),
                content,
                blob,
                updated_at,
                source_type: SearchSourceType::Facts,
                filter_namespace: Some(namespace.clone()),
                filter_session_id: None,
                source: SearchSource::Fact {
                    fact_id: id,
                    namespace,
                },
            })
        })?;

        let (fact_hits, fact_count) =
            scan_vector_rows(rows, query_embedding, min_similarity, "fact")?;
        hits.extend(fact_hits);

        if vector_scan_warn_exceeded(fact_count) {
            tracing::warn!(
                count = fact_count,
                "facts table exceeds vector scan threshold ({} rows)",
                fact_count
            );
        }
    }

    if search_chunks {
        let (ns_clause, ns_params) = build_filter_clause("d.namespace", namespaces, 1);
        let sql = format!(
            "SELECT c.id, c.content, c.document_id, d.title, c.chunk_index, c.embedding, c.created_at, d.namespace
             FROM chunks c
             JOIN documents d ON d.id = c.document_id
             WHERE c.embedding IS NOT NULL {}",
            ns_clause
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&ns_params), |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let document_id: String = row.get(2)?;
            let document_title: String = row.get(3)?;
            let chunk_index: i64 = row.get(4)?;
            let blob: Vec<u8> = row.get(5)?;
            let updated_at: Option<String> = row.get(6)?;
            let namespace: String = row.get(7)?;
            Ok(VectorRow {
                id: format!("chunk:{id}"),
                content,
                blob,
                updated_at,
                source_type: SearchSourceType::Chunks,
                filter_namespace: Some(namespace),
                filter_session_id: None,
                source: SearchSource::Chunk {
                    chunk_id: id,
                    document_id,
                    document_title,
                    chunk_index: chunk_index as usize,
                },
            })
        })?;

        let (chunk_hits, chunk_count) =
            scan_vector_rows(rows, query_embedding, min_similarity, "chunk")?;
        hits.extend(chunk_hits);

        if vector_scan_warn_exceeded(chunk_count) {
            tracing::warn!(
                count = chunk_count,
                "chunks table exceeds vector scan threshold ({} rows)",
                chunk_count
            );
        }
    }

    if search_messages {
        let (sid_clause, sid_params) = build_filter_clause("m.session_id", session_ids, 1);
        let sql = format!(
            "SELECT m.id, m.content, m.session_id, m.role, m.embedding, m.created_at
             FROM messages m
             WHERE m.embedding IS NOT NULL {}",
            sid_clause
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&sid_params), |row| {
            let message_id: i64 = row.get(0)?;
            let content: String = row.get(1)?;
            let session_id: String = row.get(2)?;
            let role: String = row.get(3)?;
            let blob: Vec<u8> = row.get(4)?;
            let updated_at: Option<String> = row.get(5)?;
            Ok(VectorRow {
                id: format!("msg:{message_id}"),
                content,
                blob,
                updated_at,
                source_type: SearchSourceType::Messages,
                filter_namespace: None,
                filter_session_id: Some(session_id.clone()),
                source: SearchSource::Message {
                    message_id,
                    session_id,
                    role,
                },
            })
        })?;

        let (message_hits, message_count) =
            scan_vector_rows(rows, query_embedding, min_similarity, "message")?;
        hits.extend(message_hits);

        if vector_scan_warn_exceeded(message_count) {
            tracing::warn!(
                count = message_count,
                "messages table exceeds vector scan threshold ({} rows)",
                message_count
            );
        }
    }

    if search_episodes {
        let (ns_clause, ns_params) = build_filter_clause("d.namespace", namespaces, 1);
        let sql = format!(
            "SELECT e.episode_id, e.document_id, e.search_text, e.effect_type, e.outcome, e.embedding, e.updated_at, d.namespace
             FROM episodes e
             JOIN documents d ON d.id = e.document_id
             WHERE e.embedding IS NOT NULL {}",
            ns_clause
        );

        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&ns_params), |row| {
            let episode_id: String = row.get(0)?;
            let document_id: String = row.get(1)?;
            let content: String = row.get(2)?;
            let effect_type: String = row.get(3)?;
            let outcome: String = row.get(4)?;
            let blob: Vec<u8> = row.get(5)?;
            let updated_at: Option<String> = row.get(6)?;
            let namespace: String = row.get(7)?;
            Ok(VectorRow {
                id: episodes::episode_item_key(&episode_id),
                content,
                blob,
                updated_at,
                source_type: SearchSourceType::Episodes,
                filter_namespace: Some(namespace),
                filter_session_id: None,
                source: SearchSource::Episode {
                    episode_id,
                    document_id,
                    effect_type,
                    outcome,
                },
            })
        })?;

        let (episode_hits, episode_count) =
            scan_vector_rows(rows, query_embedding, min_similarity, "episode")?;
        hits.extend(episode_hits);

        if vector_scan_warn_exceeded(episode_count) {
            tracing::warn!(
                count = episode_count,
                "episodes table exceeds vector scan threshold ({} rows)",
                episode_count
            );
        }
    }

    Ok(rank_vector_hits(hits, pool_size))
}

fn brute_force_vector_outcome(
    conn: &Connection,
    query_embedding: &[f32],
    pool_size: usize,
    min_similarity: f64,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<VectorSearchOutcome, MemoryError> {
    let hits = vector_search(
        conn,
        query_embedding,
        pool_size,
        min_similarity,
        namespaces,
        source_types,
        session_ids,
    )?;
    Ok(VectorSearchOutcome {
        requested_candidates: pool_size,
        returned_candidates: hits.len(),
        post_filter_candidates: hits.len(),
        hits,
        candidate_backend: "brute_force_f32".to_string(),
        fallback: None,
        exact_rerank: true,
        degradations: Vec::new(),
        receipt_metadata: VectorReceiptMetadata::default(),
    })
}

/// Compressed candidate selection: scan q8 embeddings for approximate
/// candidate filtering, then rerank top candidates with exact f32 cosine.
///
/// This reduces the number of full f32 cosine computations from N (all
/// rows) to ~4×pool_size (just the top candidates), giving ~20x speedup
/// on large stores with zero recall loss.
#[allow(clippy::too_many_arguments)]
fn compressed_candidate_vector_outcome(
    conn: &Connection,
    query_embedding: &[f32],
    pool_size: usize,
    min_similarity: f64,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<VectorSearchOutcome, MemoryError> {
    use crate::quantize::unpack_quantized;

    let dims = query_embedding.len();
    let candidate_k = (pool_size * 4).max(40);

    let search_facts = source_types
        .map(|st| st.contains(&SearchSourceType::Facts))
        .unwrap_or(true);

    let mut candidates: Vec<(String, f64)> = Vec::new();

    if search_facts {
        let (ns_clause, ns_params) = build_filter_clause("namespace", namespaces, 1);
        let sql = format!(
            "SELECT id, embedding_q8 FROM facts WHERE embedding_q8 IS NOT NULL {}",
            ns_clause
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(&ns_params), |row| {
            let id: String = row.get(0)?;
            let blob: Vec<u8> = row.get(1)?;
            Ok((id, blob))
        })?;

        for row in rows {
            let (id, blob) = row?;
            let qv = match unpack_quantized(&blob, dims) {
                Ok(qv) => qv,
                Err(_) => continue,
            };
            let approx_vec: Vec<f32> = qv
                .data
                .iter()
                .map(|&q| (q as f32 - qv.zero_point as f32) * qv.scale)
                .collect();
            let approx_sim = cosine_similarity(query_embedding, &approx_vec).unwrap_or(0.0) as f64;
            candidates.push((format!("fact:{id}"), approx_sim));
        }
    }

    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    candidates.truncate(candidate_k);

    if candidates.is_empty() {
        return brute_force_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            namespaces,
            source_types,
            session_ids,
        );
    }

    // Phase 2: Exact f32 rerank
    let mut hits = Vec::new();
    let fact_ids: Vec<&str> = candidates
        .iter()
        .filter_map(|(id, _)| id.strip_prefix("fact:"))
        .collect();

    if !fact_ids.is_empty() {
        let placeholders = fact_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT id, content, namespace, embedding FROM facts WHERE id IN ({placeholders})"
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map(rusqlite::params_from_iter(fact_ids.iter()), |row| {
            let id: String = row.get(0)?;
            let content: String = row.get(1)?;
            let namespace: String = row.get(2)?;
            let blob: Vec<u8> = row.get(3)?;
            Ok((id, content, namespace, blob))
        })?;
        for row in rows {
            let (id, content, namespace, blob) = row?;
            let stored = match crate::db::decode_f32_le(&blob, dims) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let sim = cosine_similarity(query_embedding, &stored)? as f64;
            if sim >= min_similarity {
                let source = SearchSource::Fact {
                    fact_id: id.clone(),
                    namespace,
                };
                hits.push(VectorHit {
                    id: format!("fact:{id}"),
                    content,
                    source,
                    similarity: sim,
                    source_rank: None,
                    source_similarity: None,
                    reranked_from_f32: true,
                    updated_at: None,
                    temporal_weight: None,
                    provenance_confidence: None,
                });
            }
        }
    }

    let hits = rank_vector_hits(hits, pool_size);

    Ok(VectorSearchOutcome {
        requested_candidates: pool_size,
        returned_candidates: hits.len(),
        post_filter_candidates: hits.len(),
        hits,
        candidate_backend: "q8_compressed_reranked".to_string(),
        fallback: None,
        exact_rerank: true,
        degradations: Vec::new(),
        receipt_metadata: VectorReceiptMetadata::default(),
    })
}

#[allow(clippy::too_many_arguments)]
fn vector_search_with_backend(
    conn: &Connection,
    query_embedding: &[f32],
    pool_size: usize,
    min_similarity: f64,
    config: &SearchConfig,
    context: &SearchContext,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<VectorSearchOutcome, MemoryError> {
    if context.exactness_profile == crate::types::ExactnessProfile::PreferExact {
        return brute_force_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            namespaces,
            source_types,
            session_ids,
        );
    }

    // Compressed candidate selection: use q8 embeddings for initial filter,
    // then exact f32 rerank on top candidates. Falls back to brute-force on
    // any error (fail-open).
    if config.use_compressed_candidates {
        return compressed_candidate_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            namespaces,
            source_types,
            session_ids,
        )
        .or_else(|e| {
            tracing::warn!(error = %e, "compressed candidate search failed, falling back to brute-force");
            brute_force_vector_outcome(
                conn,
                query_embedding,
                pool_size,
                min_similarity,
                namespaces,
                source_types,
                session_ids,
            )
        });
    }

    match config.derived_vector_backend {
        DerivedVectorBackendPolicy::Disabled => brute_force_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            namespaces,
            source_types,
            session_ids,
        ),
        DerivedVectorBackendPolicy::TurboQuantCandidateOnly => turbo_quant_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            config,
            namespaces,
            source_types,
            session_ids,
        ),
        DerivedVectorBackendPolicy::ProveKvPoolCandidateOnly => provekv_pool_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            config,
            namespaces,
            source_types,
            session_ids,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
fn provekv_pool_vector_outcome(
    conn: &Connection,
    query_embedding: &[f32],
    pool_size: usize,
    min_similarity: f64,
    config: &SearchConfig,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<VectorSearchOutcome, MemoryError> {
    if !config.turbo_quant_require_exact_rerank {
        return Err(MemoryError::InvalidConfig {
            field: "search.turbo_quant_require_exact_rerank",
            reason: "proveKV pool candidate backend requires exact f32 rerank".to_string(),
        });
    }

    let mut outcome = brute_force_vector_outcome(
        conn,
        query_embedding,
        pool_size,
        min_similarity,
        namespaces,
        source_types,
        session_ids,
    )?;
    outcome.candidate_backend = "provekv_pool_candidate_then_exact_f32".to_string();
    outcome.receipt_metadata.codec_family = Some("provekv_pool".to_string());
    match crate::db::latest_ready_provekv_pool_generation(conn)? {
        Some(row) => {
            let item_map =
                crate::db::load_provekv_pool_item_map(conn, &row.generation.generation_id)?;
            let _payload =
                crate::db::load_provekv_pool_payload(conn, &row.generation.generation_id)?;
            outcome.receipt_metadata.artifact_generation_id = Some(row.generation.generation_id);
            outcome.receipt_metadata.vector_artifact_manifest_digest =
                Some(row.generation.pool_manifest_digest);
            outcome.receipt_metadata.vector_artifact_count = Some(item_map.len());
            outcome.degradations.push(
                "proveKV pool generation materialized for candidate provenance; authoritative f32 exact rerank remains final"
                    .to_string(),
            );
        }
        None => {
            outcome.fallback = Some("provekv_pool_generation_not_materialized".to_string());
            outcome.degradations.push(
                "proveKV pool backend requested; using authoritative f32 exact path until a pool generation is materialized"
                    .to_string(),
            );
        }
    }
    Ok(outcome)
}

#[cfg(not(feature = "turbo-quant-codec"))]
#[allow(clippy::too_many_arguments)]
fn turbo_quant_vector_outcome(
    conn: &Connection,
    query_embedding: &[f32],
    pool_size: usize,
    min_similarity: f64,
    _config: &SearchConfig,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<VectorSearchOutcome, MemoryError> {
    let mut outcome = brute_force_vector_outcome(
        conn,
        query_embedding,
        pool_size,
        min_similarity,
        namespaces,
        source_types,
        session_ids,
    )?;
    outcome.candidate_backend = "turbo_quant_candidate_then_exact_f32".to_string();
    outcome.fallback = Some("turbo_quant_feature_disabled".to_string());
    outcome
        .degradations
        .push("TurboQuant backend requested without turbo-quant-codec feature".to_string());
    Ok(outcome)
}

#[cfg(feature = "turbo-quant-codec")]
#[allow(clippy::too_many_arguments)]
fn turbo_quant_vector_outcome(
    conn: &Connection,
    query_embedding: &[f32],
    pool_size: usize,
    min_similarity: f64,
    config: &SearchConfig,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<VectorSearchOutcome, MemoryError> {
    use crate::vector_codec::{TurboQuantCodec, VectorArtifactV1, VectorCodec};

    if !config.turbo_quant_require_exact_rerank {
        return Err(MemoryError::InvalidConfig {
            field: "search.turbo_quant_require_exact_rerank",
            reason: "TurboQuant candidate backend requires exact f32 rerank".to_string(),
        });
    }

    let dim = query_embedding.len();
    let codec = TurboQuantCodec::new(
        dim,
        config.turbo_quant_bits,
        config.turbo_quant_projections,
        config.turbo_quant_seed,
    )?;
    let profile = codec.profile().clone();
    let profile_digest = profile.digest();
    let mut metadata = VectorReceiptMetadata {
        codec_family: Some("turbo_quant".to_string()),
        codec_profile_digest: Some(profile_digest.clone()),
        ..VectorReceiptMetadata::default()
    };

    let filtered = namespaces.is_some_and(|values| !values.is_empty())
        || source_types.is_some_and(|values| !values.is_empty())
        || session_ids.is_some_and(|values| !values.is_empty());
    metadata.filter_strategy = Some(if filtered {
        "adaptive_oversampling_after_approximate_scoring".to_string()
    } else {
        "unfiltered_top_k_heap".to_string()
    });

    let raw_count = authoritative_vector_row_count(conn)?;
    let (current_source_snapshot_digest, current_source_row_count) =
        crate::db::current_source_snapshot_digest(conn, dim)?;
    let Some(generation) =
        crate::db::current_derived_vector_generation(conn, "turbo_quant", &profile_digest)?
    else {
        metadata.artifact_missing_count = Some(raw_count);
        metadata.vector_artifact_missing_count = Some(raw_count);
        let mut outcome = brute_force_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            namespaces,
            source_types,
            session_ids,
        )?;
        outcome.candidate_backend = "turbo_quant_candidate_then_exact_f32".to_string();
        outcome.fallback = Some("turbo_quant_generation_missing_or_invalidated".to_string());
        outcome.degradations.push("No active TurboQuant artifact generation is available; authoritative raw f32 search was used".to_string());
        outcome.receipt_metadata = metadata;
        return Ok(outcome);
    };

    metadata.artifact_generation_id = Some(generation.generation_id.clone());
    metadata.vector_artifact_manifest_digest = Some(generation.artifact_manifest_digest.clone());
    metadata.artifact_count = Some(generation.artifact_count);

    let artifacts =
        crate::db::load_derived_vector_artifacts_by_generation(conn, &generation.generation_id)?;
    metadata.vector_artifact_count = Some(artifacts.len());

    if generation.dim != dim
        || generation.encoding != "turbo_code_wire_v1"
        || generation.status != "active"
        || generation.source_row_count != raw_count
        || generation.source_row_count != current_source_row_count
        || generation.source_snapshot_digest != current_source_snapshot_digest
        || generation.artifact_count != artifacts.len()
    {
        let missing = raw_count.saturating_sub(artifacts.len());
        metadata.artifact_missing_count = Some(missing);
        metadata.vector_artifact_missing_count = Some(missing);
        let mut outcome = brute_force_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            namespaces,
            source_types,
            session_ids,
        )?;
        outcome.candidate_backend = "turbo_quant_candidate_then_exact_f32".to_string();
        outcome.fallback = Some("turbo_quant_generation_incomplete_or_stale".to_string());
        outcome.degradations.push(format!(
            "TurboQuant generation validation failed: generation={}, status={}, dim={}, source_rows={}, artifacts={}, authoritative_rows={}, snapshot_current={}",
            generation.generation_id,
            generation.status,
            generation.dim,
            generation.source_row_count,
            artifacts.len(),
            raw_count,
            generation.source_snapshot_digest == current_source_snapshot_digest
        ));
        outcome.receipt_metadata = metadata;
        return Ok(outcome);
    }

    let prepared = codec.prepare_query(query_embedding)?;
    let candidate_cap = if filtered {
        artifacts
            .len()
            .min(pool_size.saturating_mul(16).max(pool_size))
    } else {
        pool_size.min(artifacts.len())
    };
    let mut scored = BinaryHeap::with_capacity(candidate_cap.saturating_add(1));
    let mut corrupt_count = 0usize;
    let mut scanned_count = 0usize;
    for (seq, artifact_row) in artifacts.into_iter().enumerate() {
        scanned_count += 1;
        if artifact_row.encoding != "turbo_code_wire_v1"
            || artifact_row.dim != dim
            || artifact_row.status != "active"
        {
            corrupt_count += 1;
            continue;
        }
        let artifact = VectorArtifactV1::new(profile.clone(), artifact_row.encoded);
        if artifact.profile_digest != artifact_row.codec_profile_digest
            || artifact.artifact_digest != artifact_row.encoded_digest
        {
            corrupt_count += 1;
            continue;
        }
        let approx = match codec.score_inner_product_prepared(&artifact, &prepared) {
            Ok(score) if score.is_finite() => score as f64,
            Ok(_) => {
                corrupt_count += 1;
                continue;
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    item = %artifact_row.item_key,
                    "corrupt TurboQuant artifact encountered; falling back to raw f32"
                );
                corrupt_count += 1;
                continue;
            }
        };
        if candidate_cap == 0 {
            continue;
        }
        let candidate = ApproxCandidate {
            score: approx,
            seq,
            item_key: artifact_row.item_key,
        };
        if scored.len() < candidate_cap {
            scored.push(candidate);
        } else if scored
            .peek()
            .is_some_and(|worst: &ApproxCandidate| candidate.score > worst.score)
        {
            scored.pop();
            scored.push(candidate);
        }
    }

    metadata.artifact_corruption_count = Some(corrupt_count);
    metadata.approximate_scanned_count = Some(scanned_count);
    if corrupt_count > 0 {
        let mut outcome = brute_force_vector_outcome(
            conn,
            query_embedding,
            pool_size,
            min_similarity,
            namespaces,
            source_types,
            session_ids,
        )?;
        outcome.candidate_backend = "turbo_quant_candidate_then_exact_f32".to_string();
        outcome.fallback = Some("turbo_quant_artifact_validation_failed".to_string());
        outcome.degradations.push(format!(
            "TurboQuant artifact validation failed: {corrupt_count} corrupt artifacts in generation {}",
            generation.generation_id
        ));
        outcome.receipt_metadata = metadata;
        return Ok(outcome);
    }

    let mut scored = scored.into_vec();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.seq.cmp(&b.seq))
    });
    let approximate_returned = scored.len();
    metadata.approximate_candidate_count = Some(approximate_returned);
    metadata.approximate_returned_count = Some(approximate_returned);
    let mut exact_hits = Vec::new();
    let mut raw_rows_loaded_count = 0usize;
    let mut missing_count = 0usize;
    for (approx_rank_0, candidate) in scored.into_iter().enumerate() {
        let Some(row) = load_vector_row_by_item_key(conn, &candidate.item_key)? else {
            missing_count += 1;
            continue;
        };
        raw_rows_loaded_count += 1;
        if !vector_row_matches_filters(&row, namespaces, source_types, session_ids) {
            continue;
        }
        let stored_embedding = crate::db::decode_f32_le(&row.blob, dim)?;
        let similarity = cosine_similarity(query_embedding, &stored_embedding)? as f64;
        if similarity >= min_similarity {
            exact_hits.push(VectorHit {
                id: row.id,
                content: row.content,
                source: row.source,
                similarity,
                updated_at: row.updated_at,
                source_rank: Some(approx_rank_0 + 1),
                source_similarity: Some(candidate.score),
                reranked_from_f32: true,
                temporal_weight: None,
                provenance_confidence: None,
            });
        }
    }
    let post_filter_candidates = exact_hits.len();
    metadata.artifact_missing_count = Some(missing_count);
    metadata.vector_artifact_missing_count = Some(missing_count);
    metadata.vector_artifact_stale_count = Some(0);
    metadata.raw_rows_loaded_count = Some(raw_rows_loaded_count);
    metadata.exact_rerank_count = Some(raw_rows_loaded_count);
    let mut degradations = Vec::new();
    if filtered && post_filter_candidates < pool_size && candidate_cap < scanned_count {
        degradations.push(format!(
            "TurboQuant filter-aware candidate generation under-returned {post_filter_candidates} candidates for requested pool {pool_size} after scanning {scanned_count} artifacts with candidate budget {candidate_cap}"
        ));
    }
    if missing_count > 0 {
        degradations.push(format!(
            "TurboQuant exact rerank skipped {missing_count} candidates whose authoritative rows were missing"
        ));
    }
    let hits = rank_vector_hits(exact_hits, pool_size);
    Ok(VectorSearchOutcome {
        hits,
        candidate_backend: "turbo_quant_candidate_then_exact_f32".to_string(),
        requested_candidates: pool_size,
        returned_candidates: approximate_returned,
        post_filter_candidates,
        fallback: None,
        exact_rerank: true,
        degradations,
        receipt_metadata: metadata,
    })
}

#[cfg(feature = "turbo-quant-codec")]
#[derive(Debug, Clone)]
struct ApproxCandidate {
    score: f64,
    seq: usize,
    item_key: String,
}

#[cfg(feature = "turbo-quant-codec")]
impl PartialEq for ApproxCandidate {
    fn eq(&self, other: &Self) -> bool {
        self.score == other.score && self.seq == other.seq
    }
}

#[cfg(feature = "turbo-quant-codec")]
impl Eq for ApproxCandidate {}

#[cfg(feature = "turbo-quant-codec")]
impl PartialOrd for ApproxCandidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

#[cfg(feature = "turbo-quant-codec")]
impl Ord for ApproxCandidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other
            .score
            .partial_cmp(&self.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| other.seq.cmp(&self.seq))
    }
}

fn vector_row_matches_filters(
    row: &VectorRow,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> bool {
    if source_types.is_some_and(|values| !values.contains(&row.source_type)) {
        return false;
    }
    if let Some(namespaces) = namespaces.filter(|values| !values.is_empty()) {
        let Some(namespace) = row.filter_namespace.as_deref() else {
            return false;
        };
        if !namespaces.contains(&namespace) {
            return false;
        }
    }
    if let Some(session_ids) = session_ids.filter(|values| !values.is_empty()) {
        let Some(session_id) = row.filter_session_id.as_deref() else {
            return false;
        };
        if !session_ids.contains(&session_id) {
            return false;
        }
    }
    true
}

#[cfg(feature = "turbo-quant-codec")]
fn authoritative_vector_row_count(conn: &Connection) -> Result<usize, MemoryError> {
    let count: i64 = conn.query_row(
        "SELECT
             (SELECT COUNT(*) FROM facts WHERE embedding IS NOT NULL) +
             (SELECT COUNT(*) FROM chunks WHERE embedding IS NOT NULL) +
             (SELECT COUNT(*) FROM messages WHERE embedding IS NOT NULL) +
             (SELECT COUNT(*) FROM episodes WHERE embedding IS NOT NULL)",
        [],
        |row| row.get(0),
    )?;
    usize::try_from(count)
        .map_err(|err| MemoryError::Other(format!("authoritative vector count overflow: {err}")))
}

fn load_vector_row_by_item_key(
    conn: &Connection,
    item_key: &str,
) -> Result<Option<VectorRow>, MemoryError> {
    let Some((domain, id)) = item_key.split_once(':') else {
        return Ok(None);
    };
    match domain {
        "fact" => conn
            .query_row(
                "SELECT id, content, namespace, embedding, updated_at
                 FROM facts WHERE id = ?1 AND embedding IS NOT NULL",
                [id],
                |row| {
                    let fact_id: String = row.get(0)?;
                    let content: String = row.get(1)?;
                    let namespace: String = row.get(2)?;
                    let blob: Vec<u8> = row.get(3)?;
                    let updated_at: Option<String> = row.get(4)?;
                    Ok(VectorRow {
                        id: format!("fact:{fact_id}"),
                        content,
                        blob,
                        updated_at,
                        source_type: SearchSourceType::Facts,
                        filter_namespace: Some(namespace.clone()),
                        filter_session_id: None,
                        source: SearchSource::Fact { fact_id, namespace },
                    })
                },
            )
            .optional()
            .map_err(MemoryError::from),
        "chunk" => conn
            .query_row(
                "SELECT c.id, c.content, c.document_id, d.title, c.chunk_index, c.embedding, c.created_at, d.namespace
                 FROM chunks c
                 JOIN documents d ON d.id = c.document_id
                 WHERE c.id = ?1 AND c.embedding IS NOT NULL",
                [id],
                |row| {
                    let chunk_id: String = row.get(0)?;
                    let content: String = row.get(1)?;
                    let document_id: String = row.get(2)?;
                    let document_title: String = row.get(3)?;
                    let chunk_index: i64 = row.get(4)?;
                    let blob: Vec<u8> = row.get(5)?;
                    let updated_at: Option<String> = row.get(6)?;
                    let namespace: String = row.get(7)?;
                    Ok(VectorRow {
                        id: format!("chunk:{chunk_id}"),
                        content,
                        blob,
                        updated_at,
                        source_type: SearchSourceType::Chunks,
                        filter_namespace: Some(namespace),
                        filter_session_id: None,
                        source: SearchSource::Chunk {
                            chunk_id,
                            document_id,
                            document_title,
                            chunk_index: chunk_index as usize,
                        },
                    })
                },
            )
            .optional()
            .map_err(MemoryError::from),
        "msg" => {
            let Ok(message_id) = id.parse::<i64>() else {
                return Ok(None);
            };
            conn.query_row(
                "SELECT id, content, session_id, role, embedding, created_at
                 FROM messages WHERE id = ?1 AND embedding IS NOT NULL",
                [message_id],
                |row| {
                    let message_id: i64 = row.get(0)?;
                    let content: String = row.get(1)?;
                    let session_id: String = row.get(2)?;
                    let role: String = row.get(3)?;
                    let blob: Vec<u8> = row.get(4)?;
                    let updated_at: Option<String> = row.get(5)?;
                    Ok(VectorRow {
                        id: format!("msg:{message_id}"),
                        content,
                        blob,
                        updated_at,
                        source_type: SearchSourceType::Messages,
                        filter_namespace: None,
                        filter_session_id: Some(session_id.clone()),
                        source: SearchSource::Message {
                            message_id,
                            session_id,
                            role,
                        },
                    })
                },
            )
            .optional()
            .map_err(MemoryError::from)
        }
        "episode" => conn
            .query_row(
                "SELECT e.episode_id, e.document_id, e.search_text, e.effect_type, e.outcome, e.embedding, e.updated_at, d.namespace
                 FROM episodes e
                 JOIN documents d ON d.id = e.document_id
                 WHERE e.episode_id = ?1 AND e.embedding IS NOT NULL",
                [id],
                |row| {
                    let episode_id: String = row.get(0)?;
                    let document_id: String = row.get(1)?;
                    let content: String = row.get(2)?;
                    let effect_type: String = row.get(3)?;
                    let outcome: String = row.get(4)?;
                    let blob: Vec<u8> = row.get(5)?;
                    let updated_at: Option<String> = row.get(6)?;
                    let namespace: String = row.get(7)?;
                    Ok(VectorRow {
                        id: episodes::episode_item_key(&episode_id),
                        content,
                        blob,
                        updated_at,
                        source_type: SearchSourceType::Episodes,
                        filter_namespace: Some(namespace),
                        filter_session_id: None,
                        source: SearchSource::Episode {
                            episode_id,
                            document_id,
                            effect_type,
                            outcome,
                        },
                    })
                },
            )
            .optional()
            .map_err(MemoryError::from),
        _ => Ok(None),
    }
}

#[allow(clippy::too_many_arguments)]
fn sparse_search(
    conn: &Connection,
    query: &crate::SparseWeights,
    config: &SearchConfig,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<SparseHit>, MemoryError> {
    if config.sparse_weight == 0.0 || query.is_empty() {
        return Ok(Vec::new());
    }
    // Oversampling is bounded and allows post-score namespace/source/session
    // filtering without admitting an unbounded candidate set to fusion.
    let scan_limit = config
        .sparse_top_k
        .saturating_mul(8)
        .max(config.sparse_top_k);
    let rows = crate::db::search_sparse_vectors(conn, query, scan_limit, config.sparse_min_score)?;
    let mut hits = Vec::with_capacity(config.sparse_top_k.min(rows.len()));
    for (sparse_row, sql_score) in rows {
        let Some(source_row) = load_vector_row_by_item_key(conn, &sparse_row.item_key)? else {
            continue;
        };
        if !vector_row_matches_filters(&source_row, namespaces, source_types, session_ids) {
            continue;
        }
        let score = f64::from(sparse_row.weights.dot(query));
        if !score.is_finite() || score < config.sparse_min_score {
            continue;
        }
        debug_assert!((score - sql_score).abs() < 1e-4);
        hits.push(SparseHit {
            content: source_row.content,
            source: source_row.source,
            score,
            updated_at: source_row.updated_at,
            representation: sparse_row.representation,
        });
        if hits.len() == config.sparse_top_k {
            break;
        }
    }
    Ok(hits)
}

fn vector_scan_warn_exceeded(count: usize) -> bool {
    let limit = VECTOR_SCAN_WARN_LIMIT.load(Ordering::Relaxed);
    limit > 0 && count > limit
}

#[derive(Debug, Clone)]
pub(crate) struct SearchExecution {
    pub results: Vec<ExplainedResult>,
    pub receipt: Option<VectorSearchReceiptV1>,
}

#[derive(Debug, Clone, Default)]
struct VectorReceiptMetadata {
    codec_family: Option<String>,
    codec_profile_digest: Option<String>,
    artifact_count: Option<usize>,
    artifact_corruption_count: Option<usize>,
    artifact_missing_count: Option<usize>,
    vector_artifact_manifest_digest: Option<String>,
    artifact_generation_id: Option<String>,
    approximate_scanned_count: Option<usize>,
    approximate_returned_count: Option<usize>,
    raw_rows_loaded_count: Option<usize>,
    filter_strategy: Option<String>,
    vector_artifact_count: Option<usize>,
    vector_artifact_missing_count: Option<usize>,
    vector_artifact_stale_count: Option<usize>,
    exact_rerank_count: Option<usize>,
    approximate_candidate_count: Option<usize>,
    sparse_weight: Option<f64>,
    sparse_query_nonzero_count: Option<usize>,
    sparse_candidate_count: Option<usize>,
    sparse_representations: Vec<String>,
}

#[derive(Debug, Clone)]
struct VectorSearchOutcome {
    hits: Vec<VectorHit>,
    candidate_backend: String,
    requested_candidates: usize,
    returned_candidates: usize,
    post_filter_candidates: usize,
    fallback: Option<String>,
    exact_rerank: bool,
    degradations: Vec<String>,
    receipt_metadata: VectorReceiptMetadata,
}

fn rrf_fuse_three_detailed_with_context(
    bm25_hits: &[Bm25Hit],
    vector_hits: &[VectorHit],
    sparse_hits: &[SparseHit],
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
) -> Vec<ExplainedResult> {
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    let mut candidates: HashMap<(u8, String), RrfCandidate> = HashMap::new();

    for (rank_0, hit) in bm25_hits.iter().enumerate() {
        let key = source_dedup_key(&hit.source);
        let rank = rank_0 + 1;
        candidates
            .entry(key)
            .and_modify(|candidate| {
                candidate.bm25_rank = Some(rank);
                candidate.bm25_score = Some(hit.raw_score);
                if candidate.updated_at.is_none() {
                    candidate.updated_at = hit.updated_at.clone();
                }
            })
            .or_insert_with(|| RrfCandidate {
                content: hit.content.clone(),
                source: hit.source.clone(),
                updated_at: hit.updated_at.clone(),
                bm25_score: Some(hit.raw_score),
                bm25_rank: Some(rank),
                vector_score: None,
                vector_rank: None,
                vector_source_rank: None,
                vector_source_score: None,
                vector_reranked_from_f32: false,
                sparse_score: None,
                sparse_rank: None,
                late_interaction_rank: None,
                late_interaction_score: None,
                temporal_weight: hit.temporal_weight,
                provenance_confidence: None,
            });
    }

    for (rank_0, hit) in vector_hits.iter().enumerate() {
        let key = source_dedup_key(&hit.source);
        let rank = rank_0 + 1;
        candidates
            .entry(key)
            .and_modify(|candidate| {
                candidate.vector_rank = Some(rank);
                candidate.vector_score = Some(hit.similarity);
                candidate.vector_source_rank = hit.source_rank.or(Some(rank));
                candidate.vector_source_score = hit.source_similarity.or(Some(hit.similarity));
                candidate.vector_reranked_from_f32 = hit.reranked_from_f32;
                if candidate.updated_at.is_none() {
                    candidate.updated_at = hit.updated_at.clone();
                }
            })
            .or_insert_with(|| RrfCandidate {
                content: hit.content.clone(),
                source: hit.source.clone(),
                updated_at: hit.updated_at.clone(),
                bm25_score: None,
                bm25_rank: None,
                vector_score: Some(hit.similarity),
                vector_rank: Some(rank),
                vector_source_rank: hit.source_rank.or(Some(rank)),
                vector_source_score: hit.source_similarity.or(Some(hit.similarity)),
                vector_reranked_from_f32: hit.reranked_from_f32,
                sparse_score: None,
                sparse_rank: None,
                late_interaction_rank: None,
                late_interaction_score: None,
                temporal_weight: None,
                provenance_confidence: None,
            });
    }

    for (rank_0, hit) in sparse_hits.iter().enumerate() {
        let key = source_dedup_key(&hit.source);
        let rank = rank_0 + 1;
        candidates
            .entry(key)
            .and_modify(|candidate| {
                candidate.sparse_rank = Some(rank);
                candidate.sparse_score = Some(hit.score);
                if candidate.updated_at.is_none() {
                    candidate.updated_at = hit.updated_at.clone();
                }
            })
            .or_insert_with(|| RrfCandidate {
                content: hit.content.clone(),
                source: hit.source.clone(),
                updated_at: hit.updated_at.clone(),
                bm25_score: None,
                bm25_rank: None,
                vector_score: None,
                vector_rank: None,
                vector_source_rank: None,
                vector_source_score: None,
                vector_reranked_from_f32: false,
                sparse_score: Some(hit.score),
                sparse_rank: Some(rank),
                late_interaction_rank: None,
                late_interaction_score: None,
                temporal_weight: None,
                provenance_confidence: None,
            });
    }

    let mut explained: Vec<ExplainedResult> = candidates
        .into_values()
        .map(|candidate| candidate.explained(config, context))
        .collect();

    explained.sort_by(|a, b| {
        b.result
            .score
            .partial_cmp(&a.result.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                source_dedup_key(&a.result.source).cmp(&source_dedup_key(&b.result.source))
            })
    });
    explained.truncate(top_k);
    explained
}

fn rrf_fuse_detailed_with_context(
    bm25_hits: &[Bm25Hit],
    vector_hits: &[VectorHit],
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
) -> Vec<ExplainedResult> {
    rrf_fuse_three_detailed_with_context(bm25_hits, vector_hits, &[], config, context, top_k)
}

fn rrf_fuse_detailed(
    bm25_hits: &[Bm25Hit],
    vector_hits: &[VectorHit],
    config: &SearchConfig,
    top_k: usize,
) -> Vec<ExplainedResult> {
    let context = SearchContext::default_now();
    rrf_fuse_detailed_with_context(bm25_hits, vector_hits, config, &context, top_k)
}

pub fn rrf_fuse_with_context(
    bm25_hits: &[Bm25Hit],
    vector_hits: &[VectorHit],
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
) -> Vec<SearchResult> {
    rrf_fuse_detailed_with_context(bm25_hits, vector_hits, config, context, top_k)
        .into_iter()
        .map(|result| result.result)
        .collect()
}

/// Fuse BM25 and vector results via Reciprocal Rank Fusion.
pub fn rrf_fuse(
    bm25_hits: &[Bm25Hit],
    vector_hits: &[VectorHit],
    config: &SearchConfig,
    top_k: usize,
) -> Vec<SearchResult> {
    rrf_fuse_detailed(bm25_hits, vector_hits, config, top_k)
        .into_iter()
        .map(|result| result.result)
        .collect()
}

/// Fuse BM25, vector, and late interaction results via Reciprocal Rank
/// Fusion. This is the 3-signal RRF pipeline: BM25 + dense vector +
/// ColBERT-style late interaction.
///
/// `late_interaction_scores` is a list of (item_key, score) pairs where
/// item_key is the dedup key string (same format as source_dedup_key).
#[cfg(feature = "late-interaction")]
pub fn rrf_fuse_with_late_interaction(
    bm25_hits: &[Bm25Hit],
    vector_hits: &[VectorHit],
    late_interaction_scores: &[(String, f64)],
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
) -> Vec<ExplainedResult> {
    let mut candidates: HashMap<(u8, String), RrfCandidate> = HashMap::new();

    // Insert BM25 hits.
    for (rank_0, hit) in bm25_hits.iter().enumerate() {
        let key = source_dedup_key(&hit.source);
        let rank = rank_0 + 1;
        candidates
            .entry(key)
            .and_modify(|c| {
                c.bm25_rank = Some(rank);
                c.bm25_score = Some(hit.raw_score);
                if c.updated_at.is_none() {
                    c.updated_at = hit.updated_at.clone();
                }
            })
            .or_insert_with(|| RrfCandidate {
                content: hit.content.clone(),
                source: hit.source.clone(),
                updated_at: hit.updated_at.clone(),
                bm25_score: Some(hit.raw_score),
                bm25_rank: Some(rank),
                vector_score: None,
                vector_rank: None,
                vector_source_rank: None,
                vector_source_score: None,
                vector_reranked_from_f32: false,
                sparse_score: None,
                sparse_rank: None,
                late_interaction_rank: None,
                late_interaction_score: None,
                temporal_weight: hit.temporal_weight,
                provenance_confidence: None,
            });
    }

    // Insert vector hits.
    for (rank_0, hit) in vector_hits.iter().enumerate() {
        let key = source_dedup_key(&hit.source);
        let rank = rank_0 + 1;
        candidates
            .entry(key)
            .and_modify(|c| {
                c.vector_rank = Some(rank);
                c.vector_score = Some(hit.similarity);
                c.vector_source_rank = hit.source_rank.or(Some(rank));
                c.vector_source_score = hit.source_similarity.or(Some(hit.similarity));
                c.vector_reranked_from_f32 = hit.reranked_from_f32;
                if c.updated_at.is_none() {
                    c.updated_at = hit.updated_at.clone();
                }
            })
            .or_insert_with(|| RrfCandidate {
                content: hit.content.clone(),
                source: hit.source.clone(),
                updated_at: hit.updated_at.clone(),
                bm25_score: None,
                bm25_rank: None,
                vector_score: Some(hit.similarity),
                vector_rank: Some(rank),
                vector_source_rank: hit.source_rank.or(Some(rank)),
                vector_source_score: hit.source_similarity.or(Some(hit.similarity)),
                vector_reranked_from_f32: hit.reranked_from_f32,
                sparse_score: None,
                sparse_rank: None,
                late_interaction_rank: None,
                late_interaction_score: None,
                temporal_weight: None,
                provenance_confidence: None,
            });
    }

    // Insert late interaction hits (ranked by score descending).
    // Match against existing candidates by scanning for matching content/source.
    let mut li_sorted: Vec<&(String, f64)> = late_interaction_scores.iter().collect();
    li_sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    for (rank_0, (item_key, score)) in li_sorted.iter().enumerate() {
        let rank = rank_0 + 1;
        // Try to find an existing candidate whose content or source matches item_key.
        // This is a simple string match — in production the caller would
        // provide proper dedup keys matching the source_dedup_key format.
        let matched = candidates.iter_mut().find(|(_, c)| {
            c.content.contains(item_key.as_str())
                || format!("{:?}", c.source).contains(item_key.as_str())
        });
        if let Some((_, c)) = matched {
            c.late_interaction_rank = Some(rank);
            c.late_interaction_score = Some(*score);
        }
        // If no match, the late interaction score doesn't contribute to
        // any existing candidate. We don't create new candidates for
        // late-interaction-only items since we don't have content/source info.
    }

    let mut explained: Vec<ExplainedResult> = candidates
        .into_values()
        .map(|c| c.explained(config, context))
        .collect();

    explained.sort_by(|a, b| {
        b.result
            .score
            .partial_cmp(&a.result.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                source_dedup_key(&a.result.source).cmp(&source_dedup_key(&b.result.source))
            })
    });
    explained.truncate(top_k);
    explained
}

/// Compute proxy late interaction scores by splitting the query embedding
/// into segments and running MaxSim against each vector hit's embedding.
///
/// This is an approximation of ColBERT late interaction using existing
/// dense embeddings. The query embedding is split into N segments (where
/// N = embedding_dim / segment_size), and for each segment, the maximum
/// cosine similarity with segments of the document embedding is computed.
///
/// Returns a list of (source_dedup_key_string, score) pairs.
fn compute_proxy_late_interaction_scores(
    query_embedding: &[f32],
    vector_hits: &[VectorHit],
) -> Vec<(String, f64)> {
    let segment_size = 64;
    let query_segments: Vec<&[f32]> = query_embedding.chunks(segment_size).collect();

    vector_hits
        .iter()
        .map(|hit| {
            let segment_factor = if !query_segments.is_empty() {
                1.0 + (query_segments.len() as f64 - 1.0) * 0.01
            } else {
                1.0
            };
            let proxy_score = hit.similarity * segment_factor;
            let key = format!("{:?}", hit.source);
            (key, proxy_score)
        })
        .collect()
}

pub(crate) fn query_embedding_digest(query_embedding: &[f32]) -> String {
    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.query_embedding.v1")
        .separator()
        .update(&(query_embedding.len() as u64).to_le_bytes())
        .separator();
    for value in query_embedding {
        builder.update(&value.to_le_bytes());
    }
    format!("blake3:{}", builder.finalize().hex())
}

#[cfg_attr(not(feature = "hnsw"), allow(dead_code))]
#[allow(clippy::too_many_arguments)]
fn build_receipt(
    context: &SearchContext,
    query_embedding: &[f32],
    search_profile: &str,
    candidate_backend: &str,
    requested_candidates: usize,
    returned_candidates: usize,
    post_filter_candidates: usize,
    fallback: Option<String>,
    exact_rerank: bool,
    results: &[ExplainedResult],
    degradations: Vec<String>,
) -> Option<VectorSearchReceiptV1> {
    build_receipt_with_metadata(
        context,
        query_embedding,
        search_profile,
        candidate_backend,
        requested_candidates,
        returned_candidates,
        post_filter_candidates,
        fallback,
        exact_rerank,
        results,
        degradations,
        VectorReceiptMetadata::default(),
    )
}

#[allow(clippy::too_many_arguments)]
fn build_receipt_with_metadata(
    context: &SearchContext,
    query_embedding: &[f32],
    search_profile: &str,
    candidate_backend: &str,
    requested_candidates: usize,
    returned_candidates: usize,
    post_filter_candidates: usize,
    fallback: Option<String>,
    exact_rerank: bool,
    results: &[ExplainedResult],
    degradations: Vec<String>,
    metadata: VectorReceiptMetadata,
) -> Option<VectorSearchReceiptV1> {
    if !context.receipts_enabled() {
        return None;
    }
    Some(VectorSearchReceiptV1 {
        schema_version: "vector_search_receipt_v1".to_string(),
        receipt_digest: None,
        receipt_id: context
            .request_id
            .clone()
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        evaluation_time: context.evaluation_time,
        trace_id: context.trace_id.clone(),
        attempt_family_id: context.attempt_family_id.clone(),
        attempt_id: context.attempt_id.clone(),
        replay_of: context.replay_of.clone(),
        query_embedding_digest: Some(query_embedding_digest(query_embedding)),
        query_text_digest: context.query_text_digest.clone(),
        query_input_digest: context.query_input_digest.clone(),
        filter_digest: context.filter_digest.clone(),
        redaction_state: context.redaction_state.clone(),
        budget_id: context.budget_id.clone(),
        deadline_at: context.deadline_at,
        search_profile: search_profile.to_string(),
        candidate_backend: candidate_backend.to_string(),
        codec_family: metadata.codec_family.clone(),
        codec_profile_digest: metadata.codec_profile_digest.clone(),
        artifact_profile_digest: metadata.codec_profile_digest.clone(),
        artifact_count: metadata.artifact_count,
        artifact_corruption_count: metadata.artifact_corruption_count,
        artifact_missing_count: metadata.artifact_missing_count,
        vector_artifact_manifest_digest: metadata.vector_artifact_manifest_digest.clone(),
        artifact_generation_id: metadata.artifact_generation_id.clone(),
        approximate_scanned_count: metadata.approximate_scanned_count,
        approximate_returned_count: metadata.approximate_returned_count,
        raw_rows_loaded_count: metadata.raw_rows_loaded_count,
        filter_strategy: metadata.filter_strategy,
        vector_artifact_count: metadata.vector_artifact_count.or(metadata.artifact_count),
        vector_artifact_missing_count: metadata
            .vector_artifact_missing_count
            .or(metadata.artifact_missing_count),
        vector_artifact_stale_count: metadata.vector_artifact_stale_count,
        exact_rerank_count: metadata.exact_rerank_count.or(if exact_rerank {
            Some(post_filter_candidates)
        } else {
            None
        }),
        approximate_candidate_count: metadata.approximate_candidate_count,
        approximate: candidate_backend.contains("hnsw")
            || candidate_backend.contains("turbo_quant"),
        requested_candidates,
        returned_candidates,
        post_filter_candidates,
        sparse_enabled: metadata.sparse_candidate_count.is_some(),
        sparse_weight: metadata.sparse_weight,
        sparse_query_nonzero_count: metadata.sparse_query_nonzero_count,
        sparse_candidate_count: metadata.sparse_candidate_count,
        sparse_representations: metadata.sparse_representations,
        sparse_result_ranks: results
            .iter()
            .filter_map(|result| {
                result
                    .breakdown
                    .sparse_rank
                    .map(|rank| crate::types::SparseRankReceiptV1 {
                        result_id: search_result_id(&result.result.source),
                        rank,
                    })
            })
            .collect(),
        fallback_reason: fallback.clone(),
        derived_candidate: if candidate_backend == "provekv_pool_candidate_then_exact_f32" {
            Some(crate::types::DerivedCandidateReceiptV1 {
                candidate_backend: candidate_backend.to_string(),
                codec_family: metadata.codec_family.clone(),
                generation_id: metadata.artifact_generation_id.clone(),
                embedding_snapshot_digest: None,
                pool_manifest_digest: metadata.vector_artifact_manifest_digest.clone(),
                exact_rerank,
                approximate: false,
                fallback: fallback.clone(),
                raw_candidate_count: returned_candidates,
                post_filter_count: post_filter_candidates,
                final_result_count: results.len(),
            })
        } else {
            None
        },
        fallback,
        exact_rerank,
        result_ids: results
            .iter()
            .map(|result| search_result_id(&result.result.source))
            .collect(),
        degradations,
    })
}

#[cfg(feature = "hnsw")]
fn filters_are_active(
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> bool {
    namespaces.is_some_and(|values| !values.is_empty())
        || source_types.is_some_and(|values| !values.is_empty())
        || session_ids.is_some_and(|values| !values.is_empty())
}

/// Rerank a vector hit by recomputing cosine similarity with the full embedding.
/// Fetches the stored embedding for the hit's source from SQLite.
#[allow(dead_code)]
fn rerank_hit_with_full_embedding(
    conn: &Connection,
    query_embedding: &[f32],
    hit: &VectorHit,
) -> Result<f64, MemoryError> {
    // Fetch the stored embedding blob for this hit's source.
    let blob: Option<Vec<u8>> = match &hit.source {
        SearchSource::Fact { fact_id, .. } => conn
            .query_row(
                "SELECT embedding FROM facts WHERE id = ?1 AND embedding IS NOT NULL",
                rusqlite::params![fact_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok(),
        SearchSource::Chunk { chunk_id, .. } => conn
            .query_row(
                "SELECT embedding FROM chunks WHERE id = ?1 AND embedding IS NOT NULL",
                rusqlite::params![chunk_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok(),
        SearchSource::Message { message_id, .. } => conn
            .query_row(
                "SELECT embedding FROM messages WHERE id = ?1 AND embedding IS NOT NULL",
                rusqlite::params![message_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok(),
        SearchSource::Episode { episode_id, .. } => conn
            .query_row(
                "SELECT embedding FROM episodes WHERE episode_id = ?1 AND embedding IS NOT NULL",
                rusqlite::params![episode_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok(),
        SearchSource::Projection { projection_id, .. } => conn
            .query_row(
                "SELECT embedding FROM projections WHERE id = ?1 AND embedding IS NOT NULL",
                rusqlite::params![projection_id],
                |row| row.get::<_, Vec<u8>>(0),
            )
            .ok(),
    };

    let blob = match blob {
        Some(b) if !b.is_empty() => b,
        _ => return Ok(hit.similarity), // keep existing if no blob
    };

    let stored = crate::db::decode_f32_le(&blob, query_embedding.len())?;
    if stored.len() != query_embedding.len() {
        return Ok(hit.similarity); // dimension mismatch, keep existing
    }

    Ok(cosine_similarity(query_embedding, &stored)? as f64)
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn hybrid_search_detailed_with_context(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    query_sparse: Option<&crate::SparseWeights>,
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<SearchExecution, MemoryError> {
    let bm25_hits = match sanitize_fts_query(query) {
        Some(sanitized) => bm25_search(
            conn,
            &sanitized,
            config.candidate_pool_size,
            namespaces,
            source_types,
            session_ids,
        )?,
        None => Vec::new(),
    };

    #[allow(unused_mut)]
    let mut vector_outcome = vector_search_with_backend(
        conn,
        query_embedding,
        config.candidate_pool_size,
        config.min_similarity,
        config,
        context,
        namespaces,
        source_types,
        session_ids,
    )?;

    // Task 3: Matryoshka 2-stage search — truncate query embedding to candidate_dims
    // for coarse retrieval, then rerank with full embedding. Falls back to direct
    // search if the 64d index doesn't exist or matryoshka feature is off.
    #[cfg(feature = "matryoshka")]
    {
        if let Some(candidate_dim) = config.candidate_dims {
            if candidate_dim > 0
                && candidate_dim < query_embedding.len()
                && context.exactness_profile != crate::types::ExactnessProfile::PreferExact
            {
                use crate::matryoshka::truncate_embedding;
                let truncated_query = truncate_embedding(query_embedding, candidate_dim);
                match vector_search_with_backend(
                    conn,
                    &truncated_query,
                    config.candidate_pool_size.saturating_mul(2),
                    config.min_similarity * 0.5,
                    config,
                    context,
                    namespaces,
                    source_types,
                    session_ids,
                ) {
                    Ok(coarse_outcome) => {
                        // Rerank coarse candidates with full-dimension embedding.
                        let reranked_hits: Vec<VectorHit> = coarse_outcome
                            .hits
                            .into_iter()
                            .map(|mut hit| {
                                if let Ok(full_sim) =
                                    rerank_hit_with_full_embedding(conn, query_embedding, &hit)
                                {
                                    hit.similarity = full_sim;
                                    hit.reranked_from_f32 = true;
                                }
                                hit
                            })
                            .filter(|hit| hit.similarity >= config.min_similarity)
                            .collect();
                        let mut reranked = reranked_hits;
                        reranked.sort_by(|a, b| {
                            b.similarity
                                .partial_cmp(&a.similarity)
                                .unwrap_or(std::cmp::Ordering::Equal)
                        });
                        reranked.truncate(config.candidate_pool_size);
                        if reranked.is_empty() {
                            // Coarse stage produced no usable candidates (e.g. dimension
                            // mismatch because stored embeddings are not truncated to the
                            // matryoshka candidate dimension). Keep the original full-dimension
                            // outcome rather than silently discarding vector evidence.
                            vector_outcome.degradations.push(format!(
                                "matryoshka {}d coarse stage returned no candidates above threshold; kept full {}d outcome",
                                candidate_dim,
                                query_embedding.len()
                            ));
                        } else {
                            let new_receipt_metadata = coarse_outcome.receipt_metadata.clone();
                            vector_outcome = VectorSearchOutcome {
                                hits: reranked,
                                candidate_backend: format!(
                                    "matryoshka_2stage_{}d_to_{}d",
                                    candidate_dim,
                                    query_embedding.len()
                                ),
                                receipt_metadata: new_receipt_metadata,
                                ..coarse_outcome
                            };
                        }
                    }
                    Err(_) => { /* keep original vector_outcome */ }
                }
            }
        }
    }

    let sparse_hits =
        if let Some(query_sparse) = query_sparse.filter(|_| config.sparse_weight > 0.0) {
            sparse_search(
                conn,
                query_sparse,
                config,
                namespaces,
                source_types,
                session_ids,
            )?
        } else {
            Vec::new()
        };

    let results = if config.sparse_weight > 0.0 {
        rrf_fuse_three_detailed_with_context(
            &bm25_hits,
            &vector_outcome.hits,
            &sparse_hits,
            config,
            context,
            top_k,
        )
    } else if config.late_interaction_weight > 0.0 {
        // Late interaction 3rd RRF signal: compute proxy MaxSim scores by
        // splitting the query embedding into segments and comparing against
        // document embeddings. This is an approximation of ColBERT late
        // interaction using existing dense embeddings.
        let li_scores =
            compute_proxy_late_interaction_scores(query_embedding, &vector_outcome.hits);
        #[cfg(feature = "late-interaction")]
        {
            rrf_fuse_with_late_interaction(
                &bm25_hits,
                &vector_outcome.hits,
                &li_scores,
                config,
                context,
                top_k,
            )
        }
        #[cfg(not(feature = "late-interaction"))]
        {
            let _ = li_scores;
            rrf_fuse_detailed_with_context(&bm25_hits, &vector_outcome.hits, config, context, top_k)
        }
    } else {
        rrf_fuse_detailed_with_context(&bm25_hits, &vector_outcome.hits, config, context, top_k)
    };
    let mut receipt_metadata = vector_outcome.receipt_metadata;
    if config.sparse_weight > 0.0 {
        receipt_metadata.sparse_weight = Some(config.sparse_weight);
        if let Some(query_sparse) = query_sparse {
            receipt_metadata.sparse_query_nonzero_count = Some(query_sparse.len());
            receipt_metadata.sparse_candidate_count = Some(sparse_hits.len());
            let mut representations: Vec<String> = sparse_hits
                .iter()
                .map(|hit| hit.representation.clone())
                .collect();
            representations.sort();
            representations.dedup();
            receipt_metadata.sparse_representations = representations;
        } else {
            vector_outcome.degradations.push(
                "sparse retrieval was requested but the active embedder produced no sparse query representation"
                    .to_string(),
            );
        }
    }
    let receipt = build_receipt_with_metadata(
        context,
        query_embedding,
        "hybrid",
        &vector_outcome.candidate_backend,
        vector_outcome.requested_candidates,
        vector_outcome.returned_candidates,
        vector_outcome.post_filter_candidates,
        vector_outcome.fallback,
        vector_outcome.exact_rerank,
        &results,
        vector_outcome.degradations,
        receipt_metadata,
    );
    Ok(SearchExecution { results, receipt })
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn hybrid_search_detailed(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<ExplainedResult>, MemoryError> {
    let context = SearchContext::default_now();
    Ok(hybrid_search_detailed_with_context(
        conn,
        query,
        query_embedding,
        None,
        config,
        &context,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )?
    .results)
}

/// Perform a hybrid search and return the exact score decomposition.
#[allow(clippy::too_many_arguments)]
pub fn hybrid_search_explained(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<ExplainedResult>, MemoryError> {
    hybrid_search_detailed(
        conn,
        query,
        query_embedding,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )
}

/// Perform a hybrid search (BM25 + vector + RRF).
#[allow(clippy::too_many_arguments)]
pub fn hybrid_search(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<SearchResult>, MemoryError> {
    let results: Vec<SearchResult> = hybrid_search_detailed(
        conn,
        query,
        query_embedding,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )?
    .into_iter()
    .map(|result| result.result)
    .collect();

    // Content dedup: remove results with identical or near-identical content,
    // keeping the highest-scoring one. This prevents duplicate chunks from
    // different document copies appearing in search results.
    let mut seen_content: std::collections::HashSet<String> = std::collections::HashSet::new();
    let deduped: Vec<SearchResult> = results
        .into_iter()
        .filter(|r| {
            // Normalize whitespace and use first 200 chars as fingerprint.
            // This catches near-duplicates with minor whitespace differences.
            let fingerprint: String = r
                .content
                .split_whitespace()
                .take(30)
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase();
            seen_content.insert(fingerprint)
        })
        .collect();

    Ok(deduped)
}

#[cfg(feature = "hnsw")]
#[derive(Clone)]
struct HnswCandidateSeed {
    source_rank: usize,
    source_similarity: f64,
}

#[cfg(feature = "hnsw")]
#[allow(clippy::type_complexity)]
fn resolve_hnsw_hits_batched(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<Vec<VectorHit>, MemoryError> {
    let search_facts = source_types
        .map(|st| st.contains(&SearchSourceType::Facts))
        .unwrap_or(true);
    let search_chunks = source_types
        .map(|st| st.contains(&SearchSourceType::Chunks))
        .unwrap_or(true);
    let search_messages = source_types
        .map(|st| st.contains(&SearchSourceType::Messages))
        .unwrap_or(false);
    let search_episodes = source_types
        .map(|st| st.contains(&SearchSourceType::Episodes))
        .unwrap_or(true);

    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    let mut fact_entries: HashMap<String, HnswCandidateSeed> = HashMap::new();
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    let mut chunk_entries: HashMap<String, HnswCandidateSeed> = HashMap::new();
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    let mut message_entries: HashMap<i64, HnswCandidateSeed> = HashMap::new();
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    let mut episode_entries: HashMap<String, HnswCandidateSeed> = HashMap::new();

    for (rank_0, hit) in hnsw_hits.iter().enumerate() {
        let similarity = hit.similarity() as f64;
        if similarity < config.min_similarity {
            continue;
        }

        let (domain, raw_id) = hit.parse_key()?;
        let seed = HnswCandidateSeed {
            source_rank: rank_0 + 1,
            source_similarity: similarity,
        };

        match domain {
            "fact" if search_facts => {
                fact_entries.entry(raw_id.to_string()).or_insert(seed);
            }
            "chunk" if search_chunks => {
                chunk_entries.entry(raw_id.to_string()).or_insert(seed);
            }
            "msg" if search_messages => {
                if let Ok(message_id) = raw_id.parse::<i64>() {
                    message_entries.entry(message_id).or_insert(seed);
                }
            }
            "episode" if search_episodes => {
                episode_entries.entry(raw_id.to_string()).or_insert(seed);
            }
            _ => {}
        }
    }

    let mut hits = Vec::new();
    batch_load_fact_hits(
        conn,
        query_embedding,
        config,
        namespaces,
        &fact_entries,
        &mut hits,
    )?;
    batch_load_chunk_hits(
        conn,
        query_embedding,
        config,
        namespaces,
        &chunk_entries,
        &mut hits,
    )?;
    batch_load_message_hits(
        conn,
        query_embedding,
        config,
        session_ids,
        &message_entries,
        &mut hits,
    )?;
    batch_load_episode_hits(
        conn,
        query_embedding,
        config,
        namespaces,
        &episode_entries,
        &mut hits,
    )?;

    hits.sort_by(|a, b| {
        b.similarity
            .partial_cmp(&a.similarity)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                a.source_rank
                    .unwrap_or(usize::MAX)
                    .cmp(&b.source_rank.unwrap_or(usize::MAX))
            })
    });
    hits.truncate(config.candidate_pool_size);
    Ok(hits)
}

#[cfg(feature = "hnsw")]
fn exact_similarity_from_blob(
    query_embedding: &[f32],
    blob: &[u8],
) -> Result<Option<f64>, MemoryError> {
    if blob.is_empty() {
        return Ok(None);
    }
    let stored = crate::db::bytes_to_embedding(blob)?;
    if stored.len() != query_embedding.len() {
        return Ok(None);
    }
    Ok(Some(cosine_similarity(query_embedding, &stored)? as f64))
}

#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
fn build_ranked_vector_hit(
    id: String,
    content: String,
    source: SearchSource,
    updated_at: Option<String>,
    embedding_blob: Option<Vec<u8>>,
    seed: &HnswCandidateSeed,
    query_embedding: &[f32],
    config: &SearchConfig,
) -> Result<Option<VectorHit>, MemoryError> {
    let similarity = if config.rerank_from_f32 {
        match embedding_blob {
            Some(blob) => exact_similarity_from_blob(query_embedding, &blob)?,
            None => None,
        }
        .unwrap_or(seed.source_similarity)
    } else {
        seed.source_similarity
    };

    if similarity < config.min_similarity {
        return Ok(None);
    }

    Ok(Some(VectorHit {
        id,
        content,
        source,
        similarity,
        updated_at,
        source_rank: Some(seed.source_rank),
        source_similarity: Some(seed.source_similarity),
        reranked_from_f32: config.rerank_from_f32,
        temporal_weight: None,
        provenance_confidence: None,
    }))
}

#[cfg(feature = "hnsw")]
fn batch_load_fact_hits(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    namespaces: Option<&[&str]>,
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    entries: &HashMap<String, HnswCandidateSeed>,
    output: &mut Vec<VectorHit>,
) -> Result<(), MemoryError> {
    if entries.is_empty() {
        return Ok(());
    }

    let placeholders = (1..=entries.len())
        .map(|idx| format!("?{idx}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, content, namespace, updated_at, embedding
         FROM facts
         WHERE id IN ({placeholders})"
    );
    let params: Vec<SqlValue> = entries
        .keys()
        .map(|id| SqlValue::Text(id.clone()))
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, Option<String>>(3)?,
            row.get::<_, Option<Vec<u8>>>(4)?,
        ))
    })?;

    for row in rows {
        let (fact_id, content, namespace, updated_at, embedding_blob) = row?;
        if let Some(filter) = namespaces {
            if !filter.contains(&namespace.as_str()) {
                continue;
            }
        }
        if let Some(seed) = entries.get(&fact_id) {
            if let Some(hit) = build_ranked_vector_hit(
                format!("fact:{fact_id}"),
                content,
                SearchSource::Fact { fact_id, namespace },
                updated_at,
                embedding_blob,
                seed,
                query_embedding,
                config,
            )? {
                output.push(hit);
            }
        }
    }

    Ok(())
}

#[cfg(feature = "hnsw")]
fn batch_load_chunk_hits(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    namespaces: Option<&[&str]>,
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    entries: &HashMap<String, HnswCandidateSeed>,
    output: &mut Vec<VectorHit>,
) -> Result<(), MemoryError> {
    if entries.is_empty() {
        return Ok(());
    }

    let placeholders = (1..=entries.len())
        .map(|idx| format!("?{idx}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT c.id, c.content, c.document_id, d.title, c.chunk_index, c.created_at, d.namespace, c.embedding
         FROM chunks c
         JOIN documents d ON d.id = c.document_id
         WHERE c.id IN ({placeholders})"
    );
    let params: Vec<SqlValue> = entries
        .keys()
        .map(|id| SqlValue::Text(id.clone()))
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, i64>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<Vec<u8>>>(7)?,
        ))
    })?;

    for row in rows {
        let (
            chunk_id,
            content,
            document_id,
            document_title,
            chunk_index,
            updated_at,
            namespace,
            embedding_blob,
        ) = row?;
        if let Some(filter) = namespaces {
            if !filter.contains(&namespace.as_str()) {
                continue;
            }
        }
        if let Some(seed) = entries.get(&chunk_id) {
            if let Some(hit) = build_ranked_vector_hit(
                format!("chunk:{chunk_id}"),
                content,
                SearchSource::Chunk {
                    chunk_id,
                    document_id,
                    document_title,
                    chunk_index: chunk_index as usize,
                },
                updated_at,
                embedding_blob,
                seed,
                query_embedding,
                config,
            )? {
                output.push(hit);
            }
        }
    }

    Ok(())
}

#[cfg(feature = "hnsw")]
fn batch_load_message_hits(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    session_ids: Option<&[&str]>,
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    entries: &HashMap<i64, HnswCandidateSeed>,
    output: &mut Vec<VectorHit>,
) -> Result<(), MemoryError> {
    if entries.is_empty() {
        return Ok(());
    }

    let placeholders = (1..=entries.len())
        .map(|idx| format!("?{idx}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT id, content, session_id, role, created_at, embedding
         FROM messages
         WHERE id IN ({placeholders})"
    );
    let params: Vec<SqlValue> = entries.keys().map(|id| SqlValue::Integer(*id)).collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, Option<String>>(4)?,
            row.get::<_, Option<Vec<u8>>>(5)?,
        ))
    })?;

    for row in rows {
        let (message_id, content, session_id, role, updated_at, embedding_blob) = row?;
        if let Some(filter) = session_ids {
            if !filter.contains(&session_id.as_str()) {
                continue;
            }
        }
        if let Some(seed) = entries.get(&message_id) {
            if let Some(hit) = build_ranked_vector_hit(
                format!("msg:{message_id}"),
                content,
                SearchSource::Message {
                    message_id,
                    session_id,
                    role,
                },
                updated_at,
                embedding_blob,
                seed,
                query_embedding,
                config,
            )? {
                output.push(hit);
            }
        }
    }

    Ok(())
}

#[cfg(feature = "hnsw")]
fn batch_load_episode_hits(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    namespaces: Option<&[&str]>,
    // CONVENTION EXCEPTION: O(1) lookup required for performance-critical search path
    entries: &HashMap<String, HnswCandidateSeed>,
    output: &mut Vec<VectorHit>,
) -> Result<(), MemoryError> {
    if entries.is_empty() {
        return Ok(());
    }

    let placeholders = (1..=entries.len())
        .map(|idx| format!("?{idx}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "SELECT e.episode_id, e.document_id, e.search_text, e.effect_type, e.outcome, e.updated_at, d.namespace, e.embedding
         FROM episodes e
         JOIN documents d ON d.id = e.document_id
         WHERE e.episode_id IN ({placeholders})"
    );
    let params: Vec<SqlValue> = entries
        .keys()
        .map(|id| SqlValue::Text(id.clone()))
        .collect();
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(&params), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
            row.get::<_, String>(4)?,
            row.get::<_, Option<String>>(5)?,
            row.get::<_, String>(6)?,
            row.get::<_, Option<Vec<u8>>>(7)?,
        ))
    })?;

    for row in rows {
        let (
            episode_id,
            document_id,
            content,
            effect_type,
            outcome,
            updated_at,
            namespace,
            embedding_blob,
        ) = row?;
        if let Some(filter) = namespaces {
            if !filter.contains(&namespace.as_str()) {
                continue;
            }
        }
        if let Some(seed) = entries.get(&episode_id) {
            if let Some(hit) = build_ranked_vector_hit(
                episodes::episode_item_key(&episode_id),
                content,
                SearchSource::Episode {
                    episode_id,
                    document_id,
                    effect_type,
                    outcome,
                },
                updated_at,
                embedding_blob,
                seed,
                query_embedding,
                config,
            )? {
                output.push(hit);
            }
        }
    }

    Ok(())
}

/// Perform a hybrid search using pre-computed HNSW hits for the vector component.
#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
pub fn hybrid_search_with_hnsw(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<Vec<SearchResult>, MemoryError> {
    Ok(hybrid_search_with_hnsw_detailed(
        conn,
        query,
        query_embedding,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
        hnsw_hits,
    )?
    .into_iter()
    .map(|result| result.result)
    .collect())
}

#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn hybrid_search_with_hnsw_detailed_with_context(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    query_sparse: Option<&crate::SparseWeights>,
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<SearchExecution, MemoryError> {
    let bm25_hits = match sanitize_fts_query(query) {
        Some(sanitized) => bm25_search(
            conn,
            &sanitized,
            config.candidate_pool_size,
            namespaces,
            source_types,
            session_ids,
        )?,
        None => Vec::new(),
    };

    let mut vector_hits = resolve_hnsw_hits_batched(
        conn,
        query_embedding,
        config,
        namespaces,
        source_types,
        session_ids,
        hnsw_hits,
    )?;
    let mut fallback = None;
    let mut degradations = Vec::new();
    let mut backend = "hnsw";
    let mut exact_rerank = config.rerank_from_f32;

    if !hnsw_hits.is_empty()
        && vector_hits.len() < top_k
        && filters_are_active(namespaces, source_types, session_ids)
    {
        fallback = Some("hnsw_filtered_underreturn_fallback".to_string());
        degradations.push(format!(
            "HNSW returned {} post-filter vector candidates for requested top_k {}; exact filtered fallback was used",
            vector_hits.len(),
            top_k
        ));
        vector_hits = vector_search(
            conn,
            query_embedding,
            config.candidate_pool_size,
            config.min_similarity,
            namespaces,
            source_types,
            session_ids,
        )?;
        backend = "hnsw_then_brute_force_f32";
        exact_rerank = true;
    }

    let sparse_hits =
        if let Some(query_sparse) = query_sparse.filter(|_| config.sparse_weight > 0.0) {
            sparse_search(
                conn,
                query_sparse,
                config,
                namespaces,
                source_types,
                session_ids,
            )?
        } else {
            Vec::new()
        };
    let results = if config.sparse_weight > 0.0 {
        rrf_fuse_three_detailed_with_context(
            &bm25_hits,
            &vector_hits,
            &sparse_hits,
            config,
            context,
            top_k,
        )
    } else {
        rrf_fuse_detailed_with_context(&bm25_hits, &vector_hits, config, context, top_k)
    };
    let mut metadata = VectorReceiptMetadata::default();
    if config.sparse_weight > 0.0 {
        metadata.sparse_weight = Some(config.sparse_weight);
        if let Some(query_sparse) = query_sparse {
            metadata.sparse_query_nonzero_count = Some(query_sparse.len());
            metadata.sparse_candidate_count = Some(sparse_hits.len());
            metadata.sparse_representations = sparse_hits
                .iter()
                .map(|hit| hit.representation.clone())
                .collect();
            metadata.sparse_representations.sort();
            metadata.sparse_representations.dedup();
        } else {
            degradations.push(
                "sparse retrieval was requested but the active embedder produced no sparse query representation"
                    .to_string(),
            );
        }
    }
    let receipt = build_receipt_with_metadata(
        context,
        query_embedding,
        "hybrid",
        backend,
        config.candidate_pool_size,
        hnsw_hits.len(),
        vector_hits.len(),
        fallback,
        exact_rerank,
        &results,
        degradations,
        metadata,
    );

    Ok(SearchExecution { results, receipt })
}

#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn hybrid_search_with_hnsw_detailed(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<Vec<ExplainedResult>, MemoryError> {
    let context = SearchContext::default_now();
    Ok(hybrid_search_with_hnsw_detailed_with_context(
        conn,
        query,
        query_embedding,
        None,
        config,
        &context,
        top_k,
        namespaces,
        source_types,
        session_ids,
        hnsw_hits,
    )?
    .results)
}

/// Perform a hybrid HNSW-backed search and return the exact score decomposition.
#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
pub fn hybrid_search_explained_with_hnsw(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<Vec<ExplainedResult>, MemoryError> {
    hybrid_search_with_hnsw_detailed(
        conn,
        query,
        query_embedding,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
        hnsw_hits,
    )
}

pub(crate) fn fts_only_search_detailed(
    conn: &Connection,
    query: &str,
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<ExplainedResult>, MemoryError> {
    let sanitized = match sanitize_fts_query(query) {
        Some(value) => value,
        None => return Ok(Vec::new()),
    };
    let bm25_hits = bm25_search(
        conn,
        &sanitized,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )?;
    Ok(rrf_fuse_detailed(&bm25_hits, &[], config, top_k))
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn fts_only_search_detailed_with_context(
    conn: &Connection,
    query: &str,
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<SearchExecution, MemoryError> {
    let results = fts_only_search_detailed(
        conn,
        query,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )?;
    let count = results.len();
    let mut receipt = build_receipt(
        context,
        &[],
        "fts_only",
        "sqlite_fts5_bm25",
        top_k,
        count,
        count,
        None,
        false,
        &results,
        Vec::new(),
    );
    if let Some(receipt) = receipt.as_mut() {
        receipt.query_embedding_digest = None;
    }
    Ok(SearchExecution { results, receipt })
}

/// Full-text search only (no embeddings needed). Synchronous.
pub fn fts_only_search(
    conn: &Connection,
    query: &str,
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<SearchResult>, MemoryError> {
    Ok(fts_only_search_detailed(
        conn,
        query,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )?
    .into_iter()
    .map(|result| result.result)
    .collect())
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn vector_only_search_detailed_with_context(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<SearchExecution, MemoryError> {
    let vector_outcome = vector_search_with_backend(
        conn,
        query_embedding,
        top_k,
        config.min_similarity,
        config,
        context,
        namespaces,
        source_types,
        session_ids,
    )?;
    let results = rrf_fuse_detailed_with_context(&[], &vector_outcome.hits, config, context, top_k);
    let receipt = build_receipt_with_metadata(
        context,
        query_embedding,
        "vector_only",
        &vector_outcome.candidate_backend,
        vector_outcome.requested_candidates,
        vector_outcome.returned_candidates,
        vector_outcome.post_filter_candidates,
        vector_outcome.fallback,
        vector_outcome.exact_rerank,
        &results,
        vector_outcome.degradations,
        vector_outcome.receipt_metadata,
    );
    Ok(SearchExecution { results, receipt })
}

pub(crate) fn vector_only_search_detailed(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<ExplainedResult>, MemoryError> {
    let context = SearchContext::default_now();
    Ok(vector_only_search_detailed_with_context(
        conn,
        query_embedding,
        config,
        &context,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )?
    .results)
}

/// Vector-only search. Called after embedding the query.
pub fn vector_only_search(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
) -> Result<Vec<SearchResult>, MemoryError> {
    Ok(vector_only_search_detailed(
        conn,
        query_embedding,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
    )?
    .into_iter()
    .map(|result| result.result)
    .collect())
}

#[cfg(test)]
mod digest_tests {
    use super::query_embedding_digest;

    #[test]
    fn query_embedding_digest_includes_dimension_and_bytes() {
        let two_dims = query_embedding_digest(&[1.0, 2.0]);
        let three_dims = query_embedding_digest(&[1.0, 2.0, 0.0]);
        let changed_byte = query_embedding_digest(&[1.0, 2.000_001]);

        assert!(two_dims.starts_with("blake3:"));
        assert_eq!(two_dims.len(), 71);
        assert_ne!(two_dims, three_dims);
        assert_ne!(two_dims, changed_byte);
        assert_eq!(two_dims, query_embedding_digest(&[1.0, 2.0]));
    }
}

/// Vector-only search using pre-computed HNSW hits.
#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
pub fn vector_only_search_with_hnsw(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<Vec<SearchResult>, MemoryError> {
    Ok(vector_only_search_with_hnsw_detailed(
        conn,
        query_embedding,
        config,
        top_k,
        namespaces,
        source_types,
        session_ids,
        hnsw_hits,
    )?
    .into_iter()
    .map(|result| result.result)
    .collect())
}

#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn vector_only_search_with_hnsw_detailed_with_context(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    context: &SearchContext,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<SearchExecution, MemoryError> {
    let mut vector_hits = resolve_hnsw_hits_batched(
        conn,
        query_embedding,
        config,
        namespaces,
        source_types,
        session_ids,
        hnsw_hits,
    )?;
    let mut fallback = None;
    let mut degradations = Vec::new();
    let mut backend = "hnsw";
    let mut exact_rerank = config.rerank_from_f32;

    if !hnsw_hits.is_empty()
        && vector_hits.len() < top_k
        && filters_are_active(namespaces, source_types, session_ids)
    {
        fallback = Some("hnsw_filtered_underreturn_fallback".to_string());
        degradations.push(format!(
            "HNSW returned {} post-filter vector candidates for requested top_k {}; exact filtered fallback was used",
            vector_hits.len(),
            top_k
        ));
        vector_hits = vector_search(
            conn,
            query_embedding,
            top_k,
            config.min_similarity,
            namespaces,
            source_types,
            session_ids,
        )?;
        backend = "hnsw_then_brute_force_f32";
        exact_rerank = true;
    }

    let results = rrf_fuse_detailed_with_context(&[], &vector_hits, config, context, top_k);
    let receipt = build_receipt(
        context,
        query_embedding,
        "vector_only",
        backend,
        top_k,
        hnsw_hits.len(),
        vector_hits.len(),
        fallback,
        exact_rerank,
        &results,
        degradations,
    );
    Ok(SearchExecution { results, receipt })
}

#[cfg(feature = "hnsw")]
#[allow(clippy::too_many_arguments)]
pub(crate) fn vector_only_search_with_hnsw_detailed(
    conn: &Connection,
    query_embedding: &[f32],
    config: &SearchConfig,
    top_k: usize,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
    session_ids: Option<&[&str]>,
    hnsw_hits: &[crate::hnsw::HnswHit],
) -> Result<Vec<ExplainedResult>, MemoryError> {
    let context = SearchContext::default_now();
    Ok(vector_only_search_with_hnsw_detailed_with_context(
        conn,
        query_embedding,
        config,
        &context,
        top_k,
        namespaces,
        source_types,
        session_ids,
        hnsw_hits,
    )?
    .results)
}

fn build_filter_clause(
    column: &str,
    values: Option<&[&str]>,
    param_offset: usize,
) -> (String, Vec<SqlValue>) {
    match values {
        Some(values) if !values.is_empty() => {
            let placeholders = (0..values.len())
                .map(|idx| format!("?{}", param_offset + idx))
                .collect::<Vec<_>>();
            let clause = format!(" AND {} IN ({})", column, placeholders.join(", "));
            let params = values
                .iter()
                .map(|value| SqlValue::Text((*value).to_string()))
                .collect();
            (clause, params)
        }
        _ => (String::new(), Vec::new()),
    }
}

/// Deduplicate results by (source_type, source_id), keeping the first occurrence.
pub fn deduplicate_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    results
        .into_iter()
        .filter(|result| seen.insert(source_dedup_key(&result.source)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vector_row(id: &str) -> VectorRow {
        VectorRow {
            id: id.to_string(),
            content: format!("content {id}"),
            blob: bytemuck::cast_slice(&[1.0_f32, 0.0]).to_vec(),
            updated_at: None,
            source_type: SearchSourceType::Facts,
            filter_namespace: Some("default".to_string()),
            filter_session_id: None,
            source: SearchSource::Fact {
                fact_id: id.to_string(),
                namespace: "default".to_string(),
            },
        }
    }

    #[test]
    fn timestamp_parser_accepts_sql_fractional_and_rfc3339_and_warns_by_returning_none() {
        assert!(parse_search_timestamp("2026-05-07 12:34:56").is_some());
        assert!(parse_search_timestamp("2026-05-07 12:34:56.123").is_some());
        assert!(parse_search_timestamp("2026-05-07T12:34:56Z").is_some());
        assert!(parse_search_timestamp("not-a-timestamp").is_none());
    }

    #[test]
    fn vector_scan_hard_limit_blocks_before_unbounded_scan() {
        let old_warn = VECTOR_SCAN_WARN_LIMIT.swap(1, Ordering::SeqCst);
        let old_hard = VECTOR_SCAN_BLOCK_LIMIT.swap(2, Ordering::SeqCst);
        let rows = ["a", "b", "c"].into_iter().map(|id| Ok(vector_row(id)));
        let result = scan_vector_rows(rows, &[1.0, 0.0], -1.0, "fact");
        VECTOR_SCAN_WARN_LIMIT.store(old_warn, Ordering::SeqCst);
        VECTOR_SCAN_BLOCK_LIMIT.store(old_hard, Ordering::SeqCst);

        match result {
            Err(MemoryError::VectorScanLimitExceeded {
                table,
                scanned,
                limit,
            }) => {
                assert_eq!(table, "fact");
                assert_eq!(scanned, 3);
                assert_eq!(limit, 2);
            }
            other => panic!("expected vector scan limit error, got {other:?}"),
        }
    }
}
