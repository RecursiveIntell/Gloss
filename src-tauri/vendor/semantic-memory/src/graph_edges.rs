//! First-class stored graph edges — durable, typed relationships between
//! any two nodes in the knowledge graph.
//!
//! Unlike the derived edges in [`crate::graph`] (which are computed on-the-fly
//! from SQLite state), these edges are explicitly created by agents or users
//! and persisted in the `graph_edges` table (migration V27).
//!
//! ## Authority
//!
//! `semantic-memory` is authoritative for graph edge state. These edges are
//! first-class knowledge relationships, not projection lineage (which lives in
//! `derivation_edges`).
//!
//! ## Bitemporal semantics
//!
//! Edges are append-only. Invalidation is a separate operation that sets
//! `is_invalidated = 1` with a timestamp and reason — the original row is
//! never deleted. This follows the append-plus-supersession doctrine.
//!
//! ## Idempotent insertion
//!
//! Each edge carries a `content_digest` (blake3 of source + target + edge_type
//! + weight + metadata). Inserting the same edge twice returns the existing
//! row ID without creating a duplicate.

use crate::error::MemoryError;
use crate::types::{GraphEdge, GraphEdgeType};
use chrono::{DateTime, NaiveDateTime, Utc};
use rusqlite::{params, Connection};
use std::collections::HashSet;

/// Canonical timestamp format used for all bitemporal graph-edge columns.
///
/// Fixed-width, lexicographically sortable, and unambiguous.
const CANONICAL_TS_FMT: &str = "%Y-%m-%d %H:%M:%S%.6f";

/// Normalize a caller-provided timestamp into the canonical sortable format.
///
/// Accepts:
/// - `%Y-%m-%d %H:%M:%S%.6f` (canonical)
/// - `%Y-%m-%d %H:%M:%S` (SQLite default)
/// - RFC3339 / ISO 8601 (`2026-01-01T00:00:00Z`)
///
/// Returns `None` if the input cannot be parsed.
pub(crate) fn canonicalize_timestamp(s: &str) -> Option<String> {
    // Already canonical?
    if NaiveDateTime::parse_from_str(s, CANONICAL_TS_FMT).is_ok() {
        return Some(s.to_string());
    }
    // SQLite datetime format with seconds precision.
    if let Ok(dt) = NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.format(CANONICAL_TS_FMT).to_string());
    }
    // RFC3339 / ISO 8601.
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Some(dt.naive_utc().format(CANONICAL_TS_FMT).to_string());
    }
    None
}

/// Normalize a timestamp for storage. If parsing fails, returns the
/// original input unchanged so the caller can decide whether to reject it.
fn normalize_or_pass_through(s: &str) -> String {
    canonicalize_timestamp(s).unwrap_or_else(|| s.to_string())
}

/// A stored graph edge row.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct StoredGraphEdge {
    /// Unique edge ID (UUID v4).
    pub id: String,
    /// Source node ID (prefixed, e.g. `fact:<uuid>`).
    pub source: String,
    /// Target node ID (prefixed, e.g. `fact:<uuid>`).
    pub target: String,
    /// Serialized GraphEdgeType JSON.
    pub edge_type: String,
    /// Deserialized edge type.
    #[serde(skip)]
    pub edge_type_parsed: Option<GraphEdgeType>,
    /// Edge weight.
    pub weight: f64,
    /// Optional metadata JSON.
    pub metadata: Option<String>,
    /// Content digest (blake3 of source+target+edge_type+weight+metadata).
    pub content_digest: String,
    /// Recorded timestamp (microsecond precision).
    pub recorded_at: String,
    /// Whether this edge has been invalidated.
    pub is_invalidated: bool,
    /// Invalidation timestamp.
    pub invalidated_at: Option<String>,
    /// Invalidation reason.
    pub invalidation_reason: Option<String>,
    /// Domain/business time this edge became valid.
    pub valid_time: Option<String>,
    /// System time this edge version was recorded.
    pub recorded_time: Option<String>,
}

