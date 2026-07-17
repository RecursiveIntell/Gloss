//! Database initialization, migrations, integrity checks, and durable sidecar state.

use crate::config::{EmbeddingConfig, MemoryLimits, PoolConfig};
use crate::embedder::SparseWeights;
use crate::error::MemoryError;
use crate::quantize::unpack_quantized;
#[cfg(feature = "turbo-quant-codec")]
use crate::types::{DerivedVectorArtifactGenerationV1, VectorArtifactBuildReceiptV1};
use crate::types::{
    EpisodeOutcome, ProveKvPoolArtifactStatusV1, ProveKvPoolGenerationStatus,
    ProveKvPoolGenerationV1, ProveKvPoolItemMapEntryV1, Role, SearchSourceType,
    SparseRankReceiptV1, VectorSearchReceiptV1, VerificationStatus,
};
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{Deserialize, Serialize};
use stack_ids::ContentDigest;
#[cfg(feature = "turbo-quant-codec")]
use stack_ids::DigestBuilder;
use std::path::Path;

/// V1 migration: full schema.
const MIGRATION_V1: &str = r#"
-- CONVERSATIONS
CREATE TABLE sessions (
    id          TEXT PRIMARY KEY,
    channel     TEXT NOT NULL DEFAULT 'repl',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE INDEX idx_sessions_updated ON sessions(updated_at DESC);

CREATE TABLE messages (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    session_id  TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
    role        TEXT NOT NULL CHECK (role IN ('system', 'user', 'assistant', 'tool')),
    content     TEXT NOT NULL,
    token_count INTEGER,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE INDEX idx_messages_session ON messages(session_id, created_at ASC);
CREATE INDEX idx_messages_created ON messages(created_at DESC);

-- KNOWLEDGE (Facts)
CREATE TABLE facts (
    id          TEXT PRIMARY KEY,
    namespace   TEXT NOT NULL DEFAULT 'general',
    content     TEXT NOT NULL,
    source      TEXT,
    embedding   BLOB,
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE INDEX idx_facts_namespace ON facts(namespace);
CREATE INDEX idx_facts_updated ON facts(updated_at DESC);

CREATE TABLE facts_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    fact_id     TEXT NOT NULL UNIQUE REFERENCES facts(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE facts_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

-- DOCUMENTS (Chunked content)
CREATE TABLE documents (
    id          TEXT PRIMARY KEY,
    title       TEXT NOT NULL,
    source_path TEXT,
    namespace   TEXT NOT NULL DEFAULT 'general',
    created_at  TEXT NOT NULL DEFAULT (datetime('now')),
    metadata    TEXT
);

CREATE TABLE chunks (
    id          TEXT PRIMARY KEY,
    document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    chunk_index INTEGER NOT NULL,
    content     TEXT NOT NULL,
    token_count INTEGER,
    embedding   BLOB,
    created_at  TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX idx_chunks_document ON chunks(document_id, chunk_index ASC);

CREATE TABLE chunks_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    chunk_id    TEXT NOT NULL UNIQUE REFERENCES chunks(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE chunks_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

-- EMBEDDING METADATA
CREATE TABLE embedding_metadata (
    id          INTEGER PRIMARY KEY CHECK (id = 1),
    model_name  TEXT NOT NULL,
    dimensions  INTEGER NOT NULL,
    updated_at  TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

/// V2 migration: message embeddings for conversation search.
const MIGRATION_V2: &str = r#"
ALTER TABLE messages ADD COLUMN embedding BLOB;

CREATE TABLE messages_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    message_id  INTEGER NOT NULL UNIQUE REFERENCES messages(id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE messages_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);
"#;

/// V3 migration: embedding staleness tracking.
const MIGRATION_V3: &str = r#"
ALTER TABLE embedding_metadata ADD COLUMN embeddings_dirty INTEGER NOT NULL DEFAULT 0;
"#;

/// V4 migration: HNSW metadata tracking.
const MIGRATION_V4: &str = r#"
CREATE TABLE IF NOT EXISTS hnsw_metadata (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
"#;

/// V5 migration: quantized embeddings + HNSW keymap persistence.
const MIGRATION_V5: &str = r#"
ALTER TABLE facts ADD COLUMN embedding_q8 BLOB;
ALTER TABLE chunks ADD COLUMN embedding_q8 BLOB;
ALTER TABLE messages ADD COLUMN embedding_q8 BLOB;

CREATE TABLE IF NOT EXISTS hnsw_keymap (
    node_id     INTEGER PRIMARY KEY,
    item_key    TEXT NOT NULL UNIQUE,
    deleted     INTEGER NOT NULL DEFAULT 0
);

CREATE INDEX idx_hnsw_keymap_key ON hnsw_keymap(item_key);
"#;

/// V6 migration: episodes table for causal tracking.
const MIGRATION_V6: &str = r#"
CREATE TABLE IF NOT EXISTS episodes (
    document_id TEXT PRIMARY KEY REFERENCES documents(id) ON DELETE CASCADE,
    cause_ids TEXT NOT NULL,
    effect_type TEXT NOT NULL,
    outcome TEXT NOT NULL DEFAULT 'pending',
    confidence REAL NOT NULL DEFAULT 0.0,
    verification_status TEXT NOT NULL DEFAULT '{"status":"unverified"}',
    experiment_id TEXT,
    created_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_episodes_effect_type ON episodes(effect_type);
CREATE INDEX IF NOT EXISTS idx_episodes_outcome ON episodes(outcome);
CREATE INDEX IF NOT EXISTS idx_episodes_experiment_id ON episodes(experiment_id);
"#;

/// V7 migration: searchable episodes + durable sidecar journal.
const MIGRATION_V7: &str = r#"
ALTER TABLE episodes ADD COLUMN updated_at TEXT NOT NULL DEFAULT (datetime('now'));
ALTER TABLE episodes ADD COLUMN search_text TEXT NOT NULL DEFAULT '';
ALTER TABLE episodes ADD COLUMN embedding BLOB;
ALTER TABLE episodes ADD COLUMN embedding_q8 BLOB;

CREATE TABLE IF NOT EXISTS episodes_rowid_map (
    rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
    document_id TEXT NOT NULL UNIQUE REFERENCES episodes(document_id) ON DELETE CASCADE
);

CREATE VIRTUAL TABLE episodes_fts USING fts5(
    content,
    content='',
    content_rowid='rowid',
    tokenize='porter unicode61'
);

CREATE TABLE IF NOT EXISTS pending_index_ops (
    item_key      TEXT PRIMARY KEY,
    entity_type   TEXT NOT NULL,
    op_kind       TEXT NOT NULL CHECK (op_kind IN ('upsert', 'delete')),
    attempt_count INTEGER NOT NULL DEFAULT 0,
    last_error    TEXT,
    updated_at    TEXT NOT NULL DEFAULT (datetime('now'))
);

INSERT OR IGNORE INTO hnsw_metadata (key, value) VALUES ('sidecar_dirty', '0');

UPDATE episodes
SET search_text = TRIM(
    COALESCE(effect_type, '') || ' ' ||
    COALESCE(outcome, '') || ' ' ||
    COALESCE(experiment_id, '') || ' ' ||
    COALESCE(cause_ids, '')
)
WHERE search_text = '';

INSERT OR IGNORE INTO episodes_rowid_map (document_id)
SELECT document_id FROM episodes;

INSERT INTO episodes_fts (rowid, content)
SELECT rm.rowid, e.search_text
FROM episodes_rowid_map rm
JOIN episodes e ON e.document_id = rm.document_id;
"#;

/// V8 migration: durable episode trace IDs.
const MIGRATION_V8: &str = r#"
ALTER TABLE episodes ADD COLUMN trace_id TEXT;
"#;

/// V9 migration: first-class episode identity + normalized causal edge table.
///
/// Rebuilds the episodes table so `episode_id` is the primary key while
/// `document_id` becomes a non-unique FK allowing multiple episodes per doc.
/// Adds `episode_causes` for normalized causal backlinks.
///
/// Applied via `run_migration_v9()` because it requires table rebuild.
const MIGRATION_V9: &str = "";

/// V18 migration: durable, replay-addressable search receipts.
const MIGRATION_V18: &str = r#"
CREATE TABLE IF NOT EXISTS search_receipts (
    receipt_id             TEXT PRIMARY KEY,
    schema_version         TEXT NOT NULL,
    evaluation_time        TEXT NOT NULL,
    search_profile         TEXT NOT NULL,
    candidate_backend      TEXT NOT NULL,
    approximate            INTEGER NOT NULL CHECK (approximate IN (0, 1)),
    exact_rerank           INTEGER NOT NULL CHECK (exact_rerank IN (0, 1)),
    fallback               TEXT,
    requested_candidates   INTEGER NOT NULL CHECK (requested_candidates >= 0),
    returned_candidates    INTEGER NOT NULL CHECK (returned_candidates >= 0),
    post_filter_candidates INTEGER NOT NULL CHECK (post_filter_candidates >= 0),
    result_ids_json        TEXT NOT NULL,
    receipt_json           TEXT NOT NULL,
    receipt_digest         TEXT NOT NULL,
    created_at             TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE INDEX IF NOT EXISTS idx_search_receipts_created
ON search_receipts(created_at DESC);

CREATE INDEX IF NOT EXISTS idx_search_receipts_backend
ON search_receipts(candidate_backend);
"#;

/// V19 migration: rebuildable derived vector acceleration artifacts.
const MIGRATION_V19: &str = r#"
CREATE TABLE IF NOT EXISTS derived_vector_artifacts (
    item_key                TEXT NOT NULL,
    codec_family            TEXT NOT NULL,
    codec_profile_digest    TEXT NOT NULL,
    source_embedding_digest TEXT NOT NULL,
    encoded_digest          TEXT NOT NULL,
    artifact_digest         TEXT NOT NULL,
    encoding                TEXT NOT NULL,
    dim                     INTEGER NOT NULL,
    encoded                 BLOB NOT NULL,
    created_at              TEXT NOT NULL DEFAULT (datetime('now')),
    status                  TEXT NOT NULL DEFAULT 'active',
    PRIMARY KEY (item_key, codec_family, codec_profile_digest)
);

CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_profile
ON derived_vector_artifacts(codec_family, codec_profile_digest, status);

CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_source_digest
ON derived_vector_artifacts(source_embedding_digest);
"#;

/// V20 migration: align derived vector artifact rows with P31 evidence fields.
const MIGRATION_V20: &str = r#"
-- Procedural migration; see run_migration_v20.
"#;

/// V21 migration: generation-level manifests for derived vector artifacts.
const MIGRATION_V21: &str = r#"
CREATE TABLE IF NOT EXISTS derived_vector_artifact_generations (
    generation_id            TEXT PRIMARY KEY,
    schema_version           TEXT NOT NULL,
    codec_family             TEXT NOT NULL,
    codec_profile_digest     TEXT NOT NULL,
    source_snapshot_digest   TEXT NOT NULL,
    source_row_count         INTEGER NOT NULL,
    artifact_count           INTEGER NOT NULL,
    source_tables_json       TEXT NOT NULL,
    dim                      INTEGER NOT NULL,
    encoding                 TEXT NOT NULL,
    created_at               TEXT NOT NULL,
    build_receipt_id         TEXT,
    artifact_manifest_digest TEXT NOT NULL,
    status                   TEXT NOT NULL CHECK (status IN ('active', 'superseded', 'invalidated', 'failed')),
    degradations_json        TEXT NOT NULL DEFAULT '[]'
);

CREATE INDEX IF NOT EXISTS idx_derived_vector_generations_profile
ON derived_vector_artifact_generations(codec_family, codec_profile_digest, status, created_at DESC);
"#;

/// V23 migration: codec governance columns on derived_vector_artifacts.
/// Tracks governed compression pipeline metadata for turbo-quant-codec integration.
const MIGRATION_V23: &str = r#"
ALTER TABLE derived_vector_artifacts ADD COLUMN codec_governance_receipt_id TEXT;
ALTER TABLE derived_vector_artifacts ADD COLUMN codec_profile TEXT;
ALTER TABLE derived_vector_artifacts ADD COLUMN degradation_budget REAL;
ALTER TABLE derived_vector_artifacts ADD COLUMN raw_source_artifact_id TEXT;
"#;

/// V22 migration: bitemporal columns on episodes table.
/// Adds valid_time, recorded_time, superseded_by, and fact_digest for append-supersede semantics.
const MIGRATION_V22: &str = r#"
ALTER TABLE episodes ADD COLUMN valid_time TEXT;
ALTER TABLE episodes ADD COLUMN recorded_time TEXT NOT NULL DEFAULT (datetime('now'));
ALTER TABLE episodes ADD COLUMN superseded_by TEXT;
ALTER TABLE episodes ADD COLUMN fact_digest TEXT;
CREATE INDEX IF NOT EXISTS idx_episodes_recorded ON episodes(recorded_time ASC);
CREATE INDEX IF NOT EXISTS idx_episodes_valid ON episodes(valid_time);
CREATE INDEX IF NOT EXISTS idx_episodes_superseded ON episodes(superseded_by) WHERE superseded_by IS NOT NULL;
UPDATE episodes SET recorded_time = updated_at WHERE recorded_time IS NULL OR recorded_time = '';
"#;

/// V24 migration: proveKV/poly-kv generation-level derived candidate pool metadata.
const MIGRATION_V24: &str = r#"
CREATE TABLE IF NOT EXISTS provekv_pool_generations (
  generation_id TEXT PRIMARY KEY,
  embedding_snapshot_digest TEXT NOT NULL,
  source_digest TEXT NOT NULL,
  pool_manifest_digest TEXT NOT NULL,
  codec_family TEXT NOT NULL,
  codec_profile TEXT NOT NULL,
  vector_dim INTEGER NOT NULL,
  item_count INTEGER NOT NULL,
  payload_bytes INTEGER NOT NULL,
  payload BLOB NOT NULL,
  status TEXT NOT NULL,
  failure_reason TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS provekv_pool_item_map (
  generation_id TEXT NOT NULL,
  item_id TEXT NOT NULL,
  source_type TEXT NOT NULL,
  pool_index INTEGER NOT NULL,
  embedding_digest TEXT NOT NULL,
  PRIMARY KEY (generation_id, item_id),
  FOREIGN KEY (generation_id) REFERENCES provekv_pool_generations(generation_id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_provekv_pool_item_map_generation_index
  ON provekv_pool_item_map(generation_id, pool_index);

CREATE INDEX IF NOT EXISTS idx_provekv_pool_generations_status_created
  ON provekv_pool_generations(status, created_at DESC);
"#;

/// V25 migration: semiring provenance table (Phase 2).
///
/// Idempotent. The `provenance` table is append-only truth-bearing state keyed
/// by (item_type, item_id). The Rust API is feature-gated behind `provenance`,
/// but the table is always created so the schema version sequence stays
/// monotonic.
const MIGRATION_V25: &str = r#"
CREATE TABLE IF NOT EXISTS provenance (
    id                 TEXT PRIMARY KEY,
    item_type          TEXT NOT NULL,
    item_id            TEXT NOT NULL,
    semiring_type      TEXT NOT NULL,
    semiring_value     TEXT NOT NULL,
    support_chain_json TEXT NOT NULL DEFAULT '[]',
    recorded_at        TEXT NOT NULL DEFAULT (datetime('now')),
    episode_id         TEXT
);

CREATE INDEX IF NOT EXISTS idx_provenance_item
    ON provenance(item_type, item_id);

CREATE INDEX IF NOT EXISTS idx_provenance_episode
    ON provenance(episode_id);
"#;

/// V26 migration: temporal weight columns (Phase 3).
///
/// Procedural migration — SQLite ALTER TABLE ADD COLUMN does not support
/// IF NOT EXISTS, so we use `add_column_if_missing` for idempotency.
/// `temporal_weight` is a COMPUTED SCORE (not truth) — the only column
/// callers may UPDATE directly. The Rust API is feature-gated behind `temporal`.
const MIGRATION_V26: &str = "";

/// Run V26 migration procedurally: add temporal_weight columns if absent.
fn run_migration_v26(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(
        conn,
        "facts",
        "temporal_weight",
        "REAL NOT NULL DEFAULT 1.0",
    )?;
    add_column_if_missing(
        conn,
        "chunks",
        "temporal_weight",
        "REAL NOT NULL DEFAULT 1.0",
    )?;
    add_column_if_missing(
        conn,
        "messages",
        "temporal_weight",
        "REAL NOT NULL DEFAULT 1.0",
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_facts_temporal ON facts(temporal_weight);
         CREATE INDEX IF NOT EXISTS idx_chunks_temporal ON chunks(temporal_weight);",
    )?;
    Ok(())
}

/// V27 migration: first-class stored graph edges table.
///
/// Idempotent. Stores durable, typed relationships between any two nodes.
/// Append-only with invalidation (is_invalidated flag). Content digest
/// (blake3) ensures idempotent insertion.
const MIGRATION_V27: &str = r#"
CREATE TABLE IF NOT EXISTS graph_edges (
    id                  TEXT PRIMARY KEY,
    source              TEXT NOT NULL,
    target              TEXT NOT NULL,
    edge_type           TEXT NOT NULL,
    weight              REAL NOT NULL,
    metadata            TEXT,
    content_digest      TEXT NOT NULL,
    recorded_at         TEXT NOT NULL,
    is_invalidated      INTEGER NOT NULL DEFAULT 0,
    invalidated_at      TEXT,
    invalidation_reason TEXT
);

CREATE INDEX IF NOT EXISTS idx_graph_edges_source
    ON graph_edges(source) WHERE is_invalidated = 0;

CREATE INDEX IF NOT EXISTS idx_graph_edges_target
    ON graph_edges(target) WHERE is_invalidated = 0;

CREATE INDEX IF NOT EXISTS idx_graph_edges_digest
    ON graph_edges(content_digest) WHERE is_invalidated = 0;
"#;

/// V28 migration: bitemporal semantics for first-class graph edges.
///
/// `valid_time` is domain/business time. `recorded_time` is system knowledge
/// time. Existing V27 edges are backfilled from `recorded_at`, preserving the
/// original append-only insertion timestamp as their initial bitemporal point.
const MIGRATION_V28: &str = "";

fn run_migration_v28(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(conn, "graph_edges", "valid_time", "TEXT")?;
    add_column_if_missing(conn, "graph_edges", "recorded_time", "TEXT")?;
    // Backfill missing bitemporal timestamps with recorded_at.
    conn.execute(
        "UPDATE graph_edges
         SET valid_time = COALESCE(valid_time, recorded_at),
             recorded_time = COALESCE(recorded_time, recorded_at)",
        [],
    )?;
    // Canonicalize any RFC3339 values to fixed-width SQL microseconds so
    // lexicographic ordering equals chronological ordering. Mixed formats
    // can otherwise make as-of queries wrong.
    conn.execute(
        "UPDATE graph_edges
         SET valid_time = format('%Y-%m-%d %H:%M:%f', valid_time),
             recorded_time = format('%Y-%m-%d %H:%M:%f', recorded_time),
             invalidated_at = CASE
                 WHEN invalidated_at IS NULL THEN NULL
                 ELSE format('%Y-%m-%d %H:%M:%f', invalidated_at)
             END",
        [],
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_graph_edges_bitemporal
             ON graph_edges(valid_time, recorded_time);
         CREATE INDEX IF NOT EXISTS idx_graph_edges_recorded_time
             ON graph_edges(recorded_time);",
    )?;
    Ok(())
}

/// V29 migration: transactional authority state, lineage heads, operation journal, and receipts.
const MIGRATION_V29: &str = r#"
CREATE TABLE IF NOT EXISTS authority_state (
    id              INTEGER PRIMARY KEY CHECK (id = 1),
    retrieval_epoch INTEGER NOT NULL CHECK (retrieval_epoch >= 0)
);
INSERT OR IGNORE INTO authority_state (id, retrieval_epoch) VALUES (1, 0);

CREATE TABLE IF NOT EXISTS authority_lineages (
    lineage_id      TEXT PRIMARY KEY,
    active_head_id  TEXT NOT NULL REFERENCES facts(id),
    updated_epoch   INTEGER NOT NULL CHECK (updated_epoch >= 0)
);

CREATE TABLE IF NOT EXISTS authority_versions (
    fact_id         TEXT PRIMARY KEY REFERENCES facts(id) ON DELETE CASCADE,
    lineage_id      TEXT NOT NULL REFERENCES authority_lineages(lineage_id),
    version         INTEGER NOT NULL CHECK (version > 0),
    operation_kind  TEXT NOT NULL CHECK (operation_kind IN ('append', 'supersede', 'redact')),
    is_active       INTEGER NOT NULL DEFAULT 1 CHECK (is_active IN (0, 1)),
    is_redacted     INTEGER NOT NULL DEFAULT 0 CHECK (is_redacted IN (0, 1)),
    content_digest  TEXT NOT NULL,
    UNIQUE (lineage_id, version)
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_authority_one_active_head
    ON authority_versions(lineage_id) WHERE is_active = 1;
CREATE INDEX IF NOT EXISTS idx_authority_versions_lineage
    ON authority_versions(lineage_id, version);

CREATE TABLE IF NOT EXISTS operation_journal (
    operation_id            TEXT PRIMARY KEY,
    caller_idempotency_key  TEXT NOT NULL UNIQUE,
    operation_kind          TEXT NOT NULL,
    payload_digest          TEXT NOT NULL,
    principal               TEXT NOT NULL,
    caller_id               TEXT NOT NULL,
    before_epoch            INTEGER NOT NULL,
    after_epoch             INTEGER NOT NULL,
    affected_ids_json       TEXT NOT NULL,
    content_digest          TEXT NOT NULL,
    committed_at            TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_operation_journal_idempotency
    ON operation_journal(caller_idempotency_key);

CREATE TABLE IF NOT EXISTS authority_receipts (
    receipt_id              TEXT PRIMARY KEY,
    operation_id            TEXT NOT NULL UNIQUE REFERENCES operation_journal(operation_id),
    caller_idempotency_key  TEXT NOT NULL UNIQUE,
    receipt_json             TEXT NOT NULL,
    receipt_digest           TEXT NOT NULL,
    created_at               TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_authority_receipts_operation
    ON authority_receipts(operation_id);
CREATE INDEX IF NOT EXISTS idx_authority_receipts_idempotency
    ON authority_receipts(caller_idempotency_key);
"#;

/// V30 migration: immutable verification/quarantine records adjacent to authority state.
const MIGRATION_V30: &str = r#"
CREATE TABLE IF NOT EXISTS memory_transition_records (
    record_id               TEXT PRIMARY KEY,
    caller_idempotency_key  TEXT NOT NULL UNIQUE,
    principal               TEXT NOT NULL,
    caller_id               TEXT NOT NULL,
    candidate_digest        TEXT NOT NULL,
    candidate_json          TEXT NOT NULL,
    verification_json       TEXT NOT NULL,
    disposition             TEXT NOT NULL CHECK (disposition IN ('commit', 'quarantine')),
    authority_receipt_id    TEXT REFERENCES authority_receipts(receipt_id),
    created_at              TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_memory_transition_disposition
    ON memory_transition_records(disposition, created_at);
CREATE TRIGGER IF NOT EXISTS memory_transition_records_no_update
BEFORE UPDATE ON memory_transition_records
BEGIN
    SELECT RAISE(ABORT, 'memory transition records are immutable');
END;
CREATE TRIGGER IF NOT EXISTS memory_transition_records_no_delete
BEFORE DELETE ON memory_transition_records
BEGIN
    SELECT RAISE(ABORT, 'memory transition records are immutable');
END;
"#;

/// V31 migration: immutable write-time origin labels and append-only revocations.
const MIGRATION_V31: &str = r#"
CREATE TABLE IF NOT EXISTS origin_authority_labels (
    fact_id          TEXT PRIMARY KEY REFERENCES facts(id) ON DELETE CASCADE,
    label_json       TEXT NOT NULL,
    label_digest     TEXT NOT NULL,
    recorded_at      TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS origin_authority_labels_no_update
BEFORE UPDATE ON origin_authority_labels
BEGIN
    SELECT RAISE(ABORT, 'origin authority labels are immutable');
END;
CREATE TRIGGER IF NOT EXISTS origin_authority_labels_no_delete
BEFORE DELETE ON origin_authority_labels
BEGIN
    SELECT RAISE(ABORT, 'origin authority labels are immutable');
END;

CREATE TABLE IF NOT EXISTS origin_authority_revocations (
    revocation_id           TEXT PRIMARY KEY,
    fact_id                 TEXT NOT NULL REFERENCES facts(id),
    caller_idempotency_key  TEXT NOT NULL UNIQUE,
    principal               TEXT NOT NULL,
    revocation_reference    TEXT NOT NULL,
    revoked_at              TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_origin_authority_revocations_fact
    ON origin_authority_revocations(fact_id, revoked_at);
CREATE TRIGGER IF NOT EXISTS origin_authority_revocations_no_update
BEFORE UPDATE ON origin_authority_revocations
BEGIN
    SELECT RAISE(ABORT, 'origin authority revocations are append-only');
END;
CREATE TRIGGER IF NOT EXISTS origin_authority_revocations_no_delete
BEFORE DELETE ON origin_authority_revocations
BEGIN
    SELECT RAISE(ABORT, 'origin authority revocations are append-only');
END;
"#;

/// V32 migration: append-only selective-forgetting tombstones and closure receipts.
const MIGRATION_V32: &str = r#"
ALTER TABLE authority_state ADD COLUMN projection_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE authority_state ADD COLUMN cache_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE authority_state ADD COLUMN export_epoch INTEGER NOT NULL DEFAULT 0;
ALTER TABLE authority_state ADD COLUMN replay_epoch INTEGER NOT NULL DEFAULT 0;

CREATE TABLE IF NOT EXISTS forgotten_facts (
    fact_id          TEXT PRIMARY KEY REFERENCES facts(id),
    receipt_id       TEXT NOT NULL,
    namespace        TEXT NOT NULL,
    content_digest   TEXT NOT NULL,
    forgotten_at     TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_forgotten_facts_namespace
    ON forgotten_facts(namespace, forgotten_at);
CREATE TRIGGER IF NOT EXISTS forgotten_facts_no_update
BEFORE UPDATE ON forgotten_facts BEGIN
    SELECT RAISE(ABORT, 'forgotten fact tombstones are append-only');
END;
CREATE TRIGGER IF NOT EXISTS forgotten_facts_no_delete
BEFORE DELETE ON forgotten_facts BEGIN
    SELECT RAISE(ABORT, 'forgotten fact tombstones are append-only');
END;

CREATE TABLE IF NOT EXISTS forgetting_artifact_invalidations (
    surface_kind    TEXT NOT NULL,
    artifact_id     TEXT NOT NULL,
    receipt_id      TEXT NOT NULL,
    invalidated_at  TEXT NOT NULL,
    PRIMARY KEY(surface_kind, artifact_id)
);
CREATE INDEX IF NOT EXISTS idx_forgetting_invalidations_receipt
    ON forgetting_artifact_invalidations(receipt_id);
CREATE TRIGGER IF NOT EXISTS forgetting_invalidations_no_update
BEFORE UPDATE ON forgetting_artifact_invalidations BEGIN
    SELECT RAISE(ABORT, 'forgetting invalidations are append-only');
END;
CREATE TRIGGER IF NOT EXISTS forgetting_invalidations_no_delete
BEFORE DELETE ON forgetting_artifact_invalidations BEGIN
    SELECT RAISE(ABORT, 'forgetting invalidations are append-only');
END;

CREATE TABLE IF NOT EXISTS forgetting_closure_receipts (
    receipt_id              TEXT PRIMARY KEY,
    caller_idempotency_key  TEXT NOT NULL UNIQUE,
    payload_digest          TEXT NOT NULL,
    receipt_json            TEXT NOT NULL,
    receipt_digest          TEXT NOT NULL,
    created_at              TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS forgetting_receipts_no_update
BEFORE UPDATE ON forgetting_closure_receipts BEGIN
    SELECT RAISE(ABORT, 'forgetting receipts are immutable');
END;
CREATE TRIGGER IF NOT EXISTS forgetting_receipts_no_delete
BEFORE DELETE ON forgetting_closure_receipts BEGIN
    SELECT RAISE(ABORT, 'forgetting receipts are immutable');
END;
"#;

/// V33 migration: append-only shadow policy proposals, versions, and promotion receipts.
const MIGRATION_V33: &str = r#"
CREATE TABLE IF NOT EXISTS shadow_policy_proposals (
    proposal_id       TEXT PRIMARY KEY,
    idempotency_key   TEXT NOT NULL UNIQUE,
    principal         TEXT NOT NULL,
    policy_kind       TEXT NOT NULL,
    proposal_digest   TEXT NOT NULL,
    proposal_json     TEXT NOT NULL,
    status            TEXT NOT NULL,
    created_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shadow_policy_proposals_principal
    ON shadow_policy_proposals(principal, policy_kind, created_at);
CREATE TRIGGER IF NOT EXISTS shadow_policy_proposals_no_update
BEFORE UPDATE ON shadow_policy_proposals BEGIN
    SELECT RAISE(ABORT, 'shadow policy proposals are append-only');
END;
CREATE TRIGGER IF NOT EXISTS shadow_policy_proposals_no_delete
BEFORE DELETE ON shadow_policy_proposals BEGIN
    SELECT RAISE(ABORT, 'shadow policy proposals are append-only');
END;

CREATE TABLE IF NOT EXISTS shadow_policy_versions (
    principal          TEXT NOT NULL,
    policy_kind        TEXT NOT NULL,
    version            INTEGER NOT NULL CHECK (version > 0),
    policy_json        TEXT NOT NULL,
    policy_digest      TEXT NOT NULL,
    proposal_id        TEXT NOT NULL REFERENCES shadow_policy_proposals(proposal_id),
    activated_by       TEXT NOT NULL,
    activated_at       TEXT NOT NULL,
    PRIMARY KEY (principal, policy_kind, version)
);
CREATE TRIGGER IF NOT EXISTS shadow_policy_versions_no_update
BEFORE UPDATE ON shadow_policy_versions BEGIN
    SELECT RAISE(ABORT, 'shadow policy versions are append-only');
END;
CREATE TRIGGER IF NOT EXISTS shadow_policy_versions_no_delete
BEFORE DELETE ON shadow_policy_versions BEGIN
    SELECT RAISE(ABORT, 'shadow policy versions are append-only');
END;

CREATE TABLE IF NOT EXISTS shadow_active_policies (
    principal          TEXT NOT NULL,
    policy_kind        TEXT NOT NULL,
    version            INTEGER NOT NULL,
    policy_json        TEXT NOT NULL,
    policy_digest      TEXT NOT NULL,
    source_proposal_id TEXT NOT NULL,
    activated_by       TEXT NOT NULL,
    activated_at       TEXT NOT NULL,
    PRIMARY KEY (principal, policy_kind),
    FOREIGN KEY (principal, policy_kind, version)
        REFERENCES shadow_policy_versions(principal, policy_kind, version)
);

CREATE TABLE IF NOT EXISTS shadow_policy_receipts (
    receipt_id              TEXT PRIMARY KEY,
    caller_idempotency_key  TEXT NOT NULL UNIQUE,
    proposal_id             TEXT NOT NULL,
    principal               TEXT NOT NULL,
    policy_kind             TEXT NOT NULL,
    evidence_digest         TEXT NOT NULL,
    status                  TEXT NOT NULL,
    receipt_json            TEXT NOT NULL,
    receipt_digest          TEXT NOT NULL,
    created_at              TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_shadow_policy_receipts_proposal
    ON shadow_policy_receipts(proposal_id, created_at);
CREATE TRIGGER IF NOT EXISTS shadow_policy_receipts_no_update
BEFORE UPDATE ON shadow_policy_receipts BEGIN
    SELECT RAISE(ABORT, 'shadow policy receipts are immutable');
END;
CREATE TRIGGER IF NOT EXISTS shadow_policy_receipts_no_delete
BEFORE DELETE ON shadow_policy_receipts BEGIN
    SELECT RAISE(ABORT, 'shadow policy receipts are immutable');
END;
"#;

/// V34 migration: isolated, immutable procedural-memory artifacts and lifecycle receipts.
///
/// These tables deliberately have no FTS, vector, fact, claim, or authority-lineage bridge.
/// Procedures can therefore only be reached through the governed procedural API.
const MIGRATION_V34: &str = r#"
CREATE TABLE IF NOT EXISTS procedural_memory_artifacts (
    artifact_id        TEXT PRIMARY KEY,
    principal          TEXT NOT NULL,
    capability_domain  TEXT NOT NULL,
    capability_name    TEXT NOT NULL,
    action_kind        TEXT NOT NULL,
    version            INTEGER NOT NULL CHECK (version > 0),
    supersedes         TEXT,
    artifact_digest    TEXT NOT NULL UNIQUE,
    artifact_json      TEXT NOT NULL,
    created_at         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_procedural_memory_lookup
    ON procedural_memory_artifacts(principal, capability_domain, capability_name, action_kind, version DESC);
CREATE TRIGGER IF NOT EXISTS procedural_memory_artifacts_no_update
BEFORE UPDATE ON procedural_memory_artifacts BEGIN
    SELECT RAISE(ABORT, 'procedural memory artifacts are immutable');
END;
CREATE TRIGGER IF NOT EXISTS procedural_memory_artifacts_no_delete
BEFORE DELETE ON procedural_memory_artifacts BEGIN
    SELECT RAISE(ABORT, 'procedural memory artifacts are immutable');
END;

CREATE TABLE IF NOT EXISTS procedural_memory_events (
    event_id           TEXT PRIMARY KEY,
    artifact_id        TEXT NOT NULL REFERENCES procedural_memory_artifacts(artifact_id),
    disposition        TEXT NOT NULL,
    reason_digest      TEXT NOT NULL,
    test_receipt_json  TEXT,
    prior_event_digest TEXT,
    event_digest       TEXT NOT NULL UNIQUE,
    created_at         TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_procedural_memory_events_artifact
    ON procedural_memory_events(artifact_id, created_at, event_id);
CREATE TRIGGER IF NOT EXISTS procedural_memory_events_no_update
BEFORE UPDATE ON procedural_memory_events BEGIN
    SELECT RAISE(ABORT, 'procedural memory events are append-only');
END;
CREATE TRIGGER IF NOT EXISTS procedural_memory_events_no_delete
BEFORE DELETE ON procedural_memory_events BEGIN
    SELECT RAISE(ABORT, 'procedural memory events are append-only');
END;

CREATE TABLE IF NOT EXISTS procedural_memory_receipts (
    receipt_id              TEXT PRIMARY KEY,
    caller_idempotency_key  TEXT NOT NULL UNIQUE,
    operation               TEXT NOT NULL,
    payload_digest          TEXT NOT NULL,
    artifact_id             TEXT NOT NULL,
    receipt_json            TEXT NOT NULL,
    receipt_digest          TEXT NOT NULL UNIQUE,
    created_at              TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_procedural_memory_receipts_artifact
    ON procedural_memory_receipts(artifact_id, created_at);
CREATE TRIGGER IF NOT EXISTS procedural_memory_receipts_no_update
BEFORE UPDATE ON procedural_memory_receipts BEGIN
    SELECT RAISE(ABORT, 'procedural memory receipts are immutable');
END;
CREATE TRIGGER IF NOT EXISTS procedural_memory_receipts_no_delete
BEFORE DELETE ON procedural_memory_receipts BEGIN
    SELECT RAISE(ABORT, 'procedural memory receipts are immutable');
END;
"#;

/// V35 migration: opt-in, privacy-sensitive inputs for complete search replay.
const MIGRATION_V35: &str = r#"
CREATE TABLE IF NOT EXISTS replay_inputs (
    receipt_id TEXT PRIMARY KEY,
    query_text TEXT NOT NULL,
    namespaces_json TEXT,
    source_types_json TEXT,
    stored_at TEXT NOT NULL DEFAULT (datetime('now'))
);
"#;

/// V36 migration: durable sparse vectors keyed by canonical search item ID.
const MIGRATION_V36: &str = r#"
CREATE TABLE IF NOT EXISTS sparse_vectors (
    item_key TEXT PRIMARY KEY,
    entries_json TEXT NOT NULL,
    representation TEXT NOT NULL,
    updated_at TEXT NOT NULL DEFAULT (datetime('now'))
);

CREATE TRIGGER IF NOT EXISTS sparse_vectors_delete_fact
AFTER DELETE ON facts BEGIN
    DELETE FROM sparse_vectors WHERE item_key = 'fact:' || OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS sparse_vectors_delete_chunk
AFTER DELETE ON chunks BEGIN
    DELETE FROM sparse_vectors WHERE item_key = 'chunk:' || OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS sparse_vectors_delete_message
AFTER DELETE ON messages BEGIN
    DELETE FROM sparse_vectors WHERE item_key = 'msg:' || OLD.id;
END;

CREATE TRIGGER IF NOT EXISTS sparse_vectors_delete_episode
AFTER DELETE ON episodes BEGIN
    DELETE FROM sparse_vectors WHERE item_key = 'episode:' || OLD.episode_id;
END;
"#;

/// Ordered list of migrations.
#[allow(deprecated)]
const MIGRATIONS: &[(u32, &str)] = &[
    (1, MIGRATION_V1),
    (2, MIGRATION_V2),
    (3, MIGRATION_V3),
    (4, MIGRATION_V4),
    (5, MIGRATION_V5),
    (6, MIGRATION_V6),
    (7, MIGRATION_V7),
    (8, MIGRATION_V8),
    (9, MIGRATION_V9),
    (10, crate::projection_import::MIGRATION_V10),
    (11, crate::projection_storage::MIGRATION_V11),
    (12, crate::projection_storage::MIGRATION_V12),
    (13, crate::projection_storage::MIGRATION_V13),
    (14, crate::projection_storage::MIGRATION_V14),
    (15, crate::projection_storage::MIGRATION_V15),
    (16, crate::projection_storage::MIGRATION_V16),
    (17, crate::projection_storage::MIGRATION_V17),
    (18, MIGRATION_V18),
    (19, MIGRATION_V19),
    (20, MIGRATION_V20),
    (21, MIGRATION_V21),
    (22, MIGRATION_V22),
    (23, MIGRATION_V23),
    (24, MIGRATION_V24),
    (25, MIGRATION_V25),
    (26, MIGRATION_V26),
    (27, MIGRATION_V27),
    (28, MIGRATION_V28),
    (29, MIGRATION_V29),
    (30, MIGRATION_V30),
    (31, MIGRATION_V31),
    (32, MIGRATION_V32),
    (33, MIGRATION_V33),
    (34, MIGRATION_V34),
    (35, MIGRATION_V35),
    (36, MIGRATION_V36),
];

/// Maximum schema version this build supports.
pub const MAX_SCHEMA_VERSION: u32 = 36;

/// Procedural migration for V9: rebuild episodes table with episode_id PK.
fn run_migration_v9(conn: &Connection) -> Result<(), MemoryError> {
    // Check if episodes table exists (fresh DBs won't have it yet at V6)
    let episodes_exist: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='episodes'",
            [],
            |row| row.get(0),
        )
        .map_err(|e| MemoryError::MigrationFailed {
            version: 9,
            reason: format!("existence check failed: {e}"),
        })?;

    if !episodes_exist {
        // No episodes table to migrate; create the target schema directly
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS episode_causes (
                 episode_id    TEXT NOT NULL,
                 cause_node_id TEXT NOT NULL,
                 ordinal       INTEGER NOT NULL DEFAULT 0,
                 PRIMARY KEY (episode_id, cause_node_id)
             );
             CREATE INDEX IF NOT EXISTS idx_episode_causes_cause ON episode_causes(cause_node_id);",
        )?;
        return Ok(());
    }

    // Disable foreign keys for table rebuild
    conn.execute_batch("PRAGMA foreign_keys = OFF;")?;

    conn.execute_batch(
        "CREATE TABLE episodes_new (
             episode_id  TEXT PRIMARY KEY,
             document_id TEXT NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
             cause_ids   TEXT NOT NULL,
             effect_type TEXT NOT NULL,
             outcome     TEXT NOT NULL DEFAULT 'pending',
             confidence  REAL NOT NULL DEFAULT 0.0,
             verification_status TEXT NOT NULL DEFAULT '{\"status\":\"unverified\"}',
             experiment_id TEXT,
             created_at  TEXT NOT NULL DEFAULT (datetime('now')),
             updated_at  TEXT NOT NULL DEFAULT (datetime('now')),
             search_text TEXT NOT NULL DEFAULT '',
             embedding   BLOB,
             embedding_q8 BLOB,
             trace_id    TEXT
         )",
    )?;

    // Migrate existing data with deterministic episode_id
    conn.execute_batch(
        "INSERT INTO episodes_new
             (episode_id, document_id, cause_ids, effect_type, outcome, confidence,
              verification_status, experiment_id, created_at, updated_at,
              search_text, embedding, embedding_q8, trace_id)
         SELECT
             document_id || '-ep0',
             document_id, cause_ids, effect_type, outcome, confidence,
             verification_status, experiment_id, created_at, updated_at,
             search_text, embedding, embedding_q8, trace_id
         FROM episodes",
    )?;

    conn.execute_batch("DROP TABLE episodes")?;
    conn.execute_batch("ALTER TABLE episodes_new RENAME TO episodes")?;

    conn.execute_batch(
        "CREATE INDEX idx_episodes_document_id ON episodes(document_id);
         CREATE INDEX idx_episodes_effect_type ON episodes(effect_type);
         CREATE INDEX idx_episodes_outcome ON episodes(outcome);
         CREATE INDEX idx_episodes_experiment_id ON episodes(experiment_id);",
    )?;

    // Rebuild episodes_rowid_map with episode_id
    conn.execute_batch(
        "DROP TABLE IF EXISTS episodes_rowid_map;
         CREATE TABLE episodes_rowid_map (
             rowid       INTEGER PRIMARY KEY AUTOINCREMENT,
             episode_id  TEXT NOT NULL UNIQUE,
             document_id TEXT
         );
         INSERT INTO episodes_rowid_map (episode_id, document_id)
         SELECT episode_id, document_id FROM episodes;",
    )?;

    // Rebuild episodes FTS
    conn.execute_batch(
        "DROP TABLE IF EXISTS episodes_fts;
         CREATE VIRTUAL TABLE episodes_fts USING fts5(
             content,
             content='',
             content_rowid='rowid',
             tokenize='porter unicode61'
         );
         INSERT INTO episodes_fts (rowid, content)
         SELECT rm.rowid, e.search_text
         FROM episodes_rowid_map rm
         JOIN episodes e ON e.episode_id = rm.episode_id;",
    )?;

    // Normalized causal edge table
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS episode_causes (
             episode_id    TEXT NOT NULL,
             cause_node_id TEXT NOT NULL,
             ordinal       INTEGER NOT NULL DEFAULT 0,
             PRIMARY KEY (episode_id, cause_node_id)
         );
         CREATE INDEX IF NOT EXISTS idx_episode_causes_cause ON episode_causes(cause_node_id);",
    )?;

    // Populate edge table from existing JSON cause_ids
    conn.execute_batch(
        "INSERT OR IGNORE INTO episode_causes (episode_id, cause_node_id, ordinal)
         SELECT e.episode_id, je.value, CAST(je.key AS INTEGER)
         FROM episodes e, json_each(e.cause_ids) je;",
    )?;

    conn.execute_batch("PRAGMA foreign_keys = ON;")?;

    Ok(())
}

/// How thorough the integrity check should be.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyMode {
    /// Quick: counts and basic metadata only.
    Quick,
    /// Full: includes FTS, JSON/enum decoding, blobs, and SQLite integrity_check.
    Full,
}

/// Result of an integrity verification.
#[derive(Debug, Clone)]
pub struct IntegrityReport {
    pub ok: bool,
    pub schema_version: u32,
    pub fact_count: usize,
    pub chunk_count: usize,
    pub message_count: usize,
    pub facts_missing_embeddings: usize,
    pub chunks_missing_embeddings: usize,
    pub issues: Vec<String>,
}

/// Action to take when integrity issues are found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconcileAction {
    ReportOnly,
    RebuildFts,
    ReEmbed,
}

/// Desired HNSW sidecar mutation queued in SQLite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum IndexOpKind {
    Upsert,
    Delete,
}

impl IndexOpKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Upsert => "upsert",
            Self::Delete => "delete",
        }
    }

    fn parse(raw: &str, item_key: &str) -> Result<Self, MemoryError> {
        match raw {
            "upsert" => Ok(Self::Upsert),
            "delete" => Ok(Self::Delete),
            other => Err(MemoryError::CorruptData {
                table: "pending_index_ops",
                row_id: item_key.to_string(),
                detail: format!("invalid op_kind '{other}'"),
            }),
        }
    }
}

/// Durable sidecar repair record.
#[derive(Debug, Clone)]
pub(crate) struct PendingIndexOp {
    pub item_key: String,
    pub entity_type: String,
    pub op_kind: IndexOpKind,
    pub attempt_count: u32,
    pub last_error: Option<String>,
}

/// Run a closure inside an unchecked transaction, committing on success.
pub fn with_transaction<F, T>(conn: &Connection, f: F) -> Result<T, MemoryError>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, MemoryError>,
{
    let tx = conn.unchecked_transaction()?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

/// Open or create a SQLite database, configure pragmas, and run migrations.
pub fn open_database(
    path: &Path,
    pool: &PoolConfig,
    limits: &MemoryLimits,
) -> Result<Connection, MemoryError> {
    open_database_internal(path, pool, limits.max_db_size_bytes, true)
}

/// Open a SQLite connection with pragmas applied but without running migrations.
#[allow(dead_code)] // public API — used by external consumers, not internally
pub fn open_database_connection(
    path: &Path,
    pool: &PoolConfig,
    limits: &MemoryLimits,
) -> Result<Connection, MemoryError> {
    open_database_internal(path, pool, limits.max_db_size_bytes, false)
}

pub(crate) fn open_database_internal(
    path: &Path,
    pool: &PoolConfig,
    max_db_size_bytes: u64,
    run_schema_migrations: bool,
) -> Result<Connection, MemoryError> {
    create_parent_dirs(path)?;
    let conn = Connection::open(path)?;
    configure_connection(&conn, path, pool, max_db_size_bytes, false)?;
    if run_schema_migrations {
        run_migrations(&conn)?;
    }
    Ok(conn)
}

pub(crate) fn open_pool_member_connection(
    path: &Path,
    pool: &PoolConfig,
    limits: &MemoryLimits,
    query_only: bool,
) -> Result<Connection, MemoryError> {
    create_parent_dirs(path)?;
    let flags = OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE;
    let conn = Connection::open_with_flags(path, flags)?;
    configure_connection(&conn, path, pool, limits.max_db_size_bytes, query_only)?;
    Ok(conn)
}

fn create_parent_dirs(path: &Path) -> Result<(), MemoryError> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MemoryError::StorageError(format!(
                    "failed to create database directory {}: {}",
                    parent.display(),
                    e
                ))
            })?;
        }
    }
    Ok(())
}

fn configure_connection(
    conn: &Connection,
    path: &Path,
    pool: &PoolConfig,
    max_db_size_bytes: u64,
    query_only: bool,
) -> Result<(), MemoryError> {
    let journal_mode = if pool.enable_wal { "WAL" } else { "DELETE" };
    conn.execute_batch(&format!(
        "PRAGMA journal_mode = {};
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = {};
         PRAGMA synchronous = NORMAL;
         PRAGMA temp_store = MEMORY;
         PRAGMA wal_autocheckpoint = {};
         PRAGMA cache_size = -25600;
         PRAGMA mmap_size = 268435456;",
        journal_mode, pool.busy_timeout_ms, pool.wal_autocheckpoint,
    ))?;

    if query_only {
        conn.execute_batch("PRAGMA query_only = ON;")?;
    }

    let actual_journal_mode: String =
        conn.query_row("PRAGMA journal_mode", [], |row| row.get(0))?;
    let expected_journal_mode = if pool.enable_wal { "wal" } else { "delete" };
    if actual_journal_mode.to_lowercase() != expected_journal_mode {
        return Err(MemoryError::StorageError(format!(
            "SQLite journal mode mismatch for {}: requested {}, got {}",
            path.display(),
            expected_journal_mode,
            actual_journal_mode
        )));
    }

    if max_db_size_bytes > 0 {
        let page_size: u64 = conn.query_row("PRAGMA page_size", [], |row| row.get(0))?;
        let max_page_count = max_db_size_bytes.div_ceil(page_size);

        // SM-AUD-0065: Validate max_page_count before setting pragma
        const MAX_SQLITE_PAGE_COUNT: u64 = 1_073_741_823; // SQLite hard limit
        const MIN_SQLITE_PAGE_COUNT: u64 = 1;
        if !(MIN_SQLITE_PAGE_COUNT..=MAX_SQLITE_PAGE_COUNT).contains(&max_page_count) {
            return Err(MemoryError::StorageError(format!(
                "Invalid max_page_count {}: must be between {} and {}",
                max_page_count, MIN_SQLITE_PAGE_COUNT, MAX_SQLITE_PAGE_COUNT
            )));
        }

        let actual_max_page_count: u64 = conn.query_row(
            &format!("PRAGMA max_page_count = {}", max_page_count),
            [],
            |row| row.get(0),
        )?;
        let page_count: u64 = conn.query_row("PRAGMA page_count", [], |row| row.get(0))?;

        if page_count > actual_max_page_count {
            return Err(MemoryError::DatabaseSizeLimitExceeded {
                current: page_count.saturating_mul(page_size),
                limit: max_db_size_bytes,
            });
        }
    }

    // SM-AUD-0064: Assert foreign_keys is ON after configuration
    let foreign_keys_enabled: bool = conn.query_row("PRAGMA foreign_keys", [], |row| row.get(0))?;
    if !foreign_keys_enabled {
        return Err(MemoryError::StorageError(
            "PRAGMA foreign_keys failed to enable after configuration".to_string(),
        ));
    }

    Ok(())
}

/// Run all pending migrations.
pub fn run_migrations(conn: &Connection) -> Result<(), MemoryError> {
    let user_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|e| MemoryError::MigrationFailed {
            version: 0,
            reason: format!("failed to read PRAGMA user_version: {e}"),
        })?;

    if user_version > MAX_SCHEMA_VERSION {
        return Err(MemoryError::SchemaAhead {
            found: user_version,
            supported: MAX_SCHEMA_VERSION,
        });
    }

    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _schema_version (
            version     INTEGER PRIMARY KEY,
            applied_at  TEXT NOT NULL DEFAULT (datetime('now'))
        );",
    )?;

    for &(version, sql) in MIGRATIONS {
        let current_version: u32 = conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if current_version >= version {
            continue;
        }

        with_transaction(conn, |tx| {
            match version {
                9 => run_migration_v9(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                16 => run_migration_v16(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                17 => run_migration_v17(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                20 => run_migration_v20(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                21 => run_migration_v21(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                26 => run_migration_v26(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                28 => run_migration_v28(tx).map_err(|e| MemoryError::MigrationFailed {
                    version,
                    reason: e.to_string(),
                })?,
                _ => tx
                    .execute_batch(sql)
                    .map_err(|e| MemoryError::MigrationFailed {
                        version,
                        reason: e.to_string(),
                    })?,
            }
            tx.execute(
                "INSERT INTO _schema_version (version) VALUES (?1)",
                params![version],
            )
            .map_err(|e| MemoryError::MigrationFailed {
                version,
                reason: e.to_string(),
            })?;
            Ok(())
        })?;

        tracing::info!("Applied migration V{}", version);
    }

    // Keep the authority surface self-healing if a database carries the V29
    // marker but one of its idempotent tables was removed by an interrupted
    // setup or an older repair tool.
    with_transaction(conn, |tx| {
        tx.execute_batch(MIGRATION_V29)
            .map_err(MemoryError::Database)
    })?;
    with_transaction(conn, |tx| {
        tx.execute_batch(MIGRATION_V30)
            .map_err(MemoryError::Database)
    })?;
    with_transaction(conn, |tx| {
        tx.execute_batch(MIGRATION_V31)
            .map_err(MemoryError::Database)
    })?;

    let final_version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _schema_version",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    conn.execute_batch(&format!("PRAGMA user_version = {};", final_version))?;

    Ok(())
}

fn run_migration_v16(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(conn, "projection_import_log", "kernel_payload_json", "TEXT")?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "kernel_payload_json",
        "TEXT",
    )?;
    Ok(())
}

fn run_migration_v17(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(conn, "projection_import_log", "episode_bundle_id", "TEXT")?;
    add_column_if_missing(conn, "projection_import_log", "episode_bundle_json", "TEXT")?;
    add_column_if_missing(
        conn,
        "projection_import_log",
        "execution_context_json",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "episode_bundle_id",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "episode_bundle_json",
        "TEXT",
    )?;
    add_column_if_missing(
        conn,
        "projection_import_failures",
        "execution_context_json",
        "TEXT",
    )?;
    Ok(())
}

fn run_migration_v20(conn: &Connection) -> Result<(), rusqlite::Error> {
    add_column_if_missing(conn, "derived_vector_artifacts", "encoded_digest", "TEXT")?;
    conn.execute(
        "UPDATE derived_vector_artifacts
         SET encoded_digest = artifact_digest
         WHERE encoded_digest IS NULL OR encoded_digest = ''",
        [],
    )?;
    add_column_if_missing(
        conn,
        "derived_vector_artifacts",
        "encoding",
        "TEXT NOT NULL DEFAULT 'turbo_code_wire_v1'",
    )?;
    add_column_if_missing(
        conn,
        "derived_vector_artifacts",
        "dim",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    add_column_if_missing(
        conn,
        "derived_vector_artifacts",
        "status",
        "TEXT NOT NULL DEFAULT 'active'",
    )?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_profile
         ON derived_vector_artifacts(codec_family, codec_profile_digest, status);
         CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_source_digest
         ON derived_vector_artifacts(source_embedding_digest);",
    )?;
    Ok(())
}

fn run_migration_v21(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(MIGRATION_V21)?;
    add_column_if_missing(conn, "derived_vector_artifacts", "generation_id", "TEXT")?;
    conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_derived_vector_artifacts_generation
         ON derived_vector_artifacts(generation_id, status);",
    )?;
    Ok(())
}

const SEARCH_RECEIPT_SCHEMA_VERSION: &str = "vector_search_receipt_v1";

#[derive(Debug, Serialize, Deserialize)]
struct StoredVectorSearchReceiptV1 {
    #[serde(default = "default_search_receipt_schema_version")]
    schema_version: String,
    receipt_id: String,
    evaluation_time: DateTime<Utc>,
    #[serde(default)]
    receipt_digest: Option<String>,
    #[serde(default)]
    trace_id: Option<String>,
    #[serde(default)]
    attempt_family_id: Option<String>,
    #[serde(default)]
    attempt_id: Option<String>,
    #[serde(default)]
    replay_of: Option<String>,
    query_embedding_digest: Option<String>,
    #[serde(default)]
    query_text_digest: Option<String>,
    #[serde(default)]
    query_input_digest: Option<String>,
    #[serde(default)]
    filter_digest: Option<String>,
    #[serde(default)]
    redaction_state: Option<String>,
    #[serde(default)]
    budget_id: Option<String>,
    #[serde(default)]
    deadline_at: Option<DateTime<Utc>>,
    search_profile: String,
    candidate_backend: String,
    codec_family: Option<String>,
    codec_profile_digest: Option<String>,
    #[serde(default)]
    artifact_profile_digest: Option<String>,
    #[serde(default)]
    artifact_count: Option<u64>,
    #[serde(default)]
    artifact_corruption_count: Option<u64>,
    #[serde(default)]
    artifact_missing_count: Option<u64>,
    #[serde(default)]
    vector_artifact_manifest_digest: Option<String>,
    #[serde(default)]
    artifact_generation_id: Option<String>,
    #[serde(default)]
    approximate_scanned_count: Option<u64>,
    #[serde(default)]
    approximate_returned_count: Option<u64>,
    #[serde(default)]
    raw_rows_loaded_count: Option<u64>,
    #[serde(default)]
    filter_strategy: Option<String>,
    #[serde(default)]
    vector_artifact_count: Option<u64>,
    #[serde(default)]
    vector_artifact_missing_count: Option<u64>,
    #[serde(default)]
    vector_artifact_stale_count: Option<u64>,
    #[serde(default)]
    exact_rerank_count: Option<u64>,
    #[serde(default)]
    approximate_candidate_count: Option<u64>,
    #[serde(default)]
    fallback_reason: Option<String>,
    approximate: bool,
    requested_candidates: u64,
    returned_candidates: u64,
    post_filter_candidates: u64,
    #[serde(default)]
    sparse_enabled: bool,
    #[serde(default)]
    sparse_weight: Option<f64>,
    #[serde(default)]
    sparse_query_nonzero_count: Option<u64>,
    #[serde(default)]
    sparse_candidate_count: Option<u64>,
    #[serde(default)]
    sparse_representations: Vec<String>,
    #[serde(default)]
    sparse_result_ranks: Vec<SparseRankReceiptV1>,
    fallback: Option<String>,
    exact_rerank: bool,
    result_ids: Vec<String>,
    degradations: Vec<String>,
}

fn default_search_receipt_schema_version() -> String {
    SEARCH_RECEIPT_SCHEMA_VERSION.to_string()
}

fn b3_digest(bytes: &[u8]) -> String {
    format!("blake3:{}", ContentDigest::compute(bytes).hex())
}

/// Row from the derived vector artifact store.
#[cfg(feature = "turbo-quant-codec")]
#[derive(Debug, Clone)]
pub(crate) struct DerivedVectorArtifactRow {
    pub item_key: String,
    pub generation_id: Option<String>,
    pub codec_family: String,
    pub codec_profile_digest: String,
    pub source_embedding_digest: String,
    pub encoded_digest: String,
    pub encoding: String,
    pub dim: usize,
    pub status: String,
    pub encoded: Vec<u8>,
    // Codec governance columns (V23 migration)
    pub codec_governance_receipt_id: Option<String>,
    pub codec_profile: Option<String>,
    pub degradation_budget: Option<f64>,
    pub raw_source_artifact_id: Option<String>,
}

/// Active derived vector artifact generation row.
#[cfg(feature = "turbo-quant-codec")]
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct DerivedVectorArtifactGenerationRow {
    pub generation_id: String,
    pub codec_family: String,
    pub codec_profile_digest: String,
    pub source_snapshot_digest: String,
    pub source_row_count: usize,
    pub artifact_count: usize,
    pub dim: usize,
    pub encoding: String,
    pub artifact_manifest_digest: String,
    pub status: String,
}

/// Stable digest for an authoritative raw f32 embedding BLOB.
#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn source_embedding_digest(
    blob: &[u8],
    expected_dim: usize,
) -> Result<String, MemoryError> {
    validate_vector_blob_len(blob, expected_dim)?;
    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.source_embedding.v1")
        .separator()
        .update(&(expected_dim as u64).to_le_bytes())
        .separator()
        .update(blob);
    Ok(format!("blake3:{}", builder.finalize().hex()))
}

#[cfg(feature = "turbo-quant-codec")]
fn source_snapshot_digest(rows: &[DerivedVectorArtifactRow], dim: usize) -> String {
    let mut entries = rows
        .iter()
        .map(|row| (row.item_key.as_str(), row.source_embedding_digest.as_str()))
        .collect::<Vec<_>>();
    entries.sort_unstable();

    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.vector_source_snapshot.v1")
        .separator()
        .update(&(dim as u64).to_le_bytes())
        .separator();
    for (item_key, source_embedding_digest) in entries {
        builder
            .update_str(item_key)
            .separator()
            .update_str(source_embedding_digest)
            .separator();
    }
    format!("blake3:{}", builder.finalize().hex())
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn current_source_snapshot_digest(
    conn: &Connection,
    dim: usize,
) -> Result<(String, usize), MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT 'fact:' || id AS item_key, embedding FROM facts WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'chunk:' || id AS item_key, embedding FROM chunks WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'msg:' || id AS item_key, embedding FROM messages WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'episode:' || episode_id AS item_key, embedding FROM episodes WHERE embedding IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;

    let mut entries = Vec::new();
    for row in rows {
        let (item_key, blob) = row?;
        entries.push((item_key, source_embedding_digest(&blob, dim)?));
    }
    entries.sort_unstable();

    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.vector_source_snapshot.v1")
        .separator()
        .update(&(dim as u64).to_le_bytes())
        .separator();
    for (item_key, source_embedding_digest) in &entries {
        builder
            .update_str(item_key)
            .separator()
            .update_str(source_embedding_digest)
            .separator();
    }
    Ok((
        format!("blake3:{}", builder.finalize().hex()),
        entries.len(),
    ))
}

#[cfg(feature = "turbo-quant-codec")]
fn derived_artifact_manifest_digest(rows: &[DerivedVectorArtifactRow]) -> String {
    let mut entries = rows
        .iter()
        .map(|row| {
            (
                row.item_key.as_str(),
                row.source_embedding_digest.as_str(),
                row.encoded_digest.as_str(),
            )
        })
        .collect::<Vec<_>>();
    entries.sort_unstable();

    let mut builder = DigestBuilder::new();
    builder
        .update_str("semantic-memory.vector_artifact_manifest.v1")
        .separator();
    for (item_key, source_embedding_digest, encoded_digest) in entries {
        builder
            .update_str(item_key)
            .separator()
            .update_str(source_embedding_digest)
            .separator()
            .update_str(encoded_digest)
            .separator();
    }
    format!("blake3:{}", builder.finalize().hex())
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn upsert_derived_vector_artifact(
    conn: &Connection,
    row: &DerivedVectorArtifactRow,
) -> Result<(), MemoryError> {
    conn.execute(
        "INSERT OR REPLACE INTO derived_vector_artifacts
             (item_key, generation_id, codec_family, codec_profile_digest, source_embedding_digest,
              encoded_digest, artifact_digest, encoding, dim, encoded, created_at, status,
              codec_governance_receipt_id, codec_profile, degradation_budget, raw_source_artifact_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6, ?7, ?8, ?9, datetime('now'), ?10, ?11, ?12, ?13, ?14)",
        params![
            row.item_key,
            row.generation_id.as_deref(),
            row.codec_family,
            row.codec_profile_digest,
            row.source_embedding_digest,
            row.encoded_digest,
            row.encoding,
            i64::try_from(row.dim)
                .map_err(|err| MemoryError::Other(format!("artifact dim overflow: {err}")))?,
            row.encoded,
            row.status,
            row.codec_governance_receipt_id.as_deref(),
            row.codec_profile.as_deref(),
            row.degradation_budget,
            row.raw_source_artifact_id.as_deref(),
        ],
    )?;
    Ok(())
}

#[allow(dead_code)] // public API — used by external consumers, not internally
pub fn delete_derived_vector_artifact(
    conn: &Connection,
    item_key: &str,
) -> Result<(), MemoryError> {
    conn.execute(
        "DELETE FROM derived_vector_artifacts WHERE item_key = ?1",
        params![item_key],
    )?;
    Ok(())
}

pub fn invalidate_derived_vector_artifact(
    conn: &Connection,
    item_key: &str,
) -> Result<(), MemoryError> {
    conn.execute(
        "UPDATE derived_vector_artifacts
         SET status = 'invalidated'
         WHERE item_key = ?1 AND status = 'active'",
        params![item_key],
    )?;
    conn.execute(
        "UPDATE derived_vector_artifact_generations
         SET status = 'invalidated'
         WHERE status = 'active'",
        [],
    )?;
    Ok(())
}

#[cfg(feature = "turbo-quant-codec")]
#[allow(dead_code)]
pub(crate) fn load_derived_vector_artifacts_by_profile(
    conn: &Connection,
    codec_family: &str,
    codec_profile_digest: &str,
) -> Result<Vec<DerivedVectorArtifactRow>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT item_key, generation_id, codec_family, codec_profile_digest, source_embedding_digest,
                encoded_digest, encoding, dim, status, encoded,
                codec_governance_receipt_id, codec_profile, degradation_budget, raw_source_artifact_id
         FROM derived_vector_artifacts
         WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'",
    )?;
    let rows = stmt.query_map(params![codec_family, codec_profile_digest], |row| {
        let dim_i64: i64 = row.get(7)?;
        Ok(DerivedVectorArtifactRow {
            item_key: row.get(0)?,
            generation_id: row.get(1)?,
            codec_family: row.get(2)?,
            codec_profile_digest: row.get(3)?,
            source_embedding_digest: row.get(4)?,
            encoded_digest: row.get(5)?,
            encoding: row.get(6)?,
            dim: usize::try_from(dim_i64).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Integer,
                    Box::new(err),
                )
            })?,
            status: row.get(8)?,
            encoded: row.get(9)?,
            codec_governance_receipt_id: row.get(10)?,
            codec_profile: row.get(11)?,
            degradation_budget: row.get(12)?,
            raw_source_artifact_id: row.get(13)?,
        })
    })?;

    let mut artifacts = Vec::new();
    for row in rows {
        artifacts.push(row?);
    }
    Ok(artifacts)
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn load_derived_vector_artifacts_by_generation(
    conn: &Connection,
    generation_id: &str,
) -> Result<Vec<DerivedVectorArtifactRow>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT item_key, generation_id, codec_family, codec_profile_digest, source_embedding_digest,
                encoded_digest, encoding, dim, status, encoded,
                codec_governance_receipt_id, codec_profile, degradation_budget, raw_source_artifact_id
         FROM derived_vector_artifacts
         WHERE generation_id = ?1 AND status = 'active'",
    )?;
    let rows = stmt.query_map(params![generation_id], |row| {
        let dim_i64: i64 = row.get(7)?;
        Ok(DerivedVectorArtifactRow {
            item_key: row.get(0)?,
            generation_id: row.get(1)?,
            codec_family: row.get(2)?,
            codec_profile_digest: row.get(3)?,
            source_embedding_digest: row.get(4)?,
            encoded_digest: row.get(5)?,
            encoding: row.get(6)?,
            dim: usize::try_from(dim_i64).map_err(|err| {
                rusqlite::Error::FromSqlConversionFailure(
                    7,
                    rusqlite::types::Type::Integer,
                    Box::new(err),
                )
            })?,
            status: row.get(8)?,
            encoded: row.get(9)?,
            codec_governance_receipt_id: row.get(10)?,
            codec_profile: row.get(11)?,
            degradation_budget: row.get(12)?,
            raw_source_artifact_id: row.get(13)?,
        })
    })?;

    let mut artifacts = Vec::new();
    for row in rows {
        artifacts.push(row?);
    }
    Ok(artifacts)
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn current_derived_vector_generation(
    conn: &Connection,
    codec_family: &str,
    codec_profile_digest: &str,
) -> Result<Option<DerivedVectorArtifactGenerationRow>, MemoryError> {
    conn.query_row(
        "SELECT generation_id, codec_family, codec_profile_digest, source_snapshot_digest,
                source_row_count, artifact_count, dim, encoding, artifact_manifest_digest, status
         FROM derived_vector_artifact_generations
         WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'
         ORDER BY created_at DESC
         LIMIT 1",
        params![codec_family, codec_profile_digest],
        |row| {
            let source_row_count: i64 = row.get(4)?;
            let artifact_count: i64 = row.get(5)?;
            let dim: i64 = row.get(6)?;
            Ok(DerivedVectorArtifactGenerationRow {
                generation_id: row.get(0)?,
                codec_family: row.get(1)?,
                codec_profile_digest: row.get(2)?,
                source_snapshot_digest: row.get(3)?,
                source_row_count: usize::try_from(source_row_count).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        4,
                        rusqlite::types::Type::Integer,
                        Box::new(err),
                    )
                })?,
                artifact_count: usize::try_from(artifact_count).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        5,
                        rusqlite::types::Type::Integer,
                        Box::new(err),
                    )
                })?,
                dim: usize::try_from(dim).map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        6,
                        rusqlite::types::Type::Integer,
                        Box::new(err),
                    )
                })?,
                encoding: row.get(7)?,
                artifact_manifest_digest: row.get(8)?,
                status: row.get(9)?,
            })
        },
    )
    .optional()
    .map_err(MemoryError::from)
}

