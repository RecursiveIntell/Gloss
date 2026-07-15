//! Derived graph view over the authoritative SQLite state.

use crate::db;
use crate::episodes;
use crate::search;
use crate::types::{GraphDirection, GraphEdge, GraphEdgeType, GraphView};
use crate::{MemoryError, MemoryStoreInner};
use rusqlite::{params, Connection};
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Instant;

const SEMANTIC_EDGE_LIMIT: usize = 5;
const DEFAULT_MAX_GRAPH_NODES: usize = 500;
const DEFAULT_MAX_EDGES_PER_NODE: usize = 50;
const DEFAULT_GRAPH_TRAVERSAL_DEADLINE_MS: u64 = 5_000;

/// Maximum number of namespace member edges (facts + documents) to expand
/// during a single BFS visit.  Without this cap, a namespace with hundreds
/// of facts causes the BFS to visit every one of them, and each fact visit
/// can trigger expensive derived-edge computation.  50 is enough to bridge
/// through a namespace without exploding the search space.
const NAMESPACE_EDGE_BFS_CAP: usize = 50;

pub(crate) fn graph_view(inner: Arc<MemoryStoreInner>) -> Arc<dyn GraphView> {
    Arc::new(StoreGraphView { inner })
}

struct StoreGraphView {
    inner: Arc<MemoryStoreInner>,
}

impl GraphView for StoreGraphView {
    fn neighbors(
        &self,
        node_id: &str,
        direction: GraphDirection,
        max_depth: usize,
    ) -> Result<Vec<GraphEdge>, MemoryError> {
        if max_depth == 0 {
            return Ok(Vec::new());
        }

        let node_id = node_id.to_string();
        let min_similarity = self.inner.config.search.min_similarity.max(0.0) as f32;
        self.inner.pool.with_read_conn(|conn| {
            collect_neighbors(
                conn,
                &node_id,
                direction,
                max_depth,
                min_similarity,
                DEFAULT_MAX_GRAPH_NODES,
                DEFAULT_MAX_EDGES_PER_NODE,
                DEFAULT_GRAPH_TRAVERSAL_DEADLINE_MS,
            )
        })
    }

    fn path(
        &self,
        from: &str,
        to: &str,
        max_depth: usize,
    ) -> Result<Option<Vec<String>>, MemoryError> {
        if from == to {
            return Ok(Some(vec![from.to_string()]));
        }
        if max_depth == 0 {
            return Ok(None);
        }

        let from = from.to_string();
        let to = to.to_string();
        let min_similarity = self.inner.config.search.min_similarity.max(0.0) as f32;
        self.inner.pool.with_read_conn(|conn| {
            shortest_path(
                conn,
                &from,
                &to,
                max_depth,
                min_similarity,
                DEFAULT_MAX_GRAPH_NODES,
                DEFAULT_MAX_EDGES_PER_NODE,
                DEFAULT_GRAPH_TRAVERSAL_DEADLINE_MS,
            )
        })
    }
}

fn collect_neighbors(
    conn: &Connection,
    start: &str,
    direction: GraphDirection,
    max_depth: usize,
    min_similarity: f32,
    max_nodes: usize,
    max_edges_per_node: usize,
    deadline_ms: u64,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut visited = HashSet::from([start.to_string()]);
    let mut queue = VecDeque::from([(start.to_string(), 0usize)]);
    let mut edges = Vec::new();
    let mut seen_edges = HashSet::new();
    let mut nodes_expanded = 0usize;
    let start_time = Instant::now();

    while let Some((node_id, depth)) = queue.pop_front() {
        if nodes_expanded >= max_nodes {
            tracing::warn!(
                start = start,
                max_nodes,
                deadline_ms,
                nodes_expanded,
                "graph traversal budget exceeded: max_nodes"
            );
            break;
        }
        nodes_expanded += 1;

        if start_time.elapsed().as_millis() > deadline_ms as u128 {
            tracing::warn!(
                start = start,
                max_nodes,
                deadline_ms,
                nodes_expanded,
                "graph traversal budget exceeded: deadline_ms"
            );
            break;
        }

        if depth >= max_depth {
            continue;
        }

        for edge in direct_edges(
            conn,
            &node_id,
            direction,
            min_similarity,
            max_edges_per_node,
        )? {
            let edge_key = edge_dedup_key(&edge)?;
            if seen_edges.insert(edge_key) {
                if let Some(next) = next_node_for_edge(&edge, &node_id) {
                    if visited.insert(next.clone()) {
                        queue.push_back((next, depth + 1));
                    }
                }
                edges.push(edge);
            }
        }
    }

    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.target.cmp(&b.target))
            .then_with(|| {
                a.weight
                    .partial_cmp(&b.weight)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });
    Ok(edges)
}

