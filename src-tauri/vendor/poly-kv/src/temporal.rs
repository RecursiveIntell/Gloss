//! Bitemporal audit trail for pool and shell supersessions.
//!
//! When the `temporal-truth` feature is active, every pool build and
//! shell materialization is recorded as a bitemporal record. This
//! enables:
//!
//! - **as-of queries**: "what pool was live at time T?"
//! - **rollback**: revert to a prior pool by querying supersession history
//! - **cryptographic chain**: `SupersessionReceipt` binds BLAKE3 pool digests
//!   via the superseding/superseded digest fields
//!
//! # Storage model
//!
//! Since `bitemporal_runtime::InMemoryDb` is typed to `BitemporalRecord<()>`,
//! this module defines its own lightweight `KvAuditDb` that stores
//! `BitemporalRecord<serde_json::Value>`. Domain values (`PoolAuditValue`,
//! `ShellAuditValue`) are serialized to/from `serde_json::Value` at the
//! boundary.
//!
//! # Usage
//!
//! ```rust,ignore
//! use poly_kv::temporal::{build_pool_with_audit, materialize_shell_with_audit, KvAuditDb};
//!
//! let mut db = KvAuditDb::new();
//! let (pool, receipt, audit) = build_pool_with_audit(&corpus, &shape, 42, &mut db)?;
//! let (shell, mat_receipt, shell_audit) =
//!     materialize_shell_with_audit(&pool, "agent_1", &[], 42, &mut db)?;
//! ```

#[cfg(feature = "temporal-truth")]
use bitemporal_runtime::{BitemporalRecord, RecordId, SupersessionReceipt};

#[cfg(feature = "temporal-truth")]
use chrono::{DateTime, Utc};

use crate::error::Result;
use crate::pool::SharedKVPool;
use crate::receipt::{PoolBuildReceipt, ShellMaterializeReceipt};
use crate::shape::KvTensorShape;

// ── Typed audit store ──────────────────────────────────────────────

/// Lightweight bitemporal store for poly-kv audit records.
///
/// Stores `BitemporalRecord<serde_json::Value>` so that any
/// `Serialize`-able domain value can be inserted without changing
/// the store's type parameter.
#[cfg(feature = "temporal-truth")]
#[derive(Debug, Clone, Default)]
pub struct KvAuditDb {
    records: std::collections::BTreeMap<RecordId, Vec<BitemporalRecord<serde_json::Value>>>,
}

#[cfg(feature = "temporal-truth")]
impl KvAuditDb {
    /// Create a new empty audit store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Insert an audit record.
    pub fn insert(&mut self, record: BitemporalRecord<serde_json::Value>) {
        let id = record.id.clone();
        self.records.entry(id).or_default().push(record);
    }

    /// Get all versions of an audit record by ID.
    pub fn get_versions(&self, id: &str) -> Option<&Vec<BitemporalRecord<serde_json::Value>>> {
        self.records.get(id)
    }

    /// Returns the count of unique record IDs in this store.
    pub fn len(&self) -> usize {
        self.records.len()
    }

    /// Returns true if this store has no records.
    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

// ── Domain value types ──────────────────────────────────────────────

/// The domain value stored in a bitemporal record for a pool build event.
///
/// This captures the pool digest and the build receipt digest — enough
/// to uniquely identify a pool version and verify its integrity, without
/// storing the entire pool (which is kept in the `SharedKVPool` struct).
#[cfg(feature = "temporal-truth")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct PoolAuditValue {
    /// BLAKE3 digest of the pool (from `PoolBuildReceipt::pool_digest`).
    pub pool_digest: String,
    /// BLAKE3 digest of the build receipt (from `PoolBuildReceipt::digest()`).
    pub receipt_digest: String,
    /// Number of shared tokens in the pool.
    pub num_shared_tokens: u32,
    /// Compression ratio achieved.
    pub compression_ratio: f64,
    /// Backend used: "cpu" or "gpu".
    pub backend: String,
}

/// The domain value stored in a bitemporal record for a shell materialization event.
#[cfg(feature = "temporal-truth")]
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ShellAuditValue {
    /// Agent identifier.
    pub agent_id: String,
    /// BLAKE3 digest of the shell (from `ShellMaterializeReceipt::shell_digest`).
    pub shell_digest: String,
    /// BLAKE3 digest of the pool this shell was materialized from.
    pub pool_digest: String,
    /// Number of agent-specific tokens in the shell.
    pub num_agent_tokens: u32,
}

/// Audit receipt returned when a pool build is recorded bitemporally.
#[cfg(feature = "temporal-truth")]
#[derive(Debug, Clone)]
pub struct PoolAuditReceipt {
    /// The bitemporal record ID for this pool build.
    pub record_id: RecordId,
    /// The supersession receipt (if a previous pool was superseded).
    pub supersession: Option<SupersessionReceipt>,
}

/// Audit receipt returned when a shell materialization is recorded bitemporally.
#[cfg(feature = "temporal-truth")]
#[derive(Debug, Clone)]
pub struct ShellAuditReceipt {
    /// The bitemporal record ID for this shell materialization.
    pub record_id: RecordId,
    /// The supersession receipt (if a previous shell for this agent was superseded).
    pub supersession: Option<SupersessionReceipt>,
}

// ── Helper: serialize a typed value to serde_json::Value ────────────

#[cfg(feature = "temporal-truth")]
fn to_json_value<T: serde::Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}

#[cfg(feature = "temporal-truth")]
fn from_json_value<T: serde::de::DeserializeOwned>(value: &serde_json::Value) -> Option<T> {
    serde_json::from_value(value.clone()).ok()
}

// ── Public API ─────────────────────────────────────────────────────