#[allow(dead_code)] // public API — used by external consumers, not internally
pub fn count_derived_vector_artifacts(
    conn: &Connection,
    codec_family: &str,
    codec_profile_digest: &str,
) -> Result<usize, MemoryError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM derived_vector_artifacts
         WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'",
        params![codec_family, codec_profile_digest],
        |row| row.get(0),
    )?;
    usize::try_from(count)
        .map_err(|err| MemoryError::Other(format!("derived artifact count overflow: {err}")))
}

#[cfg(feature = "turbo-quant-codec")]
pub(crate) fn rebuild_turbo_quant_artifacts(
    conn: &Connection,
    dim: usize,
    bits: u8,
    projections: usize,
    seed: u64,
) -> Result<VectorArtifactBuildReceiptV1, MemoryError> {
    use crate::vector_codec::{TurboQuantCodec, VectorCodec};

    let started = std::time::Instant::now();
    let codec = TurboQuantCodec::new(dim, bits, projections, seed)?;
    let codec_profile_digest = codec.profile().digest();
    let generation_id = uuid::Uuid::new_v4().to_string();
    let mut source_row_count = 0usize;
    let mut artifact_count = 0usize;
    let mut skipped_row_count = 0usize;
    let mut degradations = Vec::new();

    let mut stmt = conn.prepare(
        "SELECT 'fact:' || id AS item_key, embedding FROM facts WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'chunk:' || id AS item_key, embedding FROM chunks WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'msg:' || id AS item_key, embedding FROM messages WHERE embedding IS NOT NULL
         UNION ALL
         SELECT 'episode:' || episode_id AS item_key, embedding FROM episodes WHERE embedding IS NOT NULL",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;

    let mut pending = Vec::new();
    for row in rows {
        let (item_key, blob) = row?;
        source_row_count += 1;
        let embedding = match decode_f32_le(&blob, dim) {
            Ok(embedding) => embedding,
            Err(err) => {
                skipped_row_count += 1;
                degradations.push(format!(
                    "skipped {item_key}: invalid authoritative embedding: {err}"
                ));
                continue;
            }
        };
        let artifact = match codec.encode(&embedding) {
            Ok(artifact) => artifact,
            Err(err) => {
                skipped_row_count += 1;
                degradations.push(format!("skipped {item_key}: encode failed: {err}"));
                continue;
            }
        };
        pending.push(DerivedVectorArtifactRow {
            item_key,
            generation_id: Some(generation_id.clone()),
            codec_family: "turbo_quant".to_string(),
            codec_profile_digest: codec_profile_digest.clone(),
            source_embedding_digest: source_embedding_digest(&blob, dim)?,
            encoded_digest: artifact.artifact_digest,
            encoding: "turbo_code_wire_v1".to_string(),
            dim,
            status: "active".to_string(),
            encoded: artifact.encoded,
            // V23 governance columns — populated by encode_governed path; existing
            // turbo-quant build path leaves these as None (nullable).
            codec_governance_receipt_id: None,
            codec_profile: None,
            degradation_budget: None,
            raw_source_artifact_id: None,
        });
    }
    drop(stmt);

    let build_receipt_id = uuid::Uuid::new_v4().to_string();
    let source_snapshot_digest = source_snapshot_digest(&pending, dim);
    let artifact_manifest_digest = derived_artifact_manifest_digest(&pending);
    let source_tables = vec![
        "facts".to_string(),
        "chunks".to_string(),
        "messages".to_string(),
        "episodes".to_string(),
    ];
    let generation_manifest = DerivedVectorArtifactGenerationV1 {
        schema_version: "derived_vector_artifact_generation_v1".to_string(),
        generation_id: generation_id.clone(),
        codec_family: "turbo_quant".to_string(),
        codec_profile_digest: codec_profile_digest.clone(),
        source_snapshot_digest: source_snapshot_digest.clone(),
        source_row_count,
        artifact_count: pending.len(),
        source_tables,
        dim,
        encoding: "turbo_code_wire_v1".to_string(),
        created_at: Utc::now(),
        build_receipt_id: Some(build_receipt_id.clone()),
        artifact_manifest_digest: artifact_manifest_digest.clone(),
        status: if skipped_row_count == 0 {
            "active".to_string()
        } else {
            "failed".to_string()
        },
        degradations: degradations.clone(),
    };

    with_transaction(conn, |tx| {
        tx.execute(
            "UPDATE derived_vector_artifact_generations
             SET status = 'superseded'
             WHERE codec_family = ?1 AND codec_profile_digest = ?2 AND status = 'active'",
            params!["turbo_quant", &codec_profile_digest],
        )?;
        tx.execute(
            "DELETE FROM derived_vector_artifacts
             WHERE codec_family = ?1 AND codec_profile_digest = ?2",
            params!["turbo_quant", &codec_profile_digest],
        )?;
        tx.execute(
            "INSERT INTO derived_vector_artifact_generations
                (generation_id, schema_version, codec_family, codec_profile_digest,
                 source_snapshot_digest, source_row_count, artifact_count, source_tables_json,
                 dim, encoding, created_at, build_receipt_id, artifact_manifest_digest,
                 status, degradations_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)",
            params![
                generation_manifest.generation_id,
                generation_manifest.schema_version,
                generation_manifest.codec_family,
                generation_manifest.codec_profile_digest,
                generation_manifest.source_snapshot_digest,
                i64::try_from(generation_manifest.source_row_count).map_err(|err| {
                    MemoryError::Other(format!("source row count overflow: {err}"))
                })?,
                i64::try_from(generation_manifest.artifact_count).map_err(|err| {
                    MemoryError::Other(format!("artifact count overflow: {err}"))
                })?,
                serde_json::to_string(&generation_manifest.source_tables)
                    .map_err(|err| MemoryError::Other(err.to_string()))?,
                i64::try_from(generation_manifest.dim)
                    .map_err(|err| MemoryError::Other(format!("artifact dim overflow: {err}")))?,
                generation_manifest.encoding,
                generation_manifest.created_at.to_rfc3339(),
                generation_manifest.build_receipt_id,
                generation_manifest.artifact_manifest_digest,
                generation_manifest.status,
                serde_json::to_string(&generation_manifest.degradations)
                    .map_err(|err| MemoryError::Other(err.to_string()))?,
            ],
        )?;
        for row in &pending {
            upsert_derived_vector_artifact(tx, row)?;
            artifact_count += 1;
        }
        Ok(())
    })?;

    Ok(VectorArtifactBuildReceiptV1 {
        schema_version: "vector_artifact_build_receipt_v1".to_string(),
        codec_family: "turbo_quant".to_string(),
        codec_profile_digest,
        source_row_count,
        artifact_count,
        generation_id: Some(generation_id),
        source_snapshot_digest: Some(source_snapshot_digest),
        artifact_manifest_digest: Some(artifact_manifest_digest),
        build_receipt_id: Some(build_receipt_id),
        skipped_row_count,
        elapsed_ms: started.elapsed().as_millis(),
        created_at: Utc::now(),
        degradations,
    })
}

fn receipt_count_to_u64(value: usize, field: &'static str) -> Result<u64, MemoryError> {
    u64::try_from(value).map_err(|err| MemoryError::Other(format!("{field} is too large: {err}")))
}

fn receipt_count_to_i64(value: u64, field: &'static str) -> Result<i64, MemoryError> {
    i64::try_from(value).map_err(|err| MemoryError::Other(format!("{field} is too large: {err}")))
}

fn receipt_count_to_usize(
    value: u64,
    receipt_id: &str,
    field: &'static str,
) -> Result<usize, MemoryError> {
    usize::try_from(value).map_err(|err| MemoryError::CorruptData {
        table: "search_receipts",
        row_id: receipt_id.to_string(),
        detail: format!("{field} does not fit this platform: {err}"),
    })
}

fn stored_search_receipt(
    receipt: &VectorSearchReceiptV1,
) -> Result<StoredVectorSearchReceiptV1, MemoryError> {
    Ok(StoredVectorSearchReceiptV1 {
        schema_version: SEARCH_RECEIPT_SCHEMA_VERSION.to_string(),
        receipt_id: receipt.receipt_id.clone(),
        evaluation_time: receipt.evaluation_time,
        receipt_digest: receipt.receipt_digest.clone(),
        trace_id: receipt.trace_id.clone(),
        attempt_family_id: receipt.attempt_family_id.clone(),
        attempt_id: receipt.attempt_id.clone(),
        replay_of: receipt.replay_of.clone(),
        query_embedding_digest: receipt.query_embedding_digest.clone(),
        query_text_digest: receipt.query_text_digest.clone(),
        query_input_digest: receipt.query_input_digest.clone(),
        filter_digest: receipt.filter_digest.clone(),
        redaction_state: receipt.redaction_state.clone(),
        budget_id: receipt.budget_id.clone(),
        deadline_at: receipt.deadline_at,
        search_profile: receipt.search_profile.clone(),
        candidate_backend: receipt.candidate_backend.clone(),
        codec_family: receipt.codec_family.clone(),
        codec_profile_digest: receipt.codec_profile_digest.clone(),
        artifact_profile_digest: receipt.artifact_profile_digest.clone(),
        artifact_count: receipt
            .artifact_count
            .map(|value| receipt_count_to_u64(value, "artifact_count"))
            .transpose()?,
        artifact_corruption_count: receipt
            .artifact_corruption_count
            .map(|value| receipt_count_to_u64(value, "artifact_corruption_count"))
            .transpose()?,
        artifact_missing_count: receipt
            .artifact_missing_count
            .map(|value| receipt_count_to_u64(value, "artifact_missing_count"))
            .transpose()?,
        vector_artifact_manifest_digest: receipt.vector_artifact_manifest_digest.clone(),
        artifact_generation_id: receipt.artifact_generation_id.clone(),
        approximate_scanned_count: receipt
            .approximate_scanned_count
            .map(|value| receipt_count_to_u64(value, "approximate_scanned_count"))
            .transpose()?,
        approximate_returned_count: receipt
            .approximate_returned_count
            .map(|value| receipt_count_to_u64(value, "approximate_returned_count"))
            .transpose()?,
        raw_rows_loaded_count: receipt
            .raw_rows_loaded_count
            .map(|value| receipt_count_to_u64(value, "raw_rows_loaded_count"))
            .transpose()?,
        filter_strategy: receipt.filter_strategy.clone(),
        vector_artifact_count: receipt
            .vector_artifact_count
            .map(|value| receipt_count_to_u64(value, "vector_artifact_count"))
            .transpose()?,
        vector_artifact_missing_count: receipt
            .vector_artifact_missing_count
            .map(|value| receipt_count_to_u64(value, "vector_artifact_missing_count"))
            .transpose()?,
        vector_artifact_stale_count: receipt
            .vector_artifact_stale_count
            .map(|value| receipt_count_to_u64(value, "vector_artifact_stale_count"))
            .transpose()?,
        exact_rerank_count: receipt
            .exact_rerank_count
            .map(|value| receipt_count_to_u64(value, "exact_rerank_count"))
            .transpose()?,
        approximate_candidate_count: receipt
            .approximate_candidate_count
            .map(|value| receipt_count_to_u64(value, "approximate_candidate_count"))
            .transpose()?,
        fallback_reason: receipt.fallback_reason.clone(),
        approximate: receipt.approximate,
        requested_candidates: receipt_count_to_u64(
            receipt.requested_candidates,
            "requested_candidates",
        )?,
        returned_candidates: receipt_count_to_u64(
            receipt.returned_candidates,
            "returned_candidates",
        )?,
        post_filter_candidates: receipt_count_to_u64(
            receipt.post_filter_candidates,
            "post_filter_candidates",
        )?,
        sparse_enabled: receipt.sparse_enabled,
        sparse_weight: receipt.sparse_weight,
        sparse_query_nonzero_count: receipt
            .sparse_query_nonzero_count
            .map(|value| receipt_count_to_u64(value, "sparse_query_nonzero_count"))
            .transpose()?,
        sparse_candidate_count: receipt
            .sparse_candidate_count
            .map(|value| receipt_count_to_u64(value, "sparse_candidate_count"))
            .transpose()?,
        sparse_representations: receipt.sparse_representations.clone(),
        sparse_result_ranks: receipt.sparse_result_ranks.clone(),
        fallback: receipt.fallback.clone(),
        exact_rerank: receipt.exact_rerank,
        result_ids: receipt.result_ids.clone(),
        degradations: receipt.degradations.clone(),
    })
}

fn search_receipt_from_stored(
    stored: StoredVectorSearchReceiptV1,
) -> Result<VectorSearchReceiptV1, MemoryError> {
    if stored.schema_version != SEARCH_RECEIPT_SCHEMA_VERSION {
        return Err(MemoryError::CorruptData {
            table: "search_receipts",
            row_id: stored.receipt_id,
            detail: format!(
                "unsupported receipt schema version '{}'",
                stored.schema_version
            ),
        });
    }

    Ok(VectorSearchReceiptV1 {
        schema_version: stored.schema_version.clone(),
        receipt_digest: stored.receipt_digest,
        receipt_id: stored.receipt_id.clone(),
        evaluation_time: stored.evaluation_time,
        trace_id: stored.trace_id,
        attempt_family_id: stored.attempt_family_id,
        attempt_id: stored.attempt_id,
        replay_of: stored.replay_of,
        query_embedding_digest: stored.query_embedding_digest,
        query_text_digest: stored.query_text_digest,
        query_input_digest: stored.query_input_digest,
        filter_digest: stored.filter_digest,
        redaction_state: stored.redaction_state,
        budget_id: stored.budget_id,
        deadline_at: stored.deadline_at,
        search_profile: stored.search_profile,
        candidate_backend: stored.candidate_backend,
        codec_family: stored.codec_family,
        codec_profile_digest: stored.codec_profile_digest,
        artifact_profile_digest: stored.artifact_profile_digest,
        artifact_count: stored
            .artifact_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "artifact_count"))
            .transpose()?,
        artifact_corruption_count: stored
            .artifact_corruption_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "artifact_corruption_count")
            })
            .transpose()?,
        artifact_missing_count: stored
            .artifact_missing_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "artifact_missing_count")
            })
            .transpose()?,
        vector_artifact_manifest_digest: stored.vector_artifact_manifest_digest,
        artifact_generation_id: stored.artifact_generation_id,
        approximate_scanned_count: stored
            .approximate_scanned_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "approximate_scanned_count")
            })
            .transpose()?,
        approximate_returned_count: stored
            .approximate_returned_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "approximate_returned_count")
            })
            .transpose()?,
        raw_rows_loaded_count: stored
            .raw_rows_loaded_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "raw_rows_loaded_count"))
            .transpose()?,
        filter_strategy: stored.filter_strategy,
        vector_artifact_count: stored
            .vector_artifact_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "vector_artifact_count"))
            .transpose()?,
        vector_artifact_missing_count: stored
            .vector_artifact_missing_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "vector_artifact_missing_count")
            })
            .transpose()?,
        vector_artifact_stale_count: stored
            .vector_artifact_stale_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "vector_artifact_stale_count")
            })
            .transpose()?,
        exact_rerank_count: stored
            .exact_rerank_count
            .map(|value| receipt_count_to_usize(value, &stored.receipt_id, "exact_rerank_count"))
            .transpose()?,
        approximate_candidate_count: stored
            .approximate_candidate_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "approximate_candidate_count")
            })
            .transpose()?,
        fallback_reason: stored.fallback_reason,
        derived_candidate: None,
        approximate: stored.approximate,
        requested_candidates: receipt_count_to_usize(
            stored.requested_candidates,
            &stored.receipt_id,
            "requested_candidates",
        )?,
        returned_candidates: receipt_count_to_usize(
            stored.returned_candidates,
            &stored.receipt_id,
            "returned_candidates",
        )?,
        post_filter_candidates: receipt_count_to_usize(
            stored.post_filter_candidates,
            &stored.receipt_id,
            "post_filter_candidates",
        )?,
        sparse_enabled: stored.sparse_enabled,
        sparse_weight: stored.sparse_weight,
        sparse_query_nonzero_count: stored
            .sparse_query_nonzero_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "sparse_query_nonzero_count")
            })
            .transpose()?,
        sparse_candidate_count: stored
            .sparse_candidate_count
            .map(|value| {
                receipt_count_to_usize(value, &stored.receipt_id, "sparse_candidate_count")
            })
            .transpose()?,
        sparse_representations: stored.sparse_representations,
        sparse_result_ranks: stored.sparse_result_ranks,
        fallback: stored.fallback,
        exact_rerank: stored.exact_rerank,
        result_ids: stored.result_ids,
        degradations: stored.degradations,
    })
}