fn shortest_path(
    conn: &Connection,
    from: &str,
    to: &str,
    max_depth: usize,
    min_similarity: f32,
    max_nodes: usize,
    max_edges_per_node: usize,
    deadline_ms: u64,
) -> Result<Option<Vec<String>>, MemoryError> {
    // During path traversal we skip derived semantic edges (the O(n²)
    // all-embeddings scan).  Stored graph edges and entity edges are
    // sufficient for relationship/path queries and are O(1) per node.
    // Semantic edges are only useful for explicit neighbors() expansion.
    let compute_semantic = false;
    let mut visited = HashSet::from([from.to_string()]);
    let mut parents = BTreeMap::<String, String>::new();
    let mut queue = VecDeque::from([(from.to_string(), 0usize)]);
    let mut nodes_expanded = 0usize;
    let start_time = Instant::now();

    while let Some((node_id, depth)) = queue.pop_front() {
        if nodes_expanded >= max_nodes {
            tracing::warn!(
                from = from,
                to = to,
                max_nodes,
                deadline_ms,
                nodes_expanded,
                "graph traversal budget exceeded: max_nodes"
            );
            return Ok(None);
        }
        nodes_expanded += 1;

        if start_time.elapsed().as_millis() > deadline_ms as u128 {
            tracing::warn!(
                from = from,
                to = to,
                deadline_ms,
                nodes_expanded,
                "graph traversal budget exceeded: deadline_ms"
            );
            return Ok(None);
        }

        if depth >= max_depth {
            continue;
        }

        for edge in direct_edges(
            conn,
            &node_id,
            GraphDirection::Both,
            min_similarity,
            max_edges_per_node,
        )? {
            let Some(next) = next_node_for_edge(&edge, &node_id) else {
                continue;
            };
            if !visited.insert(next.clone()) {
                continue;
            }

            parents.insert(next.clone(), node_id.clone());
            if next == to {
                let mut path = vec![to.to_string()];
                let mut cursor = to.to_string();
                let mut guard = 0usize;
                while let Some(parent) = parents.get(&cursor) {
                    path.push(parent.clone());
                    if parent == from {
                        break;
                    }
                    cursor = parent.clone();
                    guard += 1;
                    if guard > max_depth {
                        return Err(MemoryError::CorruptData {
                            table: "graph_path",
                            row_id: to.to_string(),
                            detail: "parent reconstruction exceeded max_depth".to_string(),
                        });
                    }
                }
                path.reverse();
                return Ok(Some(path));
            }

            queue.push_back((next, depth + 1));
        }
    }

    Ok(None)
}

