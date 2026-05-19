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
#[cfg(feature = "turbo-quant-codec")]
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
}

impl RrfCandidate {
    fn explained(self, config: &SearchConfig, context: &SearchContext) -> ExplainedResult {
        let bm25_contribution = self
            .bm25_rank
            .map(|rank| config.bm25_weight / (config.rrf_k + rank as f64));
        let vector_contribution = self
            .vector_rank
            .map(|rank| config.vector_weight / (config.rrf_k + rank as f64));
        let best_rank = match (self.bm25_rank, self.vector_rank) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) | (None, Some(a)) => Some(a),
            (None, None) => None,
        };
        let recency_score =
            recency_contribution(config, context, self.updated_at.as_deref(), best_rank);
        let rrf_score = bm25_contribution.unwrap_or(0.0)
            + vector_contribution.unwrap_or(0.0)
            + recency_score.unwrap_or(0.0);

        let breakdown = ScoreBreakdown {
            rrf_score,
            bm25_score: self.bm25_score,
            vector_score: self.vector_score,
            recency_score,
            bm25_rank: self.bm25_rank,
            vector_rank: self.vector_rank,
            vector_source_rank: self.vector_source_rank,
            vector_source_score: self.vector_source_score,
            bm25_contribution,
            vector_contribution,
            vector_reranked_from_f32: self.vector_reranked_from_f32,
            bm25_weight: config.bm25_weight,
            vector_weight: config.vector_weight,
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
            "SELECT fm.fact_id, f.content, f.namespace, bm25(facts_fts) AS score, f.updated_at
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
            Ok(Bm25Hit {
                id: format!("fact:{fact_id}"),
                content,
                source: SearchSource::Fact { fact_id, namespace },
                raw_score,
                updated_at,
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
    }
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

#[cfg(feature = "turbo-quant-codec")]
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

#[cfg(feature = "turbo-quant-codec")]
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

fn rrf_fuse_detailed_with_context(
    bm25_hits: &[Bm25Hit],
    vector_hits: &[VectorHit],
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
        vector_artifact_manifest_digest: metadata.vector_artifact_manifest_digest,
        artifact_generation_id: metadata.artifact_generation_id,
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
        fallback_reason: fallback.clone(),
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

#[allow(clippy::too_many_arguments)]
pub(crate) fn hybrid_search_detailed_with_context(
    conn: &Connection,
    query: &str,
    query_embedding: &[f32],
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

    let vector_outcome = vector_search_with_backend(
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

    let results =
        rrf_fuse_detailed_with_context(&bm25_hits, &vector_outcome.hits, config, context, top_k);
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
        vector_outcome.receipt_metadata,
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
    Ok(hybrid_search_detailed(
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
    .collect())
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

    let results = rrf_fuse_detailed_with_context(&bm25_hits, &vector_hits, config, context, top_k);
    let receipt = build_receipt(
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