/// Persist a search receipt as replay metadata.
///
/// SQLite rows remain authoritative for memory. This table stores only the
/// execution receipt and digest so the search can be addressed later.
pub fn store_search_receipt(
    conn: &Connection,
    receipt: &VectorSearchReceiptV1,
) -> Result<(), MemoryError> {
    let stored = stored_search_receipt(receipt)?;
    let receipt_json = serde_json::to_string(&stored)
        .map_err(|err| MemoryError::Other(format!("failed to serialize search receipt: {err}")))?;
    let receipt_digest = b3_digest(receipt_json.as_bytes());

    let existing_digest: Option<String> = conn
        .query_row(
            "SELECT receipt_digest FROM search_receipts WHERE receipt_id = ?1",
            params![&stored.receipt_id],
            |row| row.get(0),
        )
        .optional()?;
    if let Some(existing_digest) = existing_digest {
        if existing_digest == receipt_digest {
            return Ok(());
        }
        return Err(MemoryError::SearchReceiptConflict {
            receipt_id: stored.receipt_id,
        });
    }

    let result_ids_json = serde_json::to_string(&stored.result_ids).map_err(|err| {
        MemoryError::Other(format!(
            "failed to serialize search receipt result IDs: {err}"
        ))
    })?;
    conn.execute(
        "INSERT INTO search_receipts (
            receipt_id,
            schema_version,
            evaluation_time,
            search_profile,
            candidate_backend,
            approximate,
            exact_rerank,
            fallback,
            requested_candidates,
            returned_candidates,
            post_filter_candidates,
            result_ids_json,
            receipt_json,
            receipt_digest
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            &stored.receipt_id,
            SEARCH_RECEIPT_SCHEMA_VERSION,
            stored.evaluation_time.to_rfc3339(),
            &stored.search_profile,
            &stored.candidate_backend,
            if stored.approximate { 1_i64 } else { 0_i64 },
            if stored.exact_rerank { 1_i64 } else { 0_i64 },
            &stored.fallback,
            receipt_count_to_i64(stored.requested_candidates, "requested_candidates")?,
            receipt_count_to_i64(stored.returned_candidates, "returned_candidates")?,
            receipt_count_to_i64(stored.post_filter_candidates, "post_filter_candidates")?,
            &result_ids_json,
            &receipt_json,
            &receipt_digest,
        ],
    )?;
    Ok(())
}