/// Parameters for creating a graph edge.
#[derive(Debug, Clone)]
pub struct AddGraphEdgeParams {
    /// Source node ID (prefixed, e.g. `fact:<uuid>`, `namespace:<name>`).
    pub source: String,
    /// Target node ID (prefixed).
    pub target: String,
    /// Edge type (semantic, temporal, causal, entity).
    pub edge_type: GraphEdgeType,
    /// Edge weight (interpretation depends on edge_type).
    pub weight: f64,
    /// Optional metadata.
    pub metadata: Option<serde_json::Value>,
    /// Optional domain/business time. Defaults to insertion time.
    pub valid_time: Option<String>,
    /// Optional system recorded time. Defaults to insertion time.
    pub recorded_time: Option<String>,
}

/// Insert a graph edge. Idempotent on content_digest.
pub(crate) fn insert_graph_edge(
    conn: &Connection,
    params: &AddGraphEdgeParams,
) -> Result<StoredGraphEdge, MemoryError> {
    let edge_type_json = serde_json::to_string(&params.edge_type)
        .map_err(|e| MemoryError::Other(format!("failed to serialize edge_type: {e}")))?;

    let metadata_json = match &params.metadata {
        Some(v) => Some(
            serde_json::to_string(v)
                .map_err(|e| MemoryError::Other(format!("failed to serialize metadata: {e}")))?,
        ),
        None => None,
    };

    let content_digest = compute_edge_digest(
        &params.source,
        &params.target,
        &edge_type_json,
        params.weight,
        &metadata_json,
        params.valid_time.as_deref(),
        params.recorded_time.as_deref(),
    );

    // Check for existing edge with same digest (idempotent).
    let existing: Option<(String, String)> = conn
        .query_row(
            "SELECT id, recorded_at FROM graph_edges WHERE content_digest = ?1 AND is_invalidated = 0",
            params![&content_digest],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .ok();

    if let Some((id, recorded_at)) = existing {
        return Ok(StoredGraphEdge {
            id,
            source: params.source.clone(),
            target: params.target.clone(),
            edge_type: edge_type_json,
            edge_type_parsed: Some(params.edge_type.clone()),
            weight: params.weight,
            metadata: metadata_json,
            content_digest,
            recorded_at: recorded_at.clone(),
            is_invalidated: false,
            invalidated_at: None,
            invalidation_reason: None,
            valid_time: params
                .valid_time
                .as_deref()
                .map(normalize_or_pass_through)
                .or_else(|| Some(recorded_at.clone())),
            recorded_time: params
                .recorded_time
                .as_deref()
                .map(normalize_or_pass_through)
                .or_else(|| Some(recorded_at.clone())),
        });
    }

    let id = uuid::Uuid::new_v4().to_string();
    let recorded_at = Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string();
    let valid_time = params
        .valid_time
        .as_deref()
        .map(normalize_or_pass_through)
        .unwrap_or_else(|| recorded_at.clone());
    let recorded_time = params
        .recorded_time
        .as_deref()
        .map(normalize_or_pass_through)
        .unwrap_or_else(|| recorded_at.clone());

    conn.execute(
        "INSERT INTO graph_edges (id, source, target, edge_type, weight, metadata, content_digest, recorded_at, valid_time, recorded_time)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            &id,
            &params.source,
            &params.target,
            &edge_type_json,
            params.weight,
            metadata_json.as_deref(),
            &content_digest,
            &recorded_at,
            &valid_time,
            &recorded_time,
        ],
    )
    .map_err(|e| MemoryError::Database(e))?;

    Ok(StoredGraphEdge {
        id,
        source: params.source.clone(),
        target: params.target.clone(),
        edge_type: edge_type_json,
        edge_type_parsed: Some(params.edge_type.clone()),
        weight: params.weight,
        metadata: metadata_json,
        content_digest,
        recorded_at,
        is_invalidated: false,
        invalidated_at: None,
        invalidation_reason: None,
        valid_time: Some(valid_time),
        recorded_time: Some(recorded_time),
    })
}