/// Build a pool and record the build as a bitemporal record.
///
/// This wraps `SharedKVPool::build` and additionally:
/// 1. Creates a `BitemporalRecord<serde_json::Value>` in the audit store
/// 2. If a previous pool existed under the same record ID, produces a
///    `SupersessionReceipt` binding the old and new pool via digests
///
/// The record ID is derived from the pool digest (`pool:{digest}`),
/// ensuring that identical pool builds are idempotent.
#[cfg(feature = "temporal-truth")]
pub fn build_pool_with_audit(
    corpus: &[(String, Vec<f32>)],
    shape: &KvTensorShape,
    seed: u64,
    db: &mut KvAuditDb,
) -> Result<(SharedKVPool, PoolBuildReceipt, PoolAuditReceipt)> {
    let (pool, receipt) = SharedKVPool::build(corpus, shape, seed)?;

    let pool_digest_hex = receipt.pool_digest.hex().to_string();
    let receipt_digest_hex = receipt
        .digest()
        .map(|d| d.hex().to_string())
        .unwrap_or_default();

    let audit_value = PoolAuditValue {
        pool_digest: pool_digest_hex.clone(),
        receipt_digest: receipt_digest_hex,
        num_shared_tokens: receipt.total_tokens,
        compression_ratio: receipt.compression_ratio,
        backend: receipt.backend.clone(),
    };

    let now = Utc::now();
    let record_id = format!("pool:{}", pool_digest_hex);

    let record = BitemporalRecord {
        id: record_id.clone(),
        valid_time: now,
        recorded_time: now,
        value: to_json_value(&audit_value),
    };

    // Check if a prior version exists for this record ID
    let supersession = if let Some(prior_versions) = db.get_versions(&record_id) {
        if let Some(prior) = prior_versions.last() {
            let superseding = record.clone();
            let superseded = prior.clone();
            Some(SupersessionReceipt::new(superseded, superseding))
        } else {
            None
        }
    } else {
        None
    };

    db.insert(record);

    let audit_receipt = PoolAuditReceipt {
        record_id,
        supersession,
    };

    Ok((pool, receipt, audit_receipt))
}

/// Materialize a shell and record the materialization as a bitemporal record.
///
/// The record ID is `shell:{agent_id}:{pool_digest}`, which ensures
/// that re-materializing the same shell for the same agent from the
/// same pool produces a supersession (not a duplicate).
#[cfg(feature = "temporal-truth")]
pub fn materialize_shell_with_audit(
    pool: &SharedKVPool,
    agent_id: &str,
    agent_tokens: &[(String, Vec<f32>)],
    seed: u64,
    db: &mut KvAuditDb,
) -> Result<(
    crate::shell::AgentShell,
    ShellMaterializeReceipt,
    ShellAuditReceipt,
)> {
    let (shell, receipt) = pool.materialize_shell(agent_id, agent_tokens, seed)?;

    let shell_digest_hex = receipt.shell_digest.hex().to_string();
    let pool_digest_hex = pool.manifest.pool_id.hex().to_string();

    let audit_value = ShellAuditValue {
        agent_id: agent_id.to_string(),
        shell_digest: shell_digest_hex.clone(),
        pool_digest: pool_digest_hex.clone(),
        num_agent_tokens: receipt.num_unique_tokens,
    };

    let now = Utc::now();
    let record_id = format!("shell:{}:{}", agent_id, pool_digest_hex);

    let record = BitemporalRecord {
        id: record_id.clone(),
        valid_time: now,
        recorded_time: now,
        value: to_json_value(&audit_value),
    };

    // Check if a prior version exists for this record ID
    let supersession = if let Some(prior_versions) = db.get_versions(&record_id) {
        if let Some(prior) = prior_versions.last() {
            let superseding = record.clone();
            let superseded = prior.clone();
            Some(SupersessionReceipt::new(superseded, superseding))
        } else {
            None
        }
    } else {
        None
    };

    db.insert(record);

    let audit_receipt = ShellAuditReceipt {
        record_id,
        supersession,
    };

    Ok((shell, receipt, audit_receipt))
}

/// Query which pool was live at a given point in time.
///
/// Returns the `PoolAuditValue` for the pool that was valid at `valid_time`
/// as known by the system at `recorded_time`.
#[cfg(feature = "temporal-truth")]
pub fn query_pool_as_of(
    db: &KvAuditDb,
    pool_record_id: &str,
    valid_time: DateTime<Utc>,
    recorded_time: DateTime<Utc>,
) -> Option<PoolAuditValue> {
    let versions = db.get_versions(pool_record_id)?;
    versions
        .iter()
        .filter(|r| r.recorded_time <= recorded_time && r.valid_time <= valid_time)
        .last()
        .and_then(|r| from_json_value(&r.value))
}

/// Query which shell was live for a given agent at a given point in time.
#[cfg(feature = "temporal-truth")]
pub fn query_shell_as_of(
    db: &KvAuditDb,
    shell_record_id: &str,
    valid_time: DateTime<Utc>,
    recorded_time: DateTime<Utc>,
) -> Option<ShellAuditValue> {
    let versions = db.get_versions(shell_record_id)?;
    versions
        .iter()
        .filter(|r| r.recorded_time <= recorded_time && r.valid_time <= valid_time)
        .last()
        .and_then(|r| from_json_value(&r.value))
}

// ── Stubs when temporal-truth is off ────────────────────────────────

/// Pool audit value (stub when `temporal-truth` is off).
#[cfg(not(feature = "temporal-truth"))]
pub struct PoolAuditValue;

/// Shell audit value (stub when `temporal-truth` is off).
#[cfg(not(feature = "temporal-truth"))]
pub struct ShellAuditValue;

/// Audit DB stub (stub when `temporal-truth` is off).
#[cfg(not(feature = "temporal-truth"))]
pub struct KvAuditDb;