/// Load a durable search receipt by receipt/request ID.
pub fn get_search_receipt(
    conn: &Connection,
    receipt_id: &str,
) -> Result<Option<VectorSearchReceiptV1>, MemoryError> {
    let row: Option<(String, String, String)> = conn
        .query_row(
            "SELECT schema_version, receipt_json, receipt_digest
             FROM search_receipts
             WHERE receipt_id = ?1",
            params![receipt_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;

    let Some((schema_version, receipt_json, receipt_digest)) = row else {
        return Ok(None);
    };
    if schema_version != SEARCH_RECEIPT_SCHEMA_VERSION {
        return Err(MemoryError::CorruptData {
            table: "search_receipts",
            row_id: receipt_id.to_string(),
            detail: format!("unsupported receipt schema version '{schema_version}'"),
        });
    }

    let stored: StoredVectorSearchReceiptV1 =
        serde_json::from_str(&receipt_json).map_err(|err| MemoryError::CorruptData {
            table: "search_receipts",
            row_id: receipt_id.to_string(),
            detail: format!("invalid receipt JSON: {err}"),
        })?;
    let mut receipt = search_receipt_from_stored(stored)?;
    receipt.receipt_digest = Some(receipt_digest);
    Ok(Some(receipt))
}

/// Privacy-sensitive inputs retained by explicit replay opt-in.
pub(crate) struct ReplayInputs {
    pub query_text: String,
    pub namespaces: Option<Vec<String>>,
    pub source_types: Option<Vec<SearchSourceType>>,
}

/// Persist the inputs needed to replay a durable search receipt.
pub(crate) fn store_replay_inputs(
    conn: &Connection,
    receipt_id: &str,
    query_text: &str,
    namespaces: Option<&[&str]>,
    source_types: Option<&[SearchSourceType]>,
) -> Result<(), MemoryError> {
    let namespaces_json = namespaces
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| {
            MemoryError::Other(format!("failed to serialize replay namespaces: {error}"))
        })?;
    let source_types_json = source_types
        .map(serde_json::to_string)
        .transpose()
        .map_err(|error| {
            MemoryError::Other(format!("failed to serialize replay source types: {error}"))
        })?;

    let existing: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT query_text, namespaces_json, source_types_json
             FROM replay_inputs WHERE receipt_id = ?1",
            params![receipt_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    if let Some(existing) = existing {
        if existing.0 == query_text
            && existing.1.as_ref() == namespaces_json.as_ref()
            && existing.2.as_ref() == source_types_json.as_ref()
        {
            return Ok(());
        }
        return Err(MemoryError::SearchReceiptConflict {
            receipt_id: receipt_id.to_string(),
        });
    }

    conn.execute(
        "INSERT INTO replay_inputs
            (receipt_id, query_text, namespaces_json, source_types_json)
         VALUES (?1, ?2, ?3, ?4)",
        params![receipt_id, query_text, namespaces_json, source_types_json],
    )?;
    Ok(())
}

/// Load opt-in inputs for complete replay, if present.
pub(crate) fn get_replay_inputs(
    conn: &Connection,
    receipt_id: &str,
) -> Result<Option<ReplayInputs>, MemoryError> {
    let row: Option<(String, Option<String>, Option<String>)> = conn
        .query_row(
            "SELECT query_text, namespaces_json, source_types_json
             FROM replay_inputs WHERE receipt_id = ?1",
            params![receipt_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((query_text, namespaces_json, source_types_json)) = row else {
        return Ok(None);
    };
    let namespaces = namespaces_json
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(|error| MemoryError::CorruptData {
            table: "replay_inputs",
            row_id: receipt_id.to_string(),
            detail: format!("invalid namespaces JSON: {error}"),
        })?;
    let source_types = source_types_json
        .map(|json| serde_json::from_str(&json))
        .transpose()
        .map_err(|error| MemoryError::CorruptData {
            table: "replay_inputs",
            row_id: receipt_id.to_string(),
            detail: format!("invalid source types JSON: {error}"),
        })?;
    Ok(Some(ReplayInputs {
        query_text,
        namespaces,
        source_types,
    }))
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    column_sql: &str,
) -> Result<(), rusqlite::Error> {
    let pragma = format!("PRAGMA table_info({table})");
    let mut stmt = conn.prepare(&pragma)?;
    let exists = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .any(|name| name == column);

    if !exists {
        conn.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {column} {column_sql}"),
            [],
        )?;
    }

    Ok(())
}

/// Check and update the embedding metadata singleton row.
pub fn check_embedding_metadata(
    conn: &Connection,
    config: &EmbeddingConfig,
) -> Result<(), MemoryError> {
    // INTENTIONAL: row absent on first run before metadata is inserted
    let existing: Option<(String, usize)> = conn
        .query_row(
            "SELECT model_name, dimensions FROM embedding_metadata WHERE id = 1",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .ok();

    match existing {
        Some((model, dims)) => {
            if model != config.model || dims != config.dimensions {
                tracing::warn!(
                    stored_model = %model,
                    stored_dims = dims,
                    configured_model = %config.model,
                    configured_dims = config.dimensions,
                    "Embedding model changed. Existing embeddings are stale."
                );
                conn.execute(
                    "UPDATE embedding_metadata
                     SET model_name = ?1,
                         dimensions = ?2,
                         embeddings_dirty = 1,
                         updated_at = datetime('now')
                     WHERE id = 1",
                    params![config.model, config.dimensions],
                )?;
            }
        }
        None => {
            conn.execute(
                "INSERT INTO embedding_metadata (id, model_name, dimensions) VALUES (1, ?1, ?2)",
                params![config.model, config.dimensions],
            )?;
        }
    }

    Ok(())
}

/// Encode an f32 slice as bytes for SQLite BLOB storage.
pub fn embedding_to_bytes(embedding: &[f32]) -> Vec<u8> {
    encode_f32_le(embedding)
}

/// One durable sparse-vector row used by the sparse retrieval lane.
#[derive(Debug, Clone)]
pub(crate) struct SparseVectorRow {
    pub item_key: String,
    pub weights: SparseWeights,
    pub representation: String,
}

/// Insert or replace the sparse representation for a canonical search item.
pub(crate) fn store_sparse_vector(
    conn: &Connection,
    item_key: &str,
    weights: &SparseWeights,
    representation: &str,
) -> Result<(), MemoryError> {
    if item_key.split_once(':').is_none() || representation.trim().is_empty() {
        return Err(MemoryError::InvalidConfig {
            field: "sparse_vector",
            reason: "item_key must be canonical and representation must not be empty".to_string(),
        });
    }
    if weights
        .entries
        .iter()
        .any(|(_, weight)| !weight.is_finite())
    {
        return Err(MemoryError::InvalidConfig {
            field: "sparse_vector.entries",
            reason: "sparse weights must be finite".to_string(),
        });
    }
    let entries_json = serde_json::to_string(&weights.entries)
        .map_err(|error| MemoryError::Other(format!("serialize sparse weights: {error}")))?;
    conn.execute(
        "INSERT INTO sparse_vectors (item_key, entries_json, representation, updated_at)
         VALUES (?1, ?2, ?3, datetime('now'))
         ON CONFLICT(item_key) DO UPDATE SET
             entries_json = excluded.entries_json,
             representation = excluded.representation,
             updated_at = excluded.updated_at",
        params![item_key, entries_json, representation],
    )?;
    Ok(())
}

/// Load a bounded set of durable sparse rows selected by sparse dot product.
pub(crate) fn search_sparse_vectors(
    conn: &Connection,
    query: &SparseWeights,
    candidate_limit: usize,
    min_score: f64,
) -> Result<Vec<(SparseVectorRow, f64)>, MemoryError> {
    if query.is_empty() || candidate_limit == 0 {
        return Ok(Vec::new());
    }

    let mut sql = String::from("WITH query_sparse(dimension, weight) AS (VALUES ");
    for index in 0..query.entries.len() {
        if index > 0 {
            sql.push(',');
        }
        let dimension_param = index * 2 + 1;
        let weight_param = dimension_param + 1;
        sql.push_str(&format!("(?{dimension_param}, ?{weight_param})"));
    }
    let min_score_param = query.entries.len() * 2 + 1;
    let limit_param = min_score_param + 1;
    sql.push_str(&format!(
        ") SELECT sv.item_key, sv.entries_json, sv.representation,
                  SUM(CAST(json_extract(entry.value, '$[1]') AS REAL) * query_sparse.weight) AS sparse_score
           FROM sparse_vectors sv
           JOIN json_each(sv.entries_json) AS entry
           JOIN query_sparse
             ON CAST(json_extract(entry.value, '$[0]') AS INTEGER) = query_sparse.dimension
           GROUP BY sv.item_key
           HAVING sparse_score >= ?{min_score_param}
           ORDER BY sparse_score DESC, sv.item_key ASC
           LIMIT ?{limit_param}"
    ));

    let mut values: Vec<rusqlite::types::Value> = Vec::with_capacity(query.entries.len() * 2 + 2);
    for (dimension, weight) in &query.entries {
        values.push((*dimension as i64).into());
        values.push(f64::from(*weight).into());
    }
    values.push(min_score.into());
    values.push((candidate_limit as i64).into());

    let mut statement = conn.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(values), |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, f64>(3)?,
        ))
    })?;
    let mut results = Vec::new();
    for row in rows {
        let (item_key, entries_json, representation, score) = row?;
        let entries: Vec<(usize, f32)> =
            serde_json::from_str(&entries_json).map_err(|error| MemoryError::CorruptData {
                table: "sparse_vectors",
                row_id: item_key.clone(),
                detail: format!("invalid sparse entries JSON: {error}"),
            })?;
        results.push((
            SparseVectorRow {
                item_key,
                weights: SparseWeights::from_entries(entries),
                representation,
            },
            score,
        ));
    }
    Ok(results)
}