/// List all stored graph edges involving a given node (either as source or
/// target), excluding invalidated edges.
pub(crate) fn list_graph_edges_for_node(
    conn: &Connection,
    node_id: &str,
) -> Result<Vec<StoredGraphEdge>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source, target, edge_type, weight, metadata, content_digest, recorded_at,
                is_invalidated, invalidated_at, invalidation_reason, valid_time, recorded_time
         FROM graph_edges
         WHERE (source = ?1 OR target = ?1) AND is_invalidated = 0
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt
        .query_map(params![node_id], |row| {
            Ok(StoredGraphEdge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                edge_type: row.get(3)?,
                edge_type_parsed: None,
                weight: row.get(4)?,
                metadata: row.get(5)?,
                content_digest: row.get(6)?,
                recorded_at: row.get(7)?,
                is_invalidated: row.get::<_, i64>(8)? != 0,
                invalidated_at: row.get(9)?,
                invalidation_reason: row.get(10)?,
                valid_time: row.get(11)?,
                recorded_time: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// List ALL stored graph edges, excluding invalidated ones.
pub(crate) fn list_all_graph_edges(conn: &Connection) -> Result<Vec<StoredGraphEdge>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source, target, edge_type, weight, metadata, content_digest, recorded_at,
                is_invalidated, invalidated_at, invalidation_reason, valid_time, recorded_time
         FROM graph_edges
         WHERE is_invalidated = 0
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(StoredGraphEdge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                edge_type: row.get(3)?,
                edge_type_parsed: None,
                weight: row.get(4)?,
                metadata: row.get(5)?,
                content_digest: row.get(6)?,
                recorded_at: row.get(7)?,
                is_invalidated: row.get::<_, i64>(8)? != 0,
                invalidated_at: row.get(9)?,
                invalidation_reason: row.get(10)?,
                valid_time: row.get(11)?,
                recorded_time: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// List graph edges involving a node that were valid as of a domain time and
/// known as of a recorded/system time. Unlike the live list APIs, this can
/// return edges that are now invalidated if the invalidation happened after
/// `as_of_recorded_time`.
pub(crate) fn list_graph_edges_for_node_as_of(
    conn: &Connection,
    node_id: &str,
    as_of_valid_time: &str,
    as_of_recorded_time: &str,
) -> Result<Vec<StoredGraphEdge>, MemoryError> {
    // Normalize as-of inputs so mixed SQL/RFC3339 formats compare correctly.
    let as_of_valid_time = canonicalize_timestamp(as_of_valid_time).ok_or_else(|| {
        MemoryError::Other(format!(
            "invalid as_of_valid_time timestamp: {as_of_valid_time}"
        ))
    })?;
    let as_of_recorded_time = canonicalize_timestamp(as_of_recorded_time).ok_or_else(|| {
        MemoryError::Other(format!(
            "invalid as_of_recorded_time timestamp: {as_of_recorded_time}"
        ))
    })?;

    let mut stmt = conn.prepare(
        "SELECT id, source, target, edge_type, weight, metadata, content_digest, recorded_at,
                is_invalidated, invalidated_at, invalidation_reason, valid_time, recorded_time
         FROM graph_edges
         WHERE (source = ?1 OR target = ?1)
           AND COALESCE(valid_time, recorded_at) <= ?2
           AND COALESCE(recorded_time, recorded_at) <= ?3
           AND (is_invalidated = 0 OR invalidated_at IS NULL OR invalidated_at > ?3)
         ORDER BY COALESCE(recorded_time, recorded_at) ASC, id ASC",
    )?;
    let rows = stmt
        .query_map(
            params![node_id, as_of_valid_time, as_of_recorded_time],
            |row| {
                Ok(StoredGraphEdge {
                    id: row.get(0)?,
                    source: row.get(1)?,
                    target: row.get(2)?,
                    edge_type: row.get(3)?,
                    edge_type_parsed: None,
                    weight: row.get(4)?,
                    metadata: row.get(5)?,
                    content_digest: row.get(6)?,
                    recorded_at: row.get(7)?,
                    is_invalidated: row.get::<_, i64>(8)? != 0,
                    invalidated_at: row.get(9)?,
                    invalidation_reason: row.get(10)?,
                    valid_time: row.get(11)?,
                    recorded_time: row.get(12)?,
                })
            },
        )?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

/// List graph edges within N hops of the given seed node IDs.
///
/// Performs a BFS expansion from the seeds, loading only edges that
/// connect nodes already in the visited set. This avoids loading the
/// entire graph when only a local neighborhood is needed (e.g. discord
/// search, factor graph, graph_path).
///
/// `max_hops` controls the BFS depth. `max_nodes` caps the total nodes
/// visited to prevent runaway expansion on hub nodes.
pub(crate) fn list_graph_edges_for_neighborhood(
    conn: &Connection,
    seed_ids: &[String],
    max_hops: usize,
    max_nodes: usize,
) -> Result<Vec<StoredGraphEdge>, MemoryError> {
    if seed_ids.is_empty() || max_hops == 0 {
        return Ok(Vec::new());
    }

    let mut visited: HashSet<String> = seed_ids.iter().cloned().collect();
    let mut all_edges: Vec<StoredGraphEdge> = Vec::new();
    let mut frontier: Vec<String> = seed_ids.iter().cloned().collect();

    for _hop in 0..max_hops {
        if frontier.is_empty() || visited.len() >= max_nodes {
            break;
        }

        let mut next_frontier: Vec<String> = Vec::new();

        for node_id in &frontier {
            let edges = list_graph_edges_for_node(conn, node_id)?;
            for edge in edges {
                // Track both endpoints
                let other = if edge.source == *node_id {
                    &edge.target
                } else {
                    &edge.source
                };

                if !visited.contains(other) {
                    visited.insert(other.clone());
                    next_frontier.push(other.clone());
                }

                // Dedup edges by id
                if !all_edges.iter().any(|e: &StoredGraphEdge| e.id == edge.id) {
                    all_edges.push(edge);
                }
            }
        }

        frontier = next_frontier;
        if visited.len() >= max_nodes {
            break;
        }
    }

    // Sort by recorded_at for deterministic ordering
    all_edges.sort_by(|a, b| a.recorded_at.cmp(&b.recorded_at));
    Ok(all_edges)
}

/// Invalidate a graph edge by ID. Append-only — does not delete the row.
pub(crate) fn invalidate_graph_edge(
    conn: &Connection,
    edge_id: &str,
    reason: &str,
) -> Result<(), MemoryError> {
    let invalidated_at =
        normalize_or_pass_through(&Utc::now().format("%Y-%m-%d %H:%M:%S%.6f").to_string());
    let count = conn.execute(
        "UPDATE graph_edges SET is_invalidated = 1, invalidated_at = ?1, invalidation_reason = ?2
         WHERE id = ?3 AND is_invalidated = 0",
        params![&invalidated_at, reason, edge_id],
    )?;
    if count == 0 {
        return Err(MemoryError::Other(format!(
            "graph edge {} not found or already invalidated",
            edge_id
        )));
    }
    Ok(())
}

/// Load stored graph edges for a node and convert them to GraphEdge objects
/// for the derived graph view. Only non-invalidated edges are included.
#[allow(dead_code)] // public API — used by external consumers, not internally
pub(crate) fn stored_edges_for_node(
    conn: &Connection,
    node_id: &str,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let rows = list_graph_edges_for_node(conn, node_id)?;
    let mut edges = Vec::new();
    for row in rows {
        let edge_type: GraphEdgeType = serde_json::from_str(&row.edge_type)
            .map_err(|e| MemoryError::Other(format!("failed to deserialize edge_type: {e}")))?;
        let metadata: Option<serde_json::Value> =
            match &row.metadata {
                Some(s) => Some(serde_json::from_str(s).map_err(|e| {
                    MemoryError::Other(format!("failed to deserialize metadata: {e}"))
                })?),
                None => None,
            };
        edges.push(GraphEdge {
            source: row.source,
            target: row.target,
            edge_type,
            weight: row.weight,
            metadata,
        });
    }
    Ok(edges)
}

/// Load stored outgoing edges for a node (where node is the source).
/// Only non-invalidated edges are included.
pub(crate) fn stored_outgoing_edges(
    conn: &Connection,
    node_id: &str,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source, target, edge_type, weight, metadata, content_digest, recorded_at,
                is_invalidated, invalidated_at, invalidation_reason, valid_time, recorded_time
         FROM graph_edges
         WHERE source = ?1 AND is_invalidated = 0
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt
        .query_map(params![node_id], |row| {
            Ok(StoredGraphEdge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                edge_type: row.get(3)?,
                edge_type_parsed: None,
                weight: row.get(4)?,
                metadata: row.get(5)?,
                content_digest: row.get(6)?,
                recorded_at: row.get(7)?,
                is_invalidated: row.get::<_, i64>(8)? != 0,
                invalidated_at: row.get(9)?,
                invalidation_reason: row.get(10)?,
                valid_time: row.get(11)?,
                recorded_time: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows_to_graph_edges(rows)
}

/// Load stored incoming edges for a node (where node is the target).
/// Only non-invalidated edges are included.
pub(crate) fn stored_incoming_edges(
    conn: &Connection,
    node_id: &str,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, source, target, edge_type, weight, metadata, content_digest, recorded_at,
                is_invalidated, invalidated_at, invalidation_reason, valid_time, recorded_time
         FROM graph_edges
         WHERE target = ?1 AND is_invalidated = 0
         ORDER BY recorded_at ASC",
    )?;
    let rows = stmt
        .query_map(params![node_id], |row| {
            Ok(StoredGraphEdge {
                id: row.get(0)?,
                source: row.get(1)?,
                target: row.get(2)?,
                edge_type: row.get(3)?,
                edge_type_parsed: None,
                weight: row.get(4)?,
                metadata: row.get(5)?,
                content_digest: row.get(6)?,
                recorded_at: row.get(7)?,
                is_invalidated: row.get::<_, i64>(8)? != 0,
                invalidated_at: row.get(9)?,
                invalidation_reason: row.get(10)?,
                valid_time: row.get(11)?,
                recorded_time: row.get(12)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    rows_to_graph_edges(rows)
}

/// Convert StoredGraphEdge rows into GraphEdge objects.
fn rows_to_graph_edges(rows: Vec<StoredGraphEdge>) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut edges = Vec::new();
    for row in rows {
        let edge_type: GraphEdgeType = serde_json::from_str(&row.edge_type)
            .map_err(|e| MemoryError::Other(format!("failed to deserialize edge_type: {e}")))?;
        let metadata: Option<serde_json::Value> =
            match &row.metadata {
                Some(s) => Some(serde_json::from_str(s).map_err(|e| {
                    MemoryError::Other(format!("failed to deserialize metadata: {e}"))
                })?),
                None => None,
            };
        edges.push(GraphEdge {
            source: row.source,
            target: row.target,
            edge_type,
            weight: row.weight,
            metadata,
        });
    }
    Ok(edges)
}
/// Count total stored edges (non-invalidated).
pub(crate) fn count_graph_edges(conn: &Connection) -> Result<usize, MemoryError> {
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM graph_edges WHERE is_invalidated = 0",
            [],
            |row| row.get(0),
        )
        .map_err(|e| MemoryError::Database(e))?;
    Ok(count as usize)
}

fn compute_edge_digest(
    source: &str,
    target: &str,
    edge_type_json: &str,
    weight: f64,
    metadata_json: &Option<String>,
    explicit_valid_time: Option<&str>,
    explicit_recorded_time: Option<&str>,
) -> String {
    let mut builder = stack_ids::DigestBuilder::new();
    builder.update(source.as_bytes());
    builder.update(target.as_bytes());
    builder.update(edge_type_json.as_bytes());
    builder.update(&weight.to_le_bytes());
    if let Some(meta) = metadata_json {
        builder.update(meta.as_bytes());
    }
    if let Some(valid_time) = explicit_valid_time {
        builder.update(valid_time.as_bytes());
    }
    if let Some(recorded_time) = explicit_recorded_time {
        builder.update(recorded_time.as_bytes());
    }
    builder.finalize().hex().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::run_migrations;
    use rusqlite::Connection;
    use serde_json::json;

    fn migrated_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        run_migrations(&conn).unwrap();
        conn
    }

    #[test]
    fn graph_edges_have_bitemporal_columns_after_migration() {
        let conn = migrated_conn();
        let columns: Vec<String> = conn
            .prepare("PRAGMA table_info(graph_edges)")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();

        assert!(columns.contains(&"valid_time".to_string()));
        assert!(columns.contains(&"recorded_time".to_string()));
    }

    #[test]
    fn graph_edges_as_of_filters_valid_and_recorded_time() {
        let conn = migrated_conn();
        let old_edge = AddGraphEdgeParams {
            source: "fact:a".to_string(),
            target: "fact:b".to_string(),
            edge_type: GraphEdgeType::Entity {
                relation: "supports".to_string(),
            },
            weight: 1.0,
            metadata: Some(json!({"version": "old"})),
            valid_time: Some("2026-01-01T00:00:00Z".to_string()),
            recorded_time: Some("2026-01-02T00:00:00Z".to_string()),
        };
        let future_edge = AddGraphEdgeParams {
            source: "fact:a".to_string(),
            target: "fact:c".to_string(),
            edge_type: GraphEdgeType::Entity {
                relation: "supports".to_string(),
            },
            weight: 1.0,
            metadata: Some(json!({"version": "future"})),
            valid_time: Some("2026-03-01T00:00:00Z".to_string()),
            recorded_time: Some("2026-03-02T00:00:00Z".to_string()),
        };

        insert_graph_edge(&conn, &old_edge).unwrap();
        insert_graph_edge(&conn, &future_edge).unwrap();

        let visible = list_graph_edges_for_node_as_of(
            &conn,
            "fact:a",
            "2026-02-01T00:00:00Z",
            "2026-02-02T00:00:00Z",
        )
        .unwrap();

        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].target, "fact:b");
        // Stored RFC3339 values must be canonicalized to fixed-width SQL format.
        assert_eq!(
            visible[0].valid_time.as_deref(),
            Some("2026-01-01 00:00:00.000000")
        );
        assert_eq!(
            visible[0].recorded_time.as_deref(),
            Some("2026-01-02 00:00:00.000000")
        );
    }

    #[test]
    fn graph_edges_as_of_handles_mixed_timestamp_formats() {
        // Edge inserted with canonical SQL timestamp; query with RFC3339.
        let conn = migrated_conn();
        let edge = AddGraphEdgeParams {
            source: "fact:x".to_string(),
            target: "fact:y".to_string(),
            edge_type: GraphEdgeType::Semantic {
                cosine_similarity: 0.9,
            },
            weight: 1.0,
            metadata: None,
            valid_time: Some("2026-01-01 00:00:00.000000".to_string()),
            recorded_time: Some("2026-01-01 00:00:00.000000".to_string()),
        };
        insert_graph_edge(&conn, &edge).unwrap();

        let visible = list_graph_edges_for_node_as_of(
            &conn,
            "fact:x",
            "2026-01-01T23:59:59Z",
            "2026-01-01T23:59:59Z",
        )
        .unwrap();
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].target, "fact:y");
    }

    #[test]
    fn graph_edges_as_of_rejects_invalid_timestamp() {
        let conn = migrated_conn();
        let edge = AddGraphEdgeParams {
            source: "fact:x".to_string(),
            target: "fact:y".to_string(),
            edge_type: GraphEdgeType::Semantic {
                cosine_similarity: 0.9,
            },
            weight: 1.0,
            metadata: None,
            valid_time: None,
            recorded_time: None,
        };
        insert_graph_edge(&conn, &edge).unwrap();

        let err = list_graph_edges_for_node_as_of(
            &conn,
            "fact:x",
            "not-a-timestamp",
            "2026-01-01T00:00:00Z",
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid as_of_valid_time"));
    }
}