fn direct_edges(
    conn: &Connection,
    node_id: &str,
    direction: GraphDirection,
    min_similarity: f32,
    max_edges_per_node: usize,
) -> Result<Vec<GraphEdge>, MemoryError> {
    // Derived outgoing edges (semantic, temporal, causal, entity from content).
    let outgoing = outgoing_edges(conn, node_id, min_similarity, max_edges_per_node)?;

    // Stored graph edges (first-class, operator-authored).
    // These are directional: source->target is the canonical direction.
    let stored_out = crate::graph_edges::stored_outgoing_edges(conn, node_id)?;
    let stored_in = crate::graph_edges::stored_incoming_edges(conn, node_id)?;

    match direction {
        GraphDirection::Outgoing => {
            // Merge derived outgoing + stored outgoing (source=node).
            let mut edges = outgoing;
            edges.extend(stored_out);
            dedupe_edges(edges)
        }
        GraphDirection::Incoming => {
            // True incoming: stored edges where target=node, plus derived
            // cause backlinks. Do NOT reverse outgoing edges — that conflates
            // direction. Only symmetric/undirected edge types could justify
            // reversal, and we handle those under Both.
            let mut incoming = stored_in;
            incoming.extend(cause_backlinks(conn, node_id)?);
            dedupe_edges(incoming)
        }
        GraphDirection::Both => {
            // Both directions: outgoing + stored outgoing + stored incoming +
            // reversed outgoing (for symmetric traversal) + cause backlinks.
            let mut both = outgoing.clone();
            both.extend(outgoing.into_iter().map(reverse_edge));
            both.extend(stored_out);
            both.extend(stored_in);
            both.extend(cause_backlinks(conn, node_id)?);
            dedupe_edges(both)
        }
    }
}

fn dedupe_edges(edges: Vec<GraphEdge>) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut seen = HashSet::new();
    let mut deduped = Vec::new();
    for edge in edges {
        let key = edge_dedup_key(&edge)?;
        if seen.insert(key) {
            deduped.push(edge);
        }
    }
    Ok(deduped)
}

use boundary_compiler::Canonicalizer;

fn edge_dedup_key(edge: &GraphEdge) -> Result<String, MemoryError> {
    let edge_type = serde_json::to_string(&edge.edge_type)
        .map_err(|err| MemoryError::Other(format!("failed to serialize graph edge type: {err}")))?;
    let c = Canonicalizer::new();
    let metadata = match &edge.metadata {
        Some(value) => c.canonicalize(value).map_err(|err| {
            MemoryError::Other(format!("failed to canonicalize graph edge metadata: {err}"))
        })?,
        None => "null".to_string(),
    };
    Ok(format!(
        "{}|{}|{}|{:.6}|{}",
        edge.source, edge.target, edge_type, edge.weight, metadata
    ))
}

fn reverse_edge(edge: GraphEdge) -> GraphEdge {
    GraphEdge {
        source: edge.target,
        target: edge.source,
        edge_type: edge.edge_type,
        weight: edge.weight,
        metadata: edge.metadata,
    }
}

fn next_node_for_edge(edge: &GraphEdge, current: &str) -> Option<String> {
    if edge.source == current {
        Some(edge.target.clone())
    } else if edge.target == current {
        Some(edge.source.clone())
    } else {
        None
    }
}

fn outgoing_edges(
    conn: &Connection,
    node_id: &str,
    min_similarity: f32,
    max_edges_per_node: usize,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut edges = match parse_node_id(node_id) {
        ParsedNodeId::Namespace(namespace) => namespace_edges(conn, &namespace)?,
        ParsedNodeId::Fact(fact_id) => {
            fact_edges(conn, &fact_id, min_similarity, max_edges_per_node)?
        }
        ParsedNodeId::Document(document_id) => document_edges(conn, &document_id)?,
        ParsedNodeId::Chunk(chunk_id) => {
            chunk_edges(conn, &chunk_id, min_similarity, max_edges_per_node)?
        }
        ParsedNodeId::Session(session_id) => session_edges(conn, &session_id)?,
        ParsedNodeId::Message(message_id) => {
            message_edges(conn, message_id, min_similarity, max_edges_per_node)?
        }
        ParsedNodeId::Episode(document_id) => {
            episode_edges(conn, &document_id, min_similarity, max_edges_per_node)?
        }
        ParsedNodeId::Opaque => Vec::new(),
    };
    // Merge in first-class stored graph edges from the graph_edges table.
    edges.extend(crate::graph_edges::stored_edges_for_node(conn, node_id)?);
    edges.sort_by(|a, b| {
        a.source
            .cmp(&b.source)
            .then_with(|| a.target.cmp(&b.target))
    });
    Ok(edges)
}