/// Remove a sparse vector explicitly (used for supersession cleanup).
pub(crate) fn delete_sparse_vector(conn: &Connection, item_key: &str) -> Result<(), MemoryError> {
    conn.execute(
        "DELETE FROM sparse_vectors WHERE item_key = ?1",
        params![item_key],
    )?;
    Ok(())
}

/// Encode f32 values as a stable little-endian persisted representation.
pub fn encode_f32_le(values: &[f32]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(values.len() * 4);
    for value in values {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes
}

/// Validate an embedding vector before it is stored or indexed.
pub(crate) fn validate_embedding(values: &[f32], expected_dim: usize) -> Result<(), MemoryError> {
    if values.len() != expected_dim {
        return Err(MemoryError::EmbeddingDimensionMismatch {
            expected: expected_dim,
            actual: values.len(),
        });
    }
    if let Some((index, _)) = values
        .iter()
        .enumerate()
        .find(|(_, value)| !value.is_finite())
    {
        return Err(MemoryError::NonFiniteEmbeddingValue { index });
    }
    Ok(())
}

/// Validate a returned embedding batch against the requested input count.
pub(crate) fn validate_embedding_batch(
    values: &[Vec<f32>],
    requested: usize,
    expected_dim: usize,
) -> Result<(), MemoryError> {
    if values.len() != requested {
        return Err(MemoryError::EmbeddingBatchCountMismatch {
            requested,
            returned: values.len(),
        });
    }
    for embedding in values {
        validate_embedding(embedding, expected_dim)?;
    }
    Ok(())
}

/// Validate the exact byte length of a persisted f32 vector blob.
pub(crate) fn validate_vector_blob_len(
    bytes: &[u8],
    expected_dim: usize,
) -> Result<(), MemoryError> {
    let expected_bytes = expected_dim
        .checked_mul(4)
        .ok_or_else(|| MemoryError::InvalidConfig {
            field: "embedding.dimensions",
            reason: "dimension byte length overflow".to_string(),
        })?;
    if bytes.len() != expected_bytes {
        return Err(MemoryError::VectorBlobLengthMismatch {
            expected_bytes,
            actual_bytes: bytes.len(),
        });
    }
    Ok(())
}

/// Decode a stable little-endian f32 persisted representation.
#[allow(clippy::manual_is_multiple_of)]
pub fn decode_f32_le(bytes: &[u8], expected_dim: usize) -> Result<Vec<f32>, MemoryError> {
    validate_vector_blob_len(bytes, expected_dim)?;
    decode_f32_le_unchecked_dim(bytes)
}

/// Decode a SQLite embedding BLOB back to f32 values.
#[allow(clippy::manual_is_multiple_of)]
pub fn bytes_to_embedding(bytes: &[u8]) -> Result<Vec<f32>, MemoryError> {
    if bytes.len() % 4 != 0 {
        return Err(MemoryError::InvalidEmbedding {
            expected_bytes: bytes.len() - (bytes.len() % 4),
            actual_bytes: bytes.len(),
        });
    }

    decode_f32_le_unchecked_dim(bytes)
}

fn decode_f32_le_unchecked_dim(bytes: &[u8]) -> Result<Vec<f32>, MemoryError> {
    let mut embedding = Vec::with_capacity(bytes.len() / 4);
    for (index, chunk) in bytes.chunks_exact(4).enumerate() {
        let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        if !value.is_finite() {
            return Err(MemoryError::NonFiniteEmbeddingValue { index });
        }
        embedding.push(value);
    }
    Ok(embedding)
}

pub fn is_embeddings_dirty(conn: &Connection) -> Result<bool, MemoryError> {
    let dirty: i32 = conn
        .query_row(
            "SELECT COALESCE(embeddings_dirty, 0) FROM embedding_metadata WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(dirty != 0)
}

pub fn clear_embeddings_dirty(conn: &Connection) -> Result<(), MemoryError> {
    conn.execute(
        "UPDATE embedding_metadata SET embeddings_dirty = 0 WHERE id = 1",
        [],
    )?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) fn queue_pending_index_op(
    tx: &rusqlite::Transaction<'_>,
    item_key: &str,
    entity_type: &str,
    op_kind: IndexOpKind,
) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO pending_index_ops (item_key, entity_type, op_kind, attempt_count, last_error, updated_at)
         VALUES (?1, ?2, ?3, 0, NULL, datetime('now'))
         ON CONFLICT(item_key) DO UPDATE SET
             entity_type = excluded.entity_type,
             op_kind = excluded.op_kind,
             attempt_count = 0,
             last_error = NULL,
             updated_at = datetime('now')",
        params![item_key, entity_type, op_kind.as_str()],
    )?;
    mark_sidecar_dirty(tx)?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) use IndexOpKind as PendingIndexOpKind;

#[cfg(feature = "hnsw")]
pub(crate) fn enqueue_pending_index_op(
    tx: &rusqlite::Transaction<'_>,
    item_key: &str,
    entity_type: &str,
    op_kind: PendingIndexOpKind,
) -> Result<(), MemoryError> {
    queue_pending_index_op(tx, item_key, entity_type, op_kind)
}

pub(crate) fn list_pending_index_ops(
    conn: &Connection,
) -> Result<Vec<PendingIndexOp>, MemoryError> {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='pending_index_ops'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !table_exists {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT item_key, entity_type, op_kind, attempt_count, last_error
         FROM pending_index_ops
         ORDER BY updated_at ASC, item_key ASC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            let item_key: String = row.get(0)?;
            let op_kind: String = row.get(2)?;
            Ok(PendingIndexOp {
                item_key: item_key.clone(),
                entity_type: row.get(1)?,
                op_kind: IndexOpKind::parse(&op_kind, &item_key).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        2,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })?,
                attempt_count: row.get::<_, i64>(3)? as u32,
                last_error: row.get(4)?,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(feature = "hnsw")]
pub(crate) fn pending_index_op_count(conn: &Connection) -> Result<usize, MemoryError> {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='pending_index_ops'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !table_exists {
        return Ok(0);
    }

    let count: i64 = conn.query_row("SELECT COUNT(*) FROM pending_index_ops", [], |row| {
        row.get(0)
    })?;
    Ok(count as usize)
}

#[cfg(feature = "hnsw")]
pub(crate) fn mark_pending_index_ops_failed(
    conn: &Connection,
    item_keys: &[String],
    error: &str,
) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        for item_key in item_keys {
            tx.execute(
                "UPDATE pending_index_ops
                 SET attempt_count = attempt_count + 1,
                     last_error = ?1,
                     updated_at = datetime('now')
                 WHERE item_key = ?2",
                params![error, item_key],
            )?;
        }
        Ok(())
    })
}