fn namespace_edges(conn: &Connection, namespace: &str) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut edges = Vec::new();

    let mut facts_stmt =
        conn.prepare("SELECT id FROM facts WHERE namespace = ?1 ORDER BY id ASC LIMIT ?2")?;
    let fact_ids = facts_stmt
        .query_map(params![namespace, NAMESPACE_EDGE_BFS_CAP as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for fact_id in fact_ids {
        edges.push(entity_edge(
            format!("namespace:{namespace}"),
            format!("fact:{fact_id}"),
            "contains_fact",
            1.0,
            None,
        ));
    }

    let mut docs_stmt =
        conn.prepare("SELECT id FROM documents WHERE namespace = ?1 ORDER BY id ASC LIMIT ?2")?;
    let document_ids = docs_stmt
        .query_map(params![namespace, NAMESPACE_EDGE_BFS_CAP as i64], |row| {
            row.get::<_, String>(0)
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for document_id in document_ids {
        edges.push(entity_edge(
            format!("namespace:{namespace}"),
            format!("document:{document_id}"),
            "contains_document",
            1.0,
            None,
        ));
    }

    Ok(edges)
}

fn fact_edges(
    conn: &Connection,
    fact_id: &str,
    min_similarity: f32,
    max_edges_per_node: usize,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let row = conn.query_row(
        "SELECT namespace, embedding FROM facts WHERE id = ?1",
        params![fact_id],
        |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
    );

    match row {
        Ok((namespace, embedding_blob)) => {
            let mut edges = vec![entity_edge(
                format!("fact:{fact_id}"),
                format!("namespace:{namespace}"),
                "in_namespace",
                1.0,
                None,
            )];
            if let Some(blob) = embedding_blob {
                let embedding = db::bytes_to_embedding(&blob)?;
                edges.extend(semantic_edges(
                    conn,
                    &format!("fact:{fact_id}"),
                    &embedding,
                    min_similarity,
                    max_edges_per_node,
                )?);
            }
            Ok(edges)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

fn document_edges(conn: &Connection, document_id: &str) -> Result<Vec<GraphEdge>, MemoryError> {
    let namespace: String = match conn.query_row(
        "SELECT namespace FROM documents WHERE id = ?1",
        params![document_id],
        |row| row.get(0),
    ) {
        Ok(namespace) => namespace,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(Vec::new()),
        Err(err) => return Err(MemoryError::Database(err)),
    };

    let mut edges = vec![entity_edge(
        format!("document:{document_id}"),
        format!("namespace:{namespace}"),
        "in_namespace",
        1.0,
        None,
    )];

    let mut chunk_stmt = conn.prepare(
        "SELECT id, chunk_index FROM chunks WHERE document_id = ?1 ORDER BY chunk_index ASC",
    )?;
    let chunks = chunk_stmt
        .query_map(params![document_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    for (chunk_id, chunk_index) in chunks {
        edges.push(entity_edge(
            format!("document:{document_id}"),
            format!("chunk:{chunk_id}"),
            "contains_chunk",
            1.0,
            Some(serde_json::json!({ "chunk_index": chunk_index })),
        ));
    }

    // Link to all episodes for this document (supports multiple episodes per document).
    let ep_ids = episodes::list_document_episode_ids(conn, document_id)?;
    for ep_id in ep_ids {
        edges.push(entity_edge(
            format!("document:{document_id}"),
            episodes::episode_node_id(&ep_id),
            "has_episode",
            1.0,
            None,
        ));
    }

    Ok(edges)
}

fn chunk_edges(
    conn: &Connection,
    chunk_id: &str,
    min_similarity: f32,
    max_edges_per_node: usize,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let row = conn.query_row(
        "SELECT document_id, chunk_index, embedding
         FROM chunks
         WHERE id = ?1",
        params![chunk_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        },
    );

    match row {
        Ok((document_id, chunk_index, embedding_blob)) => {
            let mut edges = vec![entity_edge(
                format!("chunk:{chunk_id}"),
                format!("document:{document_id}"),
                "part_of_document",
                1.0,
                Some(serde_json::json!({ "chunk_index": chunk_index })),
            )];

            let mut stmt = conn.prepare(
                "SELECT id, chunk_index
                 FROM chunks
                 WHERE document_id = ?1 AND chunk_index IN (?2, ?3)
                 ORDER BY chunk_index ASC",
            )?;
            let neighbors = stmt
                .query_map(
                    params![document_id, chunk_index - 1, chunk_index + 1],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
                )?
                .collect::<Result<Vec<_>, _>>()?;
            for (neighbor_id, neighbor_index) in neighbors {
                edges.push(entity_edge(
                    format!("chunk:{chunk_id}"),
                    format!("chunk:{neighbor_id}"),
                    "adjacent_chunk",
                    1.0,
                    Some(serde_json::json!({ "chunk_index": neighbor_index })),
                ));
            }

            if let Some(blob) = embedding_blob {
                let embedding = db::bytes_to_embedding(&blob)?;
                edges.extend(semantic_edges(
                    conn,
                    &format!("chunk:{chunk_id}"),
                    &embedding,
                    min_similarity,
                    max_edges_per_node,
                )?);
            }

            Ok(edges)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

fn session_edges(conn: &Connection, session_id: &str) -> Result<Vec<GraphEdge>, MemoryError> {
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sessions WHERE id = ?1)",
            params![session_id],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !exists {
        return Ok(Vec::new());
    }

    let mut edges = Vec::new();
    let mut stmt = conn
        .prepare("SELECT id FROM messages WHERE session_id = ?1 ORDER BY created_at ASC, id ASC")?;
    let message_ids = stmt
        .query_map(params![session_id], |row| row.get::<_, i64>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    for (ordinal, message_id) in message_ids.into_iter().enumerate() {
        edges.push(entity_edge(
            format!("session:{session_id}"),
            format!("msg:{message_id}"),
            "contains_message",
            1.0,
            Some(serde_json::json!({ "ordinal": ordinal })),
        ));
    }
    Ok(edges)
}

fn message_edges(
    conn: &Connection,
    message_id: i64,
    min_similarity: f32,
    max_edges_per_node: usize,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let row = conn.query_row(
        "SELECT session_id, created_at, embedding
         FROM messages
         WHERE id = ?1",
        params![message_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<Vec<u8>>>(2)?,
            ))
        },
    );

    match row {
        Ok((session_id, created_at, embedding_blob)) => {
            let mut edges = vec![entity_edge(
                format!("msg:{message_id}"),
                format!("session:{session_id}"),
                "belongs_to_session",
                1.0,
                None,
            )];

            if let Some(prev) = adjacent_message(conn, &session_id, &created_at, message_id, true)?
            {
                if let Some(delta) = prev.delta_secs {
                    edges.push(temporal_edge(
                        format!("msg:{message_id}"),
                        format!("msg:{}", prev.id),
                        delta,
                    ));
                }
            }
            if let Some(next) = adjacent_message(conn, &session_id, &created_at, message_id, false)?
            {
                if let Some(delta) = next.delta_secs {
                    edges.push(temporal_edge(
                        format!("msg:{message_id}"),
                        format!("msg:{}", next.id),
                        delta,
                    ));
                }
            }

            if let Some(blob) = embedding_blob {
                let embedding = db::bytes_to_embedding(&blob)?;
                edges.extend(semantic_edges(
                    conn,
                    &format!("msg:{message_id}"),
                    &embedding,
                    min_similarity,
                    max_edges_per_node,
                )?);
            }

            Ok(edges)
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(Vec::new()),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

fn episode_edges(
    conn: &Connection,
    episode_id: &str,
    min_similarity: f32,
    max_edges_per_node: usize,
) -> Result<Vec<GraphEdge>, MemoryError> {
    // Canonical: episode nodes resolve only by episode_id.
    let row = conn.query_row(
        "SELECT episode_id, document_id, cause_ids, confidence, experiment_id, embedding
         FROM episodes WHERE episode_id = ?1",
        params![episode_id],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, f32>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<Vec<u8>>>(5)?,
            ))
        },
    );
    let row = match row {
        Ok(r) => r,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Ok(Vec::new()),
        Err(err) => return Err(MemoryError::Database(err)),
    };

    let (ep_id, document_id, cause_ids_raw, confidence, experiment_id, embedding_blob) = row;
    let cause_ids = db::parse_string_list_json("episodes", &ep_id, "cause_ids", &cause_ids_raw)?;

    let ep_node = episodes::episode_node_id(&ep_id);
    let mut edges = vec![entity_edge(
        ep_node.clone(),
        format!("document:{document_id}"),
        "attached_to_document",
        1.0,
        None,
    )];

    let evidence_ids = experiment_id
        .as_deref()
        .map(|id| vec![id.to_string()])
        .unwrap_or_default();
    for cause_id in cause_ids {
        let target = canonicalize_cause_id(conn, &cause_id)?;
        edges.push(GraphEdge {
            source: ep_node.clone(),
            target,
            edge_type: GraphEdgeType::Causal {
                confidence,
                evidence_ids: evidence_ids.clone(),
            },
            weight: confidence as f64,
            metadata: None,
        });
    }

    if let Some(blob) = embedding_blob {
        let embedding = db::bytes_to_embedding(&blob)?;
        edges.extend(semantic_edges(
            conn,
            &ep_node,
            &embedding,
            min_similarity,
            max_edges_per_node,
        )?);
    }

    Ok(edges)
}

struct AdjacentMessage {
    id: i64,
    delta_secs: Option<u64>,
}

fn adjacent_message(
    conn: &Connection,
    session_id: &str,
    created_at: &str,
    message_id: i64,
    previous: bool,
) -> Result<Option<AdjacentMessage>, MemoryError> {
    let comparator = if previous { "<" } else { ">" };
    let ordering = if previous { "DESC" } else { "ASC" };
    let sql = format!(
        "SELECT id, created_at
         FROM messages
         WHERE session_id = ?1
           AND (created_at {comparator} ?2 OR (created_at = ?2 AND id {} ?3))
         ORDER BY created_at {ordering}, id {ordering}
         LIMIT 1",
        if previous { "<" } else { ">" }
    );

    let row: Result<(i64, String), rusqlite::Error> =
        conn.query_row(&sql, params![session_id, created_at, message_id], |row| {
            Ok((row.get(0)?, row.get(1)?))
        });
    match row {
        Ok((adjacent_id, adjacent_created_at)) => {
            let delta_secs = timestamp_delta_secs(created_at, &adjacent_created_at);
            Ok(Some(AdjacentMessage {
                id: adjacent_id,
                delta_secs,
            }))
        }
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(err) => Err(MemoryError::Database(err)),
    }
}

fn timestamp_delta_secs(a: &str, b: &str) -> Option<u64> {
    let parse = |value: &str| {
        // Try the common format first, then with fractional seconds (SQLite/chrono).
        chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S")
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.6f"))
            .or_else(|_| chrono::NaiveDateTime::parse_from_str(value, "%Y-%m-%d %H:%M:%S%.3f"))
            .or_else(|_| {
                // Try RFC3339 / ISO8601 with timezone.
                chrono::DateTime::parse_from_rfc3339(value).map(|dt| dt.naive_utc())
            })
    };
    match (parse(a), parse(b)) {
        (Ok(a), Ok(b)) => Some((a - b).num_seconds().unsigned_abs()),
        // Parse failure: return None instead of 0 to avoid false temporal edges.
        _ => None,
    }
}

fn canonicalize_cause_id(conn: &Connection, raw: &str) -> Result<String, MemoryError> {
    if raw.contains(':') {
        return Ok(raw.to_string());
    }

    let fact_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM facts WHERE id = ?1)",
            params![raw],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if fact_exists {
        return Ok(format!("fact:{raw}"));
    }

    let chunk_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM chunks WHERE id = ?1)",
            params![raw],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if chunk_exists {
        return Ok(format!("chunk:{raw}"));
    }

    let document_exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM documents WHERE id = ?1)",
            params![raw],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if document_exists {
        return Ok(format!("document:{raw}"));
    }

    // INTENTIONAL: episode may not exist; None triggers fallback to other node types
    let episode_id: Option<String> = conn
        .query_row(
            "SELECT episode_id FROM episodes WHERE episode_id = ?1",
            params![raw],
            |row| row.get::<_, String>(0),
        )
        .ok();
    if let Some(ep_id) = episode_id {
        return Ok(episodes::episode_node_id(&ep_id));
    }

    if let Ok(message_id) = raw.parse::<i64>() {
        let message_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM messages WHERE id = ?1)",
                params![message_id],
                |row| row.get(0),
            )
            .unwrap_or(false);
        if message_exists {
            return Ok(format!("msg:{message_id}"));
        }
    }

    Ok(raw.to_string())
}

fn cause_backlinks(conn: &Connection, node_id: &str) -> Result<Vec<GraphEdge>, MemoryError> {
    // Strip prefix to get the raw ID for matching against cause_node_id.
    let raw_id = node_id.split_once(':').map(|(_, v)| v).unwrap_or(node_id);

    // Use the normalized episode_causes table instead of a full-table JSON scan.
    let mut stmt = conn.prepare(
        "SELECT ec.episode_id, e.confidence, e.experiment_id
         FROM episode_causes ec
         JOIN episodes e ON e.episode_id = ec.episode_id
         WHERE ec.cause_node_id = ?1 OR ec.cause_node_id = ?2
         ORDER BY ec.episode_id ASC",
    )?;
    let rows = stmt
        .query_map(params![node_id, raw_id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, f32>(1)?,
                row.get::<_, Option<String>>(2)?,
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;

    let mut edges = Vec::new();
    for (episode_id, confidence, experiment_id) in rows {
        let evidence_ids = experiment_id.into_iter().collect::<Vec<_>>();
        edges.push(GraphEdge {
            source: episodes::episode_node_id(&episode_id),
            target: node_id.to_string(),
            edge_type: GraphEdgeType::Causal {
                confidence,
                evidence_ids,
            },
            weight: confidence as f64,
            metadata: None,
        });
    }
    Ok(edges)
}

fn semantic_edges(
    conn: &Connection,
    source_node_id: &str,
    embedding: &[f32],
    min_similarity: f32,
    max_edges_per_node: usize,
) -> Result<Vec<GraphEdge>, MemoryError> {
    let mut candidates = Vec::new();
    let mut remaining_capacity = max_edges_per_node;

    if remaining_capacity > 0 {
        let mut fact_candidates = semantic_candidates(
            conn,
            "facts",
            "id",
            "embedding",
            |row_id| format!("fact:{row_id}"),
            remaining_capacity,
        )?;
        let take = remaining_capacity.min(fact_candidates.len());
        candidates.extend(fact_candidates.drain(0..take));
        remaining_capacity -= take;
    }
    if remaining_capacity > 0 {
        let mut chunk_candidates = semantic_candidates(
            conn,
            "chunks",
            "id",
            "embedding",
            |row_id| format!("chunk:{row_id}"),
            remaining_capacity,
        )?;
        let take = remaining_capacity.min(chunk_candidates.len());
        candidates.extend(chunk_candidates.drain(0..take));
        remaining_capacity -= take;
    }
    if remaining_capacity > 0 {
        let mut message_candidates = semantic_candidates(
            conn,
            "messages",
            "id",
            "embedding",
            |row_id| format!("msg:{row_id}"),
            remaining_capacity,
        )?;
        let take = remaining_capacity.min(message_candidates.len());
        candidates.extend(message_candidates.drain(0..take));
        remaining_capacity -= take;
    }
    if remaining_capacity > 0 {
        let mut episode_candidates = semantic_candidates(
            conn,
            "episodes",
            "episode_id",
            "embedding",
            |ep_id| episodes::episode_node_id(&ep_id),
            remaining_capacity,
        )?;
        let take = remaining_capacity.min(episode_candidates.len());
        candidates.extend(episode_candidates.drain(0..take));
    }

    let mut scored = candidates
        .into_iter()
        .filter(|(node_id, _)| node_id != source_node_id)
        .filter_map(|(node_id, candidate_embedding)| {
            if candidate_embedding.len() != embedding.len() {
                return None;
            }
            let similarity = match search::cosine_similarity(embedding, &candidate_embedding) {
                Ok(similarity) => similarity,
                Err(error) => {
                    tracing::warn!(error = %error, node_id, "skipping invalid semantic graph vector");
                    return None;
                }
            };
            (similarity >= min_similarity).then_some((node_id, similarity))
        })
        .collect::<Vec<_>>();

    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.0.cmp(&b.0))
    });
    scored.truncate(SEMANTIC_EDGE_LIMIT);

    Ok(scored
        .into_iter()
        .map(|(target, similarity)| GraphEdge {
            source: source_node_id.to_string(),
            target,
            edge_type: GraphEdgeType::Semantic {
                cosine_similarity: similarity,
            },
            weight: similarity as f64,
            metadata: None,
        })
        .collect())
}

fn semantic_candidates<F>(
    conn: &Connection,
    table: &str,
    id_column: &str,
    embedding_column: &str,
    key_fn: F,
    max_edges_per_node: usize,
) -> Result<Vec<(String, Vec<f32>)>, MemoryError>
where
    F: Fn(String) -> String,
{
    // SQL-level LIMIT bounds the query plan, not just the Rust iterator.
    // Without LIMIT, SQLite prepares a full-table-scan plan even though we
    // break early in the iterator. With LIMIT, SQLite can short-circuit.
    let sql = format!(
        "SELECT {id_column}, {embedding_column}
         FROM {table}
         WHERE {embedding_column} IS NOT NULL
         LIMIT {limit}",
        limit = max_edges_per_node
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;

    let mut decoded = Vec::new();
    for row in rows {
        let (row_id, blob) = row?;
        if let Ok(embedding) = db::bytes_to_embedding(&blob) {
            decoded.push((key_fn(row_id), embedding));
        }
    }
    Ok(decoded)
}

fn entity_edge(
    source: String,
    target: String,
    relation: &str,
    weight: f64,
    metadata: Option<serde_json::Value>,
) -> GraphEdge {
    GraphEdge {
        source,
        target,
        edge_type: GraphEdgeType::Entity {
            relation: relation.to_string(),
        },
        weight,
        metadata,
    }
}

fn temporal_edge(source: String, target: String, delta_secs: u64) -> GraphEdge {
    GraphEdge {
        source,
        target,
        edge_type: GraphEdgeType::Temporal { delta_secs },
        weight: if delta_secs == 0 {
            1.0
        } else {
            1.0 / delta_secs as f64
        },
        metadata: None,
    }
}

enum ParsedNodeId {
    Namespace(String),
    Fact(String),
    Document(String),
    Chunk(String),
    Session(String),
    Message(i64),
    Episode(String),
    Opaque,
}

fn parse_node_id(node_id: &str) -> ParsedNodeId {
    match node_id.split_once(':') {
        Some(("namespace", value)) => ParsedNodeId::Namespace(value.to_string()),
        Some(("fact", value)) => ParsedNodeId::Fact(value.to_string()),
        Some(("document", value)) => ParsedNodeId::Document(value.to_string()),
        Some(("chunk", value)) => ParsedNodeId::Chunk(value.to_string()),
        Some(("session", value)) => ParsedNodeId::Session(value.to_string()),
        Some(("msg", value)) => value
            .parse::<i64>()
            .map(ParsedNodeId::Message)
            .unwrap_or(ParsedNodeId::Opaque),
        Some(("episode", value)) => ParsedNodeId::Episode(value.to_string()),
        _ => ParsedNodeId::Opaque,
    }
}