#[cfg(feature = "hnsw")]
pub(crate) fn clear_pending_index_ops(
    conn: &Connection,
    item_keys: &[String],
) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        for item_key in item_keys {
            tx.execute(
                "DELETE FROM pending_index_ops WHERE item_key = ?1",
                params![item_key],
            )?;
        }
        Ok(())
    })
}

#[cfg(feature = "hnsw")]
pub(crate) fn clear_all_pending_index_ops(conn: &Connection) -> Result<(), MemoryError> {
    conn.execute("DELETE FROM pending_index_ops", [])?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) fn load_embedding_for_index_key(
    conn: &Connection,
    item_key: &str,
) -> Result<Option<Vec<f32>>, MemoryError> {
    let Some((domain, raw_id)) = item_key.split_once(':') else {
        return Err(MemoryError::InvalidKey(item_key.to_string()));
    };

    let blob_result: Result<Option<Vec<u8>>, rusqlite::Error> = match domain {
        "fact" => conn.query_row(
            "SELECT embedding FROM facts WHERE id = ?1",
            params![raw_id],
            |row| row.get(0),
        ),
        "chunk" => conn.query_row(
            "SELECT embedding FROM chunks WHERE id = ?1",
            params![raw_id],
            |row| row.get(0),
        ),
        "msg" => {
            let message_id = raw_id
                .parse::<i64>()
                .map_err(|e| MemoryError::InvalidKey(format!("{}: {e}", item_key)))?;
            conn.query_row(
                "SELECT embedding FROM messages WHERE id = ?1",
                params![message_id],
                |row| row.get(0),
            )
        }
        "episode" => conn.query_row(
            "SELECT embedding FROM episodes WHERE episode_id = ?1",
            params![raw_id],
            |row| row.get(0),
        ),
        _ => return Err(MemoryError::InvalidKey(item_key.to_string())),
    };

    let blob = match blob_result {
        Ok(blob) => blob,
        Err(rusqlite::Error::QueryReturnedNoRows) => None,
        Err(err) => return Err(err.into()),
    };

    blob.map(|bytes| bytes_to_embedding(&bytes)).transpose()
}

#[cfg(feature = "hnsw")]
fn mark_sidecar_dirty(tx: &rusqlite::Transaction<'_>) -> Result<(), MemoryError> {
    tx.execute(
        "INSERT INTO hnsw_metadata (key, value) VALUES ('sidecar_dirty', '1')
         ON CONFLICT(key) DO UPDATE SET value = '1'",
        [],
    )?;
    Ok(())
}

#[cfg(feature = "hnsw")]
pub(crate) fn is_sidecar_dirty(conn: &Connection) -> Result<bool, MemoryError> {
    // INTENTIONAL: row absent when HNSW metadata has not been written yet
    let dirty: Option<String> = conn
        .query_row(
            "SELECT value FROM hnsw_metadata WHERE key = 'sidecar_dirty'",
            [],
            |row| row.get(0),
        )
        .ok();
    Ok(matches!(dirty.as_deref(), Some("1")))
}

#[cfg(feature = "hnsw")]
pub(crate) fn set_sidecar_dirty(conn: &Connection, dirty: bool) -> Result<(), MemoryError> {
    conn.execute(
        "INSERT INTO hnsw_metadata (key, value) VALUES ('sidecar_dirty', ?1)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![if dirty { "1" } else { "0" }],
    )?;
    Ok(())
}

pub(crate) fn parse_optional_json(
    table: &'static str,
    row_id: &str,
    field: &'static str,
    raw: Option<&str>,
) -> Result<Option<serde_json::Value>, MemoryError> {
    match raw {
        Some(raw) => serde_json::from_str(raw)
            .map(Some)
            .map_err(|e| MemoryError::CorruptData {
                table,
                row_id: row_id.to_string(),
                detail: format!("invalid {field}: {e}"),
            }),
        None => Ok(None),
    }
}

pub(crate) fn parse_string_list_json(
    table: &'static str,
    row_id: &str,
    field: &'static str,
    raw: &str,
) -> Result<Vec<String>, MemoryError> {
    serde_json::from_str(raw).map_err(|e| MemoryError::CorruptData {
        table,
        row_id: row_id.to_string(),
        detail: format!("invalid {field}: {e}"),
    })
}

pub(crate) fn parse_role(
    table: &'static str,
    row_id: &str,
    raw: &str,
) -> Result<Role, MemoryError> {
    Role::from_str_value(raw).ok_or_else(|| MemoryError::CorruptData {
        table,
        row_id: row_id.to_string(),
        detail: format!("invalid role '{raw}'"),
    })
}

pub(crate) fn parse_episode_outcome(
    row_id: &str,
    raw: &str,
) -> Result<EpisodeOutcome, MemoryError> {
    EpisodeOutcome::from_str_value(raw).ok_or_else(|| MemoryError::CorruptData {
        table: "episodes",
        row_id: row_id.to_string(),
        detail: format!("invalid outcome '{raw}'"),
    })
}

pub(crate) fn parse_verification_status(
    row_id: &str,
    raw: &str,
) -> Result<VerificationStatus, MemoryError> {
    serde_json::from_str(raw).map_err(|e| MemoryError::CorruptData {
        table: "episodes",
        row_id: row_id.to_string(),
        detail: format!("invalid verification_status: {e}"),
    })
}

/// Run integrity verification on the database.
pub fn verify_integrity_sync(
    conn: &Connection,
    mode: VerifyMode,
) -> Result<IntegrityReport, MemoryError> {
    let mut issues = Vec::new();

    let schema_version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to read schema version: {e}"));
            0
        });
    if schema_version > MAX_SCHEMA_VERSION {
        issues.push(format!(
            "schema version {} is ahead of supported {}",
            schema_version, MAX_SCHEMA_VERSION
        ));
    }

    let fact_count: usize = conn
        .query_row("SELECT COUNT(*) FROM facts", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count facts: {e}"));
            0
        });
    let chunk_count: usize = conn
        .query_row("SELECT COUNT(*) FROM chunks", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count chunks: {e}"));
            0
        });
    let message_count: usize = conn
        .query_row("SELECT COUNT(*) FROM messages", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count messages: {e}"));
            0
        });
    let episode_count: usize = conn
        .query_row("SELECT COUNT(*) FROM episodes", [], |row| row.get(0))
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count episodes: {e}"));
            0
        });

    let facts_missing_embeddings: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM facts WHERE embedding IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count facts missing embeddings: {e}"));
            0
        });
    let chunks_missing_embeddings: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM chunks WHERE embedding IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count chunks missing embeddings: {e}"));
            0
        });
    let episodes_missing_embeddings: usize = conn
        .query_row(
            "SELECT COUNT(*) FROM episodes WHERE embedding IS NULL",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|e| {
            issues.push(format!("failed to count episodes missing embeddings: {e}"));
            0
        });

    if facts_missing_embeddings > 0 {
        issues.push(format!(
            "{} facts missing embeddings",
            facts_missing_embeddings
        ));
    }
    if chunks_missing_embeddings > 0 {
        issues.push(format!(
            "{} chunks missing embeddings",
            chunks_missing_embeddings
        ));
    }
    if episodes_missing_embeddings > 0 {
        issues.push(format!(
            "{} episodes missing embeddings",
            episodes_missing_embeddings
        ));
    }

    let pending_ops = list_pending_index_ops(conn).unwrap_or_default();
    if !pending_ops.is_empty() {
        issues.push(format!(
            "{} pending HNSW sidecar ops queued in SQLite",
            pending_ops.len()
        ));
        for op in pending_ops.iter().take(5) {
            let op_kind = op.op_kind.as_str();
            let detail = match &op.last_error {
                Some(last_error) => format!(
                    "{} {} {} (attempts: {}, last_error: {})",
                    op.entity_type,
                    op.op_kind.as_str(),
                    op.item_key,
                    op.attempt_count,
                    last_error
                ),
                None => format!(
                    "{} {} {} (attempts: {})",
                    op.entity_type, op_kind, op.item_key, op.attempt_count
                ),
            };
            issues.push(format!("pending sidecar op: {detail}"));
        }
    }

    if mode == VerifyMode::Full {
        let dims: usize = conn
            .query_row(
                "SELECT dimensions FROM embedding_metadata WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .unwrap_or_else(|e| {
                issues.push(format!("failed to read embedding dimensions: {e}"));
                0
            });

        verify_fts_drift(conn, "facts", "facts_rowid_map", fact_count, &mut issues);
        verify_fts_drift(conn, "chunks", "chunks_rowid_map", chunk_count, &mut issues);
        verify_fts_drift(
            conn,
            "messages",
            "messages_rowid_map",
            message_count,
            &mut issues,
        );
        verify_fts_drift(
            conn,
            "episodes",
            "episodes_rowid_map",
            episode_count,
            &mut issues,
        );

        verify_blob_table(conn, "facts", "id", "embedding", dims, &mut issues)?;
        verify_blob_table(conn, "chunks", "id", "embedding", dims, &mut issues)?;
        verify_blob_table(conn, "messages", "id", "embedding", dims, &mut issues)?;
        verify_blob_table(
            conn,
            "episodes",
            "episode_id",
            "embedding",
            dims,
            &mut issues,
        )?;

        verify_quantized_table(conn, "facts", "id", dims, &mut issues)?;
        verify_quantized_table(conn, "chunks", "id", dims, &mut issues)?;
        verify_quantized_table(conn, "messages", "id", dims, &mut issues)?;
        verify_quantized_table(conn, "episodes", "episode_id", dims, &mut issues)?;

        verify_session_rows(conn, &mut issues)?;
        verify_message_rows(conn, &mut issues)?;
        verify_fact_rows(conn, &mut issues)?;
        verify_document_rows(conn, &mut issues)?;
        verify_episode_rows(conn, &mut issues)?;

        let integrity_check: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .unwrap_or_else(|_| "error".to_string());
        if integrity_check != "ok" {
            issues.push(format!("SQLite integrity_check: {}", integrity_check));
        }
    }

    Ok(IntegrityReport {
        ok: issues.is_empty(),
        schema_version,
        fact_count,
        chunk_count,
        message_count,
        facts_missing_embeddings,
        chunks_missing_embeddings,
        issues,
    })
}

/// Reconcile FTS indexes by rebuilding them from source data.
pub fn reconcile_fts(conn: &Connection) -> Result<(), MemoryError> {
    with_transaction(conn, |tx| {
        tx.execute_batch("DROP TABLE IF EXISTS facts_fts")?;
        tx.execute_batch("DELETE FROM facts_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE facts_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch("INSERT INTO facts_rowid_map (fact_id) SELECT id FROM facts")?;
        tx.execute_batch(
            "INSERT INTO facts_fts (rowid, content)
             SELECT rm.rowid, f.content
             FROM facts_rowid_map rm
             JOIN facts f ON f.id = rm.fact_id",
        )?;

        tx.execute_batch("DROP TABLE IF EXISTS chunks_fts")?;
        tx.execute_batch("DELETE FROM chunks_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE chunks_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch("INSERT INTO chunks_rowid_map (chunk_id) SELECT id FROM chunks")?;
        tx.execute_batch(
            "INSERT INTO chunks_fts (rowid, content)
             SELECT rm.rowid, c.content
             FROM chunks_rowid_map rm
             JOIN chunks c ON c.id = rm.chunk_id",
        )?;

        tx.execute_batch("DROP TABLE IF EXISTS messages_fts")?;
        tx.execute_batch("DELETE FROM messages_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE messages_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch("INSERT INTO messages_rowid_map (message_id) SELECT id FROM messages")?;
        tx.execute_batch(
            "INSERT INTO messages_fts (rowid, content)
             SELECT rm.rowid, m.content
             FROM messages_rowid_map rm
             JOIN messages m ON m.id = rm.message_id",
        )?;

        tx.execute_batch("DROP TABLE IF EXISTS episodes_fts")?;
        tx.execute_batch("DELETE FROM episodes_rowid_map")?;
        tx.execute_batch(
            "CREATE VIRTUAL TABLE episodes_fts USING fts5(
                content,
                content='',
                content_rowid='rowid',
                tokenize='porter unicode61'
            )",
        )?;
        tx.execute_batch(
            "INSERT INTO episodes_rowid_map (episode_id, document_id) SELECT episode_id, document_id FROM episodes",
        )?;
        tx.execute_batch(
            "INSERT INTO episodes_fts (rowid, content)
             SELECT rm.rowid, e.search_text
             FROM episodes_rowid_map rm
             JOIN episodes e ON e.episode_id = rm.episode_id",
        )?;

        Ok(())
    })?;

    tracing::info!("FTS indexes reconciled");
    Ok(())
}

fn verify_fts_drift(
    conn: &Connection,
    label: &str,
    map_table: &str,
    source_count: usize,
    issues: &mut Vec<String>,
) {
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name = ?1",
            params![map_table],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if !table_exists {
        if source_count > 0 {
            issues.push(format!("{} rows exist but {} is missing", label, map_table));
        }
        return;
    }

    let sql = format!("SELECT COUNT(*) FROM {}", map_table);
    let indexed_count: usize = conn.query_row(&sql, [], |row| row.get(0)).unwrap_or(0);
    if indexed_count != source_count {
        issues.push(format!(
            "FTS {} index drift: {} rows in map vs {} source rows",
            label, indexed_count, source_count
        ));
    }
}

fn verify_blob_table(
    conn: &Connection,
    table: &'static str,
    id_column: &'static str,
    blob_column: &'static str,
    expected_dims: usize,
    issues: &mut Vec<String>,
) -> Result<(), MemoryError> {
    if expected_dims == 0 {
        return Ok(());
    }

    let sql = format!(
        "SELECT CAST({id_column} AS TEXT), {blob_column} FROM {table} WHERE {blob_column} IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
    })?;

    for row in rows {
        let (row_id, blob) = row?;
        match bytes_to_embedding(&blob) {
            Ok(embedding) if embedding.len() != expected_dims => issues.push(format!(
                "{}({}) has embedding dimension {} but expected {}",
                table,
                row_id,
                embedding.len(),
                expected_dims
            )),
            Ok(_) => {}
            Err(err) => issues.push(format!(
                "{}({}) invalid embedding blob: {}",
                table, row_id, err
            )),
        }
    }

    Ok(())
}

fn verify_quantized_table(
    conn: &Connection,
    table: &'static str,
    id_column: &'static str,
    expected_dims: usize,
    issues: &mut Vec<String>,
) -> Result<(), MemoryError> {
    if expected_dims == 0 {
        return Ok(());
    }

    let sql = format!(
        "SELECT CAST({id_column} AS TEXT), embedding_q8 FROM {table} WHERE embedding IS NOT NULL"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<Vec<u8>>>(1)?))
    })?;

    for row in rows {
        let (row_id, blob) = row?;
        match blob {
            Some(blob) => {
                if let Err(err) = unpack_quantized(&blob, expected_dims) {
                    issues.push(format!(
                        "{}({}) invalid quantized embedding: {}",
                        table, row_id, err
                    ));
                }
            }
            None => issues.push(format!("{}({}) missing quantized embedding", table, row_id)),
        }
    }

    Ok(())
}

fn verify_session_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, metadata FROM sessions WHERE metadata IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, metadata) = row?;
        if let Err(err) = parse_optional_json("sessions", &id, "metadata", Some(&metadata)) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_message_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, role, metadata FROM messages")?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, Option<String>>(2)?,
        ))
    })?;
    for row in rows {
        let (id, role, metadata) = row?;
        let row_id = id.to_string();
        if let Err(err) = parse_role("messages", &row_id, &role) {
            issues.push(err.to_string());
        }
        if let Err(err) = parse_optional_json("messages", &row_id, "metadata", metadata.as_deref())
        {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_fact_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, metadata FROM facts WHERE metadata IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, metadata) = row?;
        if let Err(err) = parse_optional_json("facts", &id, "metadata", Some(&metadata)) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_document_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare("SELECT id, metadata FROM documents WHERE metadata IS NOT NULL")?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    for row in rows {
        let (id, metadata) = row?;
        if let Err(err) = parse_optional_json("documents", &id, "metadata", Some(&metadata)) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

fn verify_episode_rows(conn: &Connection, issues: &mut Vec<String>) -> Result<(), MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT episode_id, cause_ids, outcome, verification_status
         FROM episodes",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            row.get::<_, String>(2)?,
            row.get::<_, String>(3)?,
        ))
    })?;
    for row in rows {
        let (episode_id, cause_ids, outcome, verification_status) = row?;
        if let Err(err) = parse_string_list_json("episodes", &episode_id, "cause_ids", &cause_ids) {
            issues.push(err.to_string());
        }
        if let Err(err) = parse_episode_outcome(&episode_id, &outcome) {
            issues.push(err.to_string());
        }
        if let Err(err) = parse_verification_status(&episode_id, &verification_status) {
            issues.push(err.to_string());
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
pub(crate) struct ProveKvPoolGenerationRow {
    pub generation: ProveKvPoolGenerationV1,
}

#[allow(dead_code)] // retained for provekv pool diagnostics, not currently called
fn parse_provekv_status(value: &str) -> ProveKvPoolGenerationStatus {
    match value {
        "disabled" => ProveKvPoolGenerationStatus::Disabled,
        "missing" => ProveKvPoolGenerationStatus::Missing,
        "building" => ProveKvPoolGenerationStatus::Building,
        "ready" => ProveKvPoolGenerationStatus::Ready,
        "stale" => ProveKvPoolGenerationStatus::Stale,
        "failed" => ProveKvPoolGenerationStatus::Failed,
        _ => ProveKvPoolGenerationStatus::Failed,
    }
}

fn provekv_generation_from_row(
    row: &rusqlite::Row<'_>,
) -> rusqlite::Result<ProveKvPoolGenerationRow> {
    let created_at: String = row.get(9)?;
    Ok(ProveKvPoolGenerationRow {
        generation: ProveKvPoolGenerationV1 {
            schema_version: "semantic_memory_provekv_pool_generation_v1".to_string(),
            generation_id: row.get(0)?,
            embedding_snapshot_digest: row.get(1)?,
            source_digest: row.get(2)?,
            pool_manifest_digest: row.get(3)?,
            codec_family: row.get(4)?,
            codec_profile: row.get(5)?,
            vector_dim: row.get::<_, i64>(6)? as usize,
            item_count: row.get::<_, i64>(7)? as usize,
            payload_bytes: row.get::<_, i64>(8)? as u64,
            created_at: DateTime::parse_from_rfc3339(&created_at)
                .map(|dt| dt.with_timezone(&Utc))
                .map_err(|err| {
                    rusqlite::Error::FromSqlConversionFailure(
                        9,
                        rusqlite::types::Type::Text,
                        Box::new(err),
                    )
                })?,
        },
    })
}

#[allow(dead_code)]
pub(crate) fn insert_provekv_pool_generation(
    conn: &Connection,
    generation: &ProveKvPoolGenerationV1,
    payload: &[u8],
    item_map: &[ProveKvPoolItemMapEntryV1],
) -> Result<(), MemoryError> {
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT OR REPLACE INTO provekv_pool_generations
         (generation_id, embedding_snapshot_digest, source_digest, pool_manifest_digest,
          codec_family, codec_profile, vector_dim, item_count, payload_bytes, payload,
          status, failure_reason, created_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, 'ready', NULL, ?11)",
        params![
            generation.generation_id,
            generation.embedding_snapshot_digest,
            generation.source_digest,
            generation.pool_manifest_digest,
            generation.codec_family,
            generation.codec_profile,
            generation.vector_dim as i64,
            generation.item_count as i64,
            generation.payload_bytes as i64,
            payload,
            generation.created_at.to_rfc3339(),
        ],
    )?;
    tx.execute(
        "DELETE FROM provekv_pool_item_map WHERE generation_id = ?1",
        params![generation.generation_id],
    )?;
    for entry in item_map {
        tx.execute(
            "INSERT INTO provekv_pool_item_map
             (generation_id, item_id, source_type, pool_index, embedding_digest)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                entry.generation_id,
                entry.item_id,
                entry.source_type,
                entry.pool_index as i64,
                entry.embedding_digest,
            ],
        )?;
    }
    tx.commit()?;
    Ok(())
}

pub(crate) fn latest_ready_provekv_pool_generation(
    conn: &Connection,
) -> Result<Option<ProveKvPoolGenerationRow>, MemoryError> {
    conn.query_row(
        "SELECT generation_id, embedding_snapshot_digest, source_digest, pool_manifest_digest,
                codec_family, codec_profile, vector_dim, item_count, payload_bytes, created_at
         FROM provekv_pool_generations
         WHERE status = 'ready'
         ORDER BY created_at DESC
         LIMIT 1",
        [],
        provekv_generation_from_row,
    )
    .optional()
    .map_err(MemoryError::from)
}

pub(crate) fn load_provekv_pool_payload(
    conn: &Connection,
    generation_id: &str,
) -> Result<Vec<u8>, MemoryError> {
    conn.query_row(
        "SELECT payload FROM provekv_pool_generations WHERE generation_id = ?1",
        params![generation_id],
        |row| row.get(0),
    )
    .map_err(MemoryError::from)
}

pub(crate) fn load_provekv_pool_item_map(
    conn: &Connection,
    generation_id: &str,
) -> Result<Vec<ProveKvPoolItemMapEntryV1>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT generation_id, item_id, source_type, pool_index, embedding_digest
         FROM provekv_pool_item_map
         WHERE generation_id = ?1
         ORDER BY pool_index ASC",
    )?;
    let rows = stmt.query_map(params![generation_id], |row| {
        Ok(ProveKvPoolItemMapEntryV1 {
            generation_id: row.get(0)?,
            item_id: row.get(1)?,
            source_type: row.get(2)?,
            pool_index: row.get::<_, i64>(3)? as usize,
            embedding_digest: row.get(4)?,
        })
    })?;
    let mut entries = Vec::new();
    for row in rows {
        entries.push(row?);
    }
    Ok(entries)
}

#[allow(dead_code)]
pub(crate) fn mark_provekv_pool_generation_failed(
    conn: &Connection,
    generation_id: &str,
    reason: &str,
) -> Result<(), MemoryError> {
    conn.execute(
        "UPDATE provekv_pool_generations SET status = 'failed', failure_reason = ?2 WHERE generation_id = ?1",
        params![generation_id, reason],
    )?;
    Ok(())
}

#[allow(dead_code)] // retained for provekv pool diagnostics, not currently called
pub(crate) fn provekv_pool_artifact_status(
    conn: &Connection,
) -> Result<ProveKvPoolArtifactStatusV1, MemoryError> {
    let row = conn
        .query_row(
            "SELECT generation_id, embedding_snapshot_digest, pool_manifest_digest,
                    item_count, payload_bytes, status, failure_reason
             FROM provekv_pool_generations
             ORDER BY created_at DESC
             LIMIT 1",
            [],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, Option<String>>(6)?,
                ))
            },
        )
        .optional()?;
    Ok(match row {
        Some((
            generation_id,
            snapshot_digest,
            manifest_digest,
            item_count,
            payload_bytes,
            status,
            reason,
        )) => ProveKvPoolArtifactStatusV1 {
            status: parse_provekv_status(&status),
            generation_id: Some(generation_id),
            embedding_snapshot_digest: Some(snapshot_digest),
            pool_manifest_digest: Some(manifest_digest),
            item_count: item_count as usize,
            payload_bytes: payload_bytes as u64,
            reason,
        },
        None => ProveKvPoolArtifactStatusV1 {
            status: ProveKvPoolGenerationStatus::Missing,
            generation_id: None,
            embedding_snapshot_digest: None,
            pool_manifest_digest: None,
            item_count: 0,
            payload_bytes: 0,
            reason: Some("provekv_pool_generation_not_materialized".into()),
        },
    })
}

#[cfg(test)]
mod provekv_pool_generation_db_tests {
    use super::*;

    fn test_conn() -> Connection {
        let conn = Connection::open_in_memory().expect("in-memory db opens");
        conn.execute_batch(MIGRATION_V24)
            .expect("proveKV schema migration applies");
        conn
    }

    fn generation(id: &str) -> ProveKvPoolGenerationV1 {
        ProveKvPoolGenerationV1 {
            schema_version: "semantic_memory_provekv_pool_generation_v1".to_string(),
            generation_id: id.to_string(),
            embedding_snapshot_digest: "blake3:snapshot".to_string(),
            source_digest: "blake3:source".to_string(),
            pool_manifest_digest: "blake3:manifest".to_string(),
            codec_family: "provekv_pool".to_string(),
            codec_profile: "semantic-memory-f32-derived-candidate-v1".to_string(),
            vector_dim: 4,
            item_count: 2,
            payload_bytes: 3,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn provekv_pool_generation_roundtrips_and_cascades_item_map() {
        let conn = test_conn();
        let gen = generation("gen-1");
        let item_map = vec![
            ProveKvPoolItemMapEntryV1 {
                generation_id: gen.generation_id.clone(),
                item_id: "fact-1".to_string(),
                source_type: "fact".to_string(),
                pool_index: 0,
                embedding_digest: "blake3:item-1".to_string(),
            },
            ProveKvPoolItemMapEntryV1 {
                generation_id: gen.generation_id.clone(),
                item_id: "fact-2".to_string(),
                source_type: "fact".to_string(),
                pool_index: 1,
                embedding_digest: "blake3:item-2".to_string(),
            },
        ];
        insert_provekv_pool_generation(&conn, &gen, &[1, 2, 3], &item_map).unwrap();

        let latest = latest_ready_provekv_pool_generation(&conn)
            .unwrap()
            .expect("latest ready generation");
        assert_eq!(latest.generation.generation_id, gen.generation_id);
        assert_eq!(
            load_provekv_pool_payload(&conn, "gen-1").unwrap(),
            vec![1, 2, 3]
        );
        assert_eq!(
            load_provekv_pool_item_map(&conn, "gen-1").unwrap(),
            item_map
        );

        mark_provekv_pool_generation_failed(&conn, "gen-1", "boom").unwrap();
        let status = provekv_pool_artifact_status(&conn).unwrap();
        assert_eq!(status.status, ProveKvPoolGenerationStatus::Failed);
        assert_eq!(status.reason.as_deref(), Some("boom"));

        conn.execute(
            "DELETE FROM provekv_pool_generations WHERE generation_id = 'gen-1'",
            [],
        )
        .unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM provekv_pool_item_map", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
}
