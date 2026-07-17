//! Phase 2: Semiring provenance for semantic-memory.
//!
//! Semiring-valued provenance records are stored as append-only rows in the
//! `provenance` SQLite table (migration V25). SQLite is the only truth store —
//! there is no in-memory shadow cache. Every `set_provenance()` and
//! `combine_provenance()` call emits a [`ProvenanceReceiptV1`](crate::provenance::ProvenanceReceiptV1)
//! receipt so the evidence chain is complete (AGENTS.md §0 invariant 3).
//!
//! The module is gated behind the `provenance` feature flag. The migration that
//! creates the `provenance` table is always applied (so the schema version
//! sequence stays monotonic), but the Rust API is compiled out when the feature
//! is off — the table simply remains unused in that configuration.
//!
//! ## Semirings
//!
//! | Semiring              | Carrier            | `add` | `mul` | `zero` | `one` |
//! |-----------------------|--------------------|-------|-------|--------|-------|
//! | `BooleanSemiring`     | `bool`             | OR    | AND   | false  | true  |
//! | `TropicalSemiring`    | `f64`              | min   | +     | +inf   | 0     |
//! | `ProbabilitySemiring`| `f64`              | max   | *     | 0      | 1     |
//! | `ConfidenceSemiring`  | `ConfidenceValue`  | max   | *     | 0/s0   | 1/s+1 |
//!
//! All four satisfy the semiring axioms (associativity, commutativity of add,
//! distributivity, identity, annihilation) as verified by the property tests in
//! `tests/provenance_test.rs`.
//!
//! ## Append-plus-supersession
//!
//! `set_provenance()` always INSERTs a new row. `get_provenance()` returns
//! the row with the greatest `recorded_at` for a given `(item_type, item_id)`
//! (the latest non-superseded record). Supersession propagates through the
//! semiring `add` operation via `combine_provenance()`, which reads the
//! existing record, combines it with a new value, and appends the result as a
//! fresh row — never UPDATEing a truth-bearing row (AGENTS.md §0 invariant 4).

use crate::error::MemoryError;
use crate::MemoryStore;
use crate::SearchSourceType;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::fmt;

// ─── Trait ─────────────────────────────────────────────────────────────

/// A semiring used to value provenance records.
///
/// A semiring is a ring without additive inverses: `(S, +, 0)` is a commutative
/// monoid, `(S, *, 1)` is a monoid, `*` distributes over `+`, and `0` is an
/// annihilator for `*`.
///
/// The carrier type `T` must be [`Serialize`] + [`Deserialize`] + [`Clone`] +
/// [`PartialEq`] + [`fmt::Debug`] + [`Send`] so that provenance records can be
/// persisted to SQLite as JSON and moved across `spawn_blocking` boundaries.
pub trait ProvenanceSemiring: Sized + Send + Sync + 'static {
    /// Carrier value type.
    type Value: Serialize
        + for<'de> Deserialize<'de>
        + Clone
        + PartialEq
        + fmt::Debug
        + Send
        + 'static;

    /// Additive identity (`0`): `a + 0 = a` for all `a`.
    fn zero() -> Self::Value;

    /// Multiplicative identity (`1`): `a * 1 = a` for all `a`.
    fn one() -> Self::Value;

    /// Addition (commutative monoid operation).
    fn add(a: &Self::Value, b: &Self::Value) -> Self::Value;

    /// Multiplication (monoid operation).
    fn mul(a: &Self::Value, b: &Self::Value) -> Self::Value;

    /// Whether the carrier value represents a "supported" provenance (i.e. not
    /// the annihilating zero). Used by `search_with_provenance` to filter out
    /// unsupported results.
    fn is_supported(value: &Self::Value) -> bool;

    /// Stable label used as the `semiring_type` column value in SQLite.
    fn label() -> &'static str;

    /// Serialize a carrier value to the JSON string stored in the
    /// `semiring_value` column.
    fn encode(value: &Self::Value) -> Result<String, MemoryError> {
        serde_json::to_string(value).map_err(|e| MemoryError::Other(e.to_string()))
    }

    /// Deserialize a carrier value from the `semiring_value` JSON string.
    fn decode(raw: &str) -> Result<Self::Value, MemoryError> {
        serde_json::from_str(raw).map_err(|e| MemoryError::CorruptData {
            table: "provenance",
            row_id: "(unknown)".to_string(),
            detail: format!("invalid semiring_value JSON: {e}"),
        })
    }
}

// ─── Boolean semiring ──────────────────────────────────────────────────

/// Boolean semiring: `(+, *) = (OR, AND)`, `0 = false`, `1 = true`.
#[derive(Debug, Clone, Copy)]
pub struct BooleanSemiring;

impl ProvenanceSemiring for BooleanSemiring {
    type Value = bool;

    fn zero() -> Self::Value {
        false
    }

    fn one() -> Self::Value {
        true
    }

    fn add(a: &Self::Value, b: &Self::Value) -> Self::Value {
        *a || *b
    }

    fn mul(a: &Self::Value, b: &Self::Value) -> Self::Value {
        *a && *b
    }

    fn is_supported(value: &Self::Value) -> bool {
        *value
    }

    fn label() -> &'static str {
        "boolean"
    }
}

// ─── Tropical semiring ──────────────────────────────────────────────────

/// Tropical (min-plus) semiring: `(+, *) = (min, +)`, `0 = +inf`, `1 = 0`.
///
/// Natural for computing minimal-cost provenance chains (e.g. decoder
/// correction costs — see Phase 6 matrix/06b_corrections.md).
#[derive(Debug, Clone, Copy)]
pub struct TropicalSemiring;

impl ProvenanceSemiring for TropicalSemiring {
    type Value = f64;

    fn zero() -> Self::Value {
        f64::INFINITY
    }

    fn one() -> Self::Value {
        0.0
    }

    fn add(a: &Self::Value, b: &Self::Value) -> Self::Value {
        a.min(*b)
    }

    fn mul(a: &Self::Value, b: &Self::Value) -> Self::Value {
        a + b
    }

    fn is_supported(value: &Self::Value) -> bool {
        value.is_finite()
    }

    fn label() -> &'static str {
        "tropical"
    }

    // Override encode/decode because serde_json serializes f64::INFINITY as null.
    fn encode(value: &Self::Value) -> Result<String, MemoryError> {
        if value.is_infinite() {
            Ok("\"Infinity\"".to_string())
        } else if value.is_nan() {
            Ok("\"NaN\"".to_string())
        } else {
            serde_json::to_string(value).map_err(|e| MemoryError::Other(e.to_string()))
        }
    }

    fn decode(raw: &str) -> Result<Self::Value, MemoryError> {
        if raw == "\"Infinity\"" || raw == "Infinity" {
            return Ok(f64::INFINITY);
        }
        if raw == "\"NaN\"" || raw == "NaN" {
            return Ok(f64::NAN);
        }
        serde_json::from_str::<f64>(raw).map_err(|e| MemoryError::CorruptData {
            table: "provenance",
            row_id: "(unknown)".to_string(),
            detail: format!("invalid semiring_value JSON: {e}"),
        })
    }
}

// ─── Probability semiring ──────────────────────────────────────────────

/// Probability (max-times) semiring: `(+, *) = (max, *)`, `0 = 0`, `1 = 1`.
///
/// Natural for combining independent probability-like scores along a
/// provenance chain.
#[derive(Debug, Clone, Copy)]
pub struct ProbabilitySemiring;

impl ProvenanceSemiring for ProbabilitySemiring {
    type Value = f64;

    fn zero() -> Self::Value {
        0.0
    }

    fn one() -> Self::Value {
        1.0
    }

    fn add(a: &Self::Value, b: &Self::Value) -> Self::Value {
        a.max(*b)
    }

    fn mul(a: &Self::Value, b: &Self::Value) -> Self::Value {
        a * b
    }

    fn is_supported(value: &Self::Value) -> bool {
        *value > 0.0
    }

    fn label() -> &'static str {
        "probability"
    }
}

// ─── Confidence semiring ────────────────────────────────────────────────

/// A confidence value with a support count.
///
/// The carrier for the [`ConfidenceSemiring`]: a confidence score in `[0,1]`
/// plus a `support_count` recording how many independent pieces of evidence
/// contributed to it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ConfidenceValue {
    /// Confidence score in `[0.0, 1.0]`.
    pub confidence: f64,
    /// Number of independent supporting observations.
    pub support_count: u32,
}

impl ConfidenceValue {
    /// Create a new confidence value.
    pub const fn new(confidence: f64, support_count: u32) -> Self {
        Self {
            confidence,
            support_count,
        }
    }
}

/// Confidence semiring: `(+, *) = (max-confidence, *-with-count-increment)`.
///
/// - `zero` = confidence `0.0`, support_count `0` (unsupported).
/// - `one`   = confidence `1.0`, support_count `1` (single self-supporting
///   observation).
/// - `add`   = take the higher confidence; on a tie, the higher support_count.
/// - `mul`   = multiply confidences and sum support counts (independent
///   evidence chains combine multiplicatively).
#[derive(Debug, Clone, Copy)]
pub struct ConfidenceSemiring;

impl ProvenanceSemiring for ConfidenceSemiring {
    type Value = ConfidenceValue;

    fn zero() -> Self::Value {
        ConfidenceValue::new(0.0, 0)
    }

    fn one() -> Self::Value {
        // Multiplicative identity: confidence 1.0, support_count 0.
        // a * one() must equal a, so one() must not add to support_count.
        ConfidenceValue::new(1.0, 0)
    }

    fn add(a: &Self::Value, b: &Self::Value) -> Self::Value {
        if a.confidence > b.confidence {
            *a
        } else if b.confidence > a.confidence {
            *b
        } else {
            // Tie on confidence — keep the higher support_count.
            ConfidenceValue::new(a.confidence, a.support_count.max(b.support_count))
        }
    }

    fn mul(a: &Self::Value, b: &Self::Value) -> Self::Value {
        // If either operand is zero (confidence 0), the result is zero.
        // This ensures annihilation: a * 0 = 0 and 0 * a = 0.
        if a.confidence == 0.0 || b.confidence == 0.0 {
            return ConfidenceSemiring::zero();
        }
        ConfidenceValue::new(
            a.confidence * b.confidence,
            a.support_count.saturating_add(b.support_count),
        )
    }

    fn is_supported(value: &Self::Value) -> bool {
        value.confidence > 0.0 && value.support_count > 0
    }

    fn label() -> &'static str {
        "confidence"
    }
}

// ─── Provenance record + receipt ───────────────────────────────────────

/// The kind of item a provenance record is attached to.
///
/// Mirrors the `item_type` column. We use a stringly-typed enum so new item
/// types can be added additively without a schema change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceItemType {
    /// A fact row (`facts.id`).
    Fact,
    /// A document chunk (`chunks.id`).
    Chunk,
    /// A conversation message (`messages.id`).
    Message,
    /// An episode (`episodes.episode_id`).
    Episode,
}

impl ProvenanceItemType {
    /// Stable string label stored in the `item_type` column.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fact => "fact",
            Self::Chunk => "chunk",
            Self::Message => "message",
            Self::Episode => "episode",
        }
    }

    /// Parse a stored item_type label back into the enum.
    pub fn from_str_value(s: &str) -> Option<Self> {
        match s {
            "fact" => Some(Self::Fact),
            "chunk" => Some(Self::Chunk),
            "message" => Some(Self::Message),
            "episode" => Some(Self::Episode),
            _ => None,
        }
    }
}

impl fmt::Display for ProvenanceItemType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A provenance record stored in the `provenance` table.
///
/// This is the deserialized form of one row. The `semiring_value` is kept as
/// the raw JSON string to avoid requiring a type parameter at the storage
/// boundary; callers deserialize it via [`ProvenanceSemiring::decode`] using
/// the semiring identified by `semiring_type`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceRecord {
    /// UUID v4 primary key.
    pub id: String,
    /// Item kind label (e.g. "fact", "chunk", "episode").
    pub item_type: String,
    /// Authoritative row id for the item (e.g. fact UUID, episode_id).
    pub item_id: String,
    /// Semiring label (e.g. "boolean", "tropical", "probability", "confidence").
    pub semiring_type: String,
    /// JSON-encoded semiring carrier value.
    pub semiring_value: String,
    /// JSON array of support-chain item references (e.g. `["fact:abc", "chunk:def"]`).
    pub support_chain_json: String,
    /// Recorded time (SQLite `datetime('now')`), UTC ISO-8601.
    pub recorded_at: String,
    /// Optional episode_id this provenance record is attached to.
    pub episode_id: Option<String>,
}

/// Receipt for a provenance write operation (set or combine).
///
/// Every provenance mutation emits one of these so the evidence chain is
/// complete (AGENTS.md §0 invariant 3: "No receipt = it didn't happen").
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceReceiptV1 {
    /// Schema version label.
    pub schema_version: &'static str,
    /// The provenance row id that was written.
    pub provenance_id: String,
    /// Item type label.
    pub item_type: String,
    /// Item id.
    pub item_id: String,
    /// Semiring label.
    pub semiring_type: String,
    /// Semiring carrier value JSON (post-combine for `combine`, the new value
    /// for `set`).
    pub semiring_value: String,
    /// Whether this receipt was produced by a combine (append-supersession)
    /// or a plain set.
    pub operation: ProvenanceOperation,
    /// Recorded time of the written row.
    pub recorded_at: String,
    /// Optional episode id.
    pub episode_id: Option<String>,
}

/// What kind of provenance mutation produced this receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceOperation {
    /// A fresh `set_provenance` write.
    Set,
    /// A `combine_provenance` append (read-combine-append, never UPDATE).
    Combine,
}

// ─── Low-level SQLite helpers ──────────────────────────────────────────

/// Insert a new provenance row. Append-only — never UPDATEs an existing row.
///
/// Returns the fully-populated [`ProvenanceRecord`] that was written.
pub(crate) fn insert_provenance(
    conn: &Connection,
    id: &str,
    item_type: &str,
    item_id: &str,
    semiring_type: &str,
    semiring_value: &str,
    support_chain_json: &str,
    episode_id: Option<&str>,
) -> Result<ProvenanceRecord, MemoryError> {
    // Use chrono UTC with microsecond precision to avoid timestamp collisions
    // that break ORDER BY recorded_at DESC (SQLite datetime('now') is second-precision).
    let recorded_at = chrono::Utc::now()
        .format("%Y-%m-%d %H:%M:%S%.6f")
        .to_string();
    db_with_transaction(conn, |tx| {
        tx.execute(
            "INSERT INTO provenance
                (id, item_type, item_id, semiring_type, semiring_value,
                 support_chain_json, recorded_at, episode_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                id,
                item_type,
                item_id,
                semiring_type,
                semiring_value,
                support_chain_json,
                recorded_at,
                episode_id,
            ],
        )
        .map_err(MemoryError::Database)?;

        let recorded_at: String = tx.query_row(
            "SELECT recorded_at FROM provenance WHERE id = ?1",
            params![id],
            |row| row.get(0),
        )?;

        Ok(ProvenanceRecord {
            id: id.to_string(),
            item_type: item_type.to_string(),
            item_id: item_id.to_string(),
            semiring_type: semiring_type.to_string(),
            semiring_value: semiring_value.to_string(),
            support_chain_json: support_chain_json.to_string(),
            recorded_at,
            episode_id: episode_id.map(|s| s.to_string()),
        })
    })
}

/// Read the latest provenance record for `(item_type, item_id)` matching the
/// given semiring label. "Latest" = greatest `recorded_at`.
///
/// Returns `Ok(None)` when no matching row exists.
pub(crate) fn read_latest_provenance(
    conn: &Connection,
    item_type: &str,
    item_id: &str,
    semiring_type: &str,
) -> Result<Option<ProvenanceRecord>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, item_type, item_id, semiring_type, semiring_value,
                support_chain_json, recorded_at, episode_id
         FROM provenance
         WHERE item_type = ?1 AND item_id = ?2 AND semiring_type = ?3
         ORDER BY recorded_at DESC, id DESC
         LIMIT 1",
    )?;
    let mut rows = stmt.query(params![item_type, item_id, semiring_type])?;
    match rows.next()? {
        Some(row) => Ok(Some(ProvenanceRecord {
            id: row.get(0)?,
            item_type: row.get(1)?,
            item_id: row.get(2)?,
            semiring_type: row.get(3)?,
            semiring_value: row.get(4)?,
            support_chain_json: row.get(5)?,
            recorded_at: row.get(6)?,
            episode_id: row.get(7)?,
        })),
        None => Ok(None),
    }
}

/// Read all provenance records for `(item_type, item_id)` matching the given
/// semiring label, ordered by `recorded_at` ascending (oldest first).
pub(crate) fn read_provenance_history(
    conn: &Connection,
    item_type: &str,
    item_id: &str,
    semiring_type: &str,
) -> Result<Vec<ProvenanceRecord>, MemoryError> {
    let mut stmt = conn.prepare(
        "SELECT id, item_type, item_id, semiring_type, semiring_value,
                support_chain_json, recorded_at, episode_id
         FROM provenance
         WHERE item_type = ?1 AND item_id = ?2 AND semiring_type = ?3
         ORDER BY recorded_at ASC, id ASC",
    )?;
    let rows = stmt.query_map(params![item_type, item_id, semiring_type], |row| {
        Ok(ProvenanceRecord {
            id: row.get(0)?,
            item_type: row.get(1)?,
            item_id: row.get(2)?,
            semiring_type: row.get(3)?,
            semiring_value: row.get(4)?,
            support_chain_json: row.get(5)?,
            recorded_at: row.get(6)?,
            episode_id: row.get(7)?,
        })
    })?;
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Run a closure inside an unchecked transaction, committing on success.
///
/// Mirrors [`crate::db::with_transaction`] but kept local to this module so the
/// feature can be compiled independently. Uses `unchecked_transaction` to
/// match the existing crate pattern (foreign keys are managed by the caller).
fn db_with_transaction<F, T>(conn: &Connection, f: F) -> Result<T, MemoryError>
where
    F: FnOnce(&rusqlite::Transaction<'_>) -> Result<T, MemoryError>,
{
    let tx = conn.unchecked_transaction()?;
    let result = f(&tx)?;
    tx.commit()?;
    Ok(result)
}

/// Build a receipt from a written record + operation kind.
fn build_receipt(record: &ProvenanceRecord, operation: ProvenanceOperation) -> ProvenanceReceiptV1 {
    ProvenanceReceiptV1 {
        schema_version: "provenance.v1",
        provenance_id: record.id.clone(),
        item_type: record.item_type.clone(),
        item_id: record.item_id.clone(),
        semiring_type: record.semiring_type.clone(),
        semiring_value: record.semiring_value.clone(),
        operation,
        recorded_at: record.recorded_at.clone(),
        episode_id: record.episode_id.clone(),
    }
}

/// Generate a UUID v4 for a new provenance row.
fn new_provenance_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// Encode the support chain as a JSON array of strings.
fn encode_support_chain(chain: &[String]) -> Result<String, MemoryError> {
    serde_json::to_string(chain).map_err(|e| MemoryError::Other(e.to_string()))
}

/// Decode a support chain JSON array back into `Vec<String>`.
fn decode_support_chain(raw: &str) -> Result<Vec<String>, MemoryError> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(raw).map_err(|e| MemoryError::CorruptData {
        table: "provenance",
        row_id: "(unknown)".to_string(),
        detail: format!("invalid support_chain_json: {e}"),
    })
}

// ─── MemoryStore API ───────────────────────────────────────────────────

impl MemoryStore {
    /// Set (append) a fresh provenance record for `(item_type, item_id)`.
    ///
    /// This always INSERTs a new row — it never UPDATEs an existing one
    /// (append-plus-supersession, AGENTS.md §0 invariant 4). Use
    /// [`combine_provenance`](Self::combine_provenance) to read-combine-append.
    ///
    /// Returns a receipt for the written row.
    pub async fn set_provenance<S: ProvenanceSemiring>(
        &self,
        item_type: &ProvenanceItemType,
        item_id: &str,
        value: &S::Value,
        support_chain: &[String],
        episode_id: Option<&str>,
    ) -> Result<ProvenanceReceiptV1, MemoryError> {
        let id = new_provenance_id();
        let item_type_label = item_type.as_str().to_string();
        let item_id_owned = item_id.to_string();
        let semiring_label = S::label().to_string();
        let semiring_value = S::encode(value)?;
        let support_chain_json = encode_support_chain(support_chain)?;
        let episode_id_owned = episode_id.map(|s| s.to_string());

        let record = self
            .with_write_conn(move |conn| {
                insert_provenance(
                    conn,
                    &id,
                    &item_type_label,
                    &item_id_owned,
                    &semiring_label,
                    &semiring_value,
                    &support_chain_json,
                    episode_id_owned.as_deref(),
                )
            })
            .await?;

        Ok(build_receipt(&record, ProvenanceOperation::Set))
    }

    /// Get the latest provenance value for `(item_type, item_id)` under
    /// semiring `S`.
    ///
    /// Returns `Ok(None)` when no provenance record exists for this item under
    /// the requested semiring.
    pub async fn get_provenance<S: ProvenanceSemiring>(
        &self,
        item_type: &ProvenanceItemType,
        item_id: &str,
    ) -> Result<Option<(S::Value, Vec<String>)>, MemoryError> {
        let item_type_label = item_type.as_str().to_string();
        let item_id_owned = item_id.to_string();
        let semiring_label = S::label().to_string();

        self.with_read_conn(move |conn| {
            let Some(record) =
                read_latest_provenance(conn, &item_type_label, &item_id_owned, &semiring_label)?
            else {
                return Ok(None);
            };

            // Guard against a semiring_type mismatch in the stored row. This
            // can happen if a caller switches semirings for the same item
            // without going through combine; we surface it as corrupt data so
            // it is not silently widened.
            if record.semiring_type != semiring_label {
                return Err(MemoryError::CorruptData {
                    table: "provenance",
                    row_id: record.id,
                    detail: format!(
                        "semiring_type mismatch: stored '{}', requested '{}'",
                        record.semiring_type, semiring_label
                    ),
                });
            }

            let value = S::decode(&record.semiring_value)?;
            let chain = decode_support_chain(&record.support_chain_json)?;
            Ok(Some((value, chain)))
        })
        .await
    }

    /// Combine a new provenance value with the latest existing one for
    /// `(item_type, item_id)` under semiring `S`, then append the result.
    ///
    /// The combine uses the semiring `add` operation: `new = add(old, value)`.
    /// If no existing record exists, this is equivalent to `set_provenance`.
    /// The support chains are merged (union preserving order: existing first,
    /// then new entries not already present).
    ///
    /// This never UPDATEs a truth-bearing row — it reads the latest record and
    /// INSERTs a new combined row (append-plus-supersession).
    ///
    /// Returns a receipt for the newly written combined row.
    pub async fn combine_provenance<S: ProvenanceSemiring>(
        &self,
        item_type: &ProvenanceItemType,
        item_id: &str,
        value: &S::Value,
        support_chain: &[String],
        episode_id: Option<&str>,
    ) -> Result<ProvenanceReceiptV1, MemoryError> {
        let id = new_provenance_id();
        let item_type_label = item_type.as_str().to_string();
        let item_id_owned = item_id.to_string();
        let semiring_label = S::label().to_string();
        let semiring_label_for_read = semiring_label.clone();
        let item_type_for_read = item_type_label.clone();
        let item_id_for_read = item_id_owned.clone();
        let new_value = value.clone();
        let new_chain = support_chain.to_vec();
        let episode_id_owned = episode_id.map(|s| s.to_string());
        let episode_id_for_read = episode_id_owned.clone();

        let record = self
            .with_write_conn(move |conn| {
                // Read the latest existing record inside the same write
                // transaction so the read-combine-append is atomic with respect
                // to other writers. This is the canonical append-supersession
                // pattern: read current truth, compute the combined value, and
                // INSERT a new row — never UPDATE the old one.
                let existing = read_latest_provenance(
                    conn,
                    &item_type_for_read,
                    &item_id_for_read,
                    &semiring_label_for_read,
                )?;

                let (combined_value, combined_chain, existing_episode_id) = match existing {
                    Some(record) => {
                        if record.semiring_type != semiring_label_for_read {
                            return Err(MemoryError::CorruptData {
                                table: "provenance",
                                row_id: record.id,
                                detail: format!(
                                    "semiring_type mismatch: stored '{}', requested '{}'",
                                    record.semiring_type, semiring_label_for_read
                                ),
                            });
                        }
                        let old_value = S::decode(&record.semiring_value)?;
                        let old_chain = decode_support_chain(&record.support_chain_json)?;
                        let combined = S::add(&old_value, &new_value);
                        let merged = merge_support_chains(&old_chain, &new_chain);
                        // Preserve the existing episode_id when the caller does
                        // not supply one (supersession propagation).
                        let ep = episode_id_for_read.clone().or(record.episode_id.clone());
                        (combined, merged, ep)
                    }
                    None => {
                        // No existing record — this is the first write for this
                        // item under this semiring. The "combined" value is just
                        // the new value (add with zero would be wrong for
                        // semirings where zero is not the true identity for
                        // arbitrary inputs, e.g. tropical +inf). Use the value
                        // directly to match set_provenance semantics.
                        (
                            new_value.clone(),
                            new_chain.clone(),
                            episode_id_for_read.clone(),
                        )
                    }
                };

                let semiring_value = S::encode(&combined_value)?;
                let support_chain_json = encode_support_chain(&combined_chain)?;

                insert_provenance(
                    conn,
                    &id,
                    &item_type_label,
                    &item_id_owned,
                    &semiring_label,
                    &semiring_value,
                    &support_chain_json,
                    existing_episode_id.as_deref(),
                )
            })
            .await?;

        Ok(build_receipt(&record, ProvenanceOperation::Combine))
    }

    /// Search across facts, chunks, and episodes, then attach the latest
    /// provenance value for each result that has one under semiring `S`.
    ///
    /// Results without provenance are retained but flagged with
    /// [`ProvenanceAnnotation::Unsupported`]. Results whose provenance
    /// `is_supported()` returns false are likewise flagged `Unsupported`.
    /// Supported results carry the decoded semiring value and support chain.
    ///
    /// This is a thin wrapper over [`MemoryStore::search`] that does one extra
    /// read pass per result to fetch provenance — it does not alter the search
    /// ranking. The search itself remains deterministic (AGENTS.md §0: no LLM
    /// stages in the retrieval pipeline).
    pub async fn search_with_provenance<S: ProvenanceSemiring>(
        &self,
        query: &str,
        top_k: Option<usize>,
        namespaces: Option<&[&str]>,
        source_types: Option<&[SearchSourceType]>,
    ) -> Result<Vec<ProvenanceSearchResult<S::Value>>, MemoryError> {
        let results = self.search(query, top_k, namespaces, source_types).await?;

        let mut annotated = Vec::with_capacity(results.len());
        for result in results {
            let (item_type, item_id) = provenance_key_for_source(&result.source);
            let provenance = self.get_provenance::<S>(&item_type, &item_id).await?;

            let annotation = match provenance {
                Some((value, chain)) => {
                    if S::is_supported(&value) {
                        ProvenanceAnnotation::Supported {
                            value,
                            support_chain: chain,
                        }
                    } else {
                        ProvenanceAnnotation::Unsupported
                    }
                }
                None => ProvenanceAnnotation::Unsupported,
            };

            annotated.push(ProvenanceSearchResult {
                result,
                provenance: annotation,
            });
        }
        Ok(annotated)
    }

    /// Read the full provenance history for `(item_type, item_id)` under
    /// semiring `S`, oldest first. Useful for audit / supersession replay.
    pub async fn provenance_history<S: ProvenanceSemiring>(
        &self,
        item_type: &ProvenanceItemType,
        item_id: &str,
    ) -> Result<Vec<ProvenanceRecord>, MemoryError> {
        let item_type_label = item_type.as_str().to_string();
        let item_id_owned = item_id.to_string();
        let semiring_label = S::label().to_string();
        self.with_read_conn(move |conn| {
            read_provenance_history(conn, &item_type_label, &item_id_owned, &semiring_label)
        })
        .await
    }
}

// ─── Search-annotation types ────────────────────────────────────────────

/// A search result annotated with its provenance value.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProvenanceSearchResult<T> {
    /// The underlying search result.
    pub result: crate::SearchResult,
    /// The provenance annotation for this result.
    pub provenance: ProvenanceAnnotation<T>,
}

/// Provenance annotation on a search result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ProvenanceAnnotation<T> {
    /// The result has supported provenance under the requested semiring.
    Supported {
        /// Decoded semiring carrier value.
        value: T,
        /// Support chain item references.
        support_chain: Vec<String>,
    },
    /// The result has no provenance under the requested semiring, or its
    /// provenance value is the semiring zero (unsupported).
    Unsupported,
}

// ─── Helpers ───────────────────────────────────────────────────────────

/// Merge two support chains: existing entries first, then new entries that are
/// not already present (order-preserving union).
fn merge_support_chains(existing: &[String], new: &[String]) -> Vec<String> {
    let mut merged = existing.to_vec();
    for entry in new {
        if !merged.contains(entry) {
            merged.push(entry.clone());
        }
    }
    merged
}

/// Map a [`crate::SearchSource`] to the `(item_type, item_id)` pair used to key
/// provenance records.
fn provenance_key_for_source(source: &crate::SearchSource) -> (ProvenanceItemType, String) {
    match source {
        crate::SearchSource::Fact { fact_id, .. } => (ProvenanceItemType::Fact, fact_id.clone()),
        crate::SearchSource::Chunk { chunk_id, .. } => {
            (ProvenanceItemType::Chunk, chunk_id.clone())
        }
        crate::SearchSource::Message { message_id, .. } => {
            (ProvenanceItemType::Message, message_id.to_string())
        }
        crate::SearchSource::Episode { episode_id, .. } => {
            (ProvenanceItemType::Episode, episode_id.clone())
        }
        // Projections are not currently backed by provenance records; treat as
        // unsupported (the annotation will be `Unsupported`).
        crate::SearchSource::Projection { projection_id, .. } => {
            // Use the projection_id as the item_id; there is no dedicated
            // ProvenanceItemType variant for projections yet (additive only).
            // We map to the closest existing type and rely on get_provenance
            // returning None for unknown item types, which yields Unsupported.
            (ProvenanceItemType::Fact, projection_id.clone())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── Boolean semiring properties ────────────────────────────────────

    fn bool_values() -> Vec<bool> {
        vec![false, true]
    }

    #[test]
    fn boolean_add_identity() {
        for &a in &bool_values() {
            assert_eq!(
                BooleanSemiring::add(&a, &BooleanSemiring::zero()),
                a,
                "a + 0 = a"
            );
            assert_eq!(
                BooleanSemiring::add(&BooleanSemiring::zero(), &a),
                a,
                "0 + a = a"
            );
        }
    }

    #[test]
    fn boolean_mul_identity() {
        for &a in &bool_values() {
            assert_eq!(
                BooleanSemiring::mul(&a, &BooleanSemiring::one()),
                a,
                "a * 1 = a"
            );
            assert_eq!(
                BooleanSemiring::mul(&BooleanSemiring::one(), &a),
                a,
                "1 * a = a"
            );
        }
    }

    #[test]
    fn boolean_annihilation() {
        for &a in &bool_values() {
            assert_eq!(
                BooleanSemiring::mul(&a, &BooleanSemiring::zero()),
                false,
                "a * 0 = 0"
            );
            assert_eq!(
                BooleanSemiring::mul(&BooleanSemiring::zero(), &a),
                false,
                "0 * a = 0"
            );
        }
    }

    #[test]
    fn boolean_add_commutativity() {
        for &a in &bool_values() {
            for &b in &bool_values() {
                assert_eq!(
                    BooleanSemiring::add(&a, &b),
                    BooleanSemiring::add(&b, &a),
                    "add commutative"
                );
            }
        }
    }

    #[test]
    fn boolean_add_associativity() {
        for &a in &bool_values() {
            for &b in &bool_values() {
                for &c in &bool_values() {
                    let left = BooleanSemiring::add(&BooleanSemiring::add(&a, &b), &c);
                    let right = BooleanSemiring::add(&a, &BooleanSemiring::add(&b, &c));
                    assert_eq!(left, right, "add associative");
                }
            }
        }
    }

    #[test]
    fn boolean_mul_associativity() {
        for &a in &bool_values() {
            for &b in &bool_values() {
                for &c in &bool_values() {
                    let left = BooleanSemiring::mul(&BooleanSemiring::mul(&a, &b), &c);
                    let right = BooleanSemiring::mul(&a, &BooleanSemiring::mul(&b, &c));
                    assert_eq!(left, right, "mul associative");
                }
            }
        }
    }

    #[test]
    fn boolean_distributivity() {
        for &a in &bool_values() {
            for &b in &bool_values() {
                for &c in &bool_values() {
                    let left = BooleanSemiring::mul(&a, &BooleanSemiring::add(&b, &c));
                    let right = BooleanSemiring::add(
                        &BooleanSemiring::mul(&a, &b),
                        &BooleanSemiring::mul(&a, &c),
                    );
                    assert_eq!(left, right, "a*(b+c) = a*b + a*c");
                }
            }
        }
    }

    // ── Tropical semiring properties ────────────────────────────────────

    fn tropical_values() -> Vec<f64> {
        vec![0.0, 1.0, 2.5, 100.0, f64::INFINITY]
    }

    fn approx_eq(a: f64, b: f64) -> bool {
        if a.is_infinite() && b.is_infinite() {
            a.signum() == b.signum()
        } else if a.is_infinite() || b.is_infinite() {
            false
        } else {
            (a - b).abs() < 1e-9
        }
    }

    #[test]
    fn tropical_add_identity() {
        for &a in &tropical_values() {
            assert!(
                approx_eq(TropicalSemiring::add(&a, &TropicalSemiring::zero()), a),
                "a + 0 = a"
            );
        }
    }

    #[test]
    fn tropical_mul_identity() {
        for &a in &tropical_values() {
            assert!(
                approx_eq(TropicalSemiring::mul(&a, &TropicalSemiring::one()), a),
                "a * 1 = a"
            );
        }
    }

    #[test]
    fn tropical_annihilation() {
        for &a in &tropical_values() {
            assert!(
                approx_eq(
                    TropicalSemiring::mul(&a, &TropicalSemiring::zero()),
                    f64::INFINITY
                ),
                "a * 0 = 0 (inf)"
            );
        }
    }

    #[test]
    fn tropical_add_commutativity() {
        for &a in &tropical_values() {
            for &b in &tropical_values() {
                assert!(
                    approx_eq(TropicalSemiring::add(&a, &b), TropicalSemiring::add(&b, &a)),
                    "add commutative"
                );
            }
        }
    }

    #[test]
    fn tropical_add_associativity() {
        for &a in &tropical_values() {
            for &b in &tropical_values() {
                for &c in &tropical_values() {
                    let left = TropicalSemiring::add(&TropicalSemiring::add(&a, &b), &c);
                    let right = TropicalSemiring::add(&a, &TropicalSemiring::add(&b, &c));
                    assert!(approx_eq(left, right), "add associative");
                }
            }
        }
    }

    #[test]
    fn tropical_mul_associativity() {
        for &a in &tropical_values() {
            for &b in &tropical_values() {
                for &c in &tropical_values() {
                    let left = TropicalSemiring::mul(&TropicalSemiring::mul(&a, &b), &c);
                    let right = TropicalSemiring::mul(&a, &TropicalSemiring::mul(&b, &c));
                    assert!(approx_eq(left, right), "mul associative");
                }
            }
        }
    }

    #[test]
    fn tropical_distributivity() {
        for &a in &tropical_values() {
            for &b in &tropical_values() {
                for &c in &tropical_values() {
                    let left = TropicalSemiring::mul(&a, &TropicalSemiring::add(&b, &c));
                    let right = TropicalSemiring::add(
                        &TropicalSemiring::mul(&a, &b),
                        &TropicalSemiring::mul(&a, &c),
                    );
                    assert!(approx_eq(left, right), "a*(b+c) = a*b + a*c");
                }
            }
        }
    }

    // ── Probability semiring properties ─────────────────────────────────

    fn prob_values() -> Vec<f64> {
        vec![0.0, 0.25, 0.5, 0.75, 1.0]
    }

    #[test]
    fn probability_add_identity() {
        for &a in &prob_values() {
            assert!(
                approx_eq(
                    ProbabilitySemiring::add(&a, &ProbabilitySemiring::zero()),
                    a
                ),
                "a + 0 = a"
            );
        }
    }

    #[test]
    fn probability_mul_identity() {
        for &a in &prob_values() {
            assert!(
                approx_eq(ProbabilitySemiring::mul(&a, &ProbabilitySemiring::one()), a),
                "a * 1 = a"
            );
        }
    }

    #[test]
    fn probability_annihilation() {
        for &a in &prob_values() {
            assert!(
                approx_eq(
                    ProbabilitySemiring::mul(&a, &ProbabilitySemiring::zero()),
                    0.0
                ),
                "a * 0 = 0"
            );
        }
    }

    #[test]
    fn probability_add_commutativity() {
        for &a in &prob_values() {
            for &b in &prob_values() {
                assert!(
                    approx_eq(
                        ProbabilitySemiring::add(&a, &b),
                        ProbabilitySemiring::add(&b, &a)
                    ),
                    "add commutative"
                );
            }
        }
    }

    #[test]
    fn probability_add_associativity() {
        for &a in &prob_values() {
            for &b in &prob_values() {
                for &c in &prob_values() {
                    let left = ProbabilitySemiring::add(&ProbabilitySemiring::add(&a, &b), &c);
                    let right = ProbabilitySemiring::add(&a, &ProbabilitySemiring::add(&b, &c));
                    assert!(approx_eq(left, right), "add associative");
                }
            }
        }
    }

    #[test]
    fn probability_mul_associativity() {
        for &a in &prob_values() {
            for &b in &prob_values() {
                for &c in &prob_values() {
                    let left = ProbabilitySemiring::mul(&ProbabilitySemiring::mul(&a, &b), &c);
                    let right = ProbabilitySemiring::mul(&a, &ProbabilitySemiring::mul(&b, &c));
                    assert!(approx_eq(left, right), "mul associative");
                }
            }
        }
    }

    #[test]
    fn probability_distributivity() {
        for &a in &prob_values() {
            for &b in &prob_values() {
                for &c in &prob_values() {
                    let left = ProbabilitySemiring::mul(&a, &ProbabilitySemiring::add(&b, &c));
                    let right = ProbabilitySemiring::add(
                        &ProbabilitySemiring::mul(&a, &b),
                        &ProbabilitySemiring::mul(&a, &c),
                    );
                    assert!(approx_eq(left, right), "a*(b+c) = a*b + a*c");
                }
            }
        }
    }

    // ── Confidence semiring properties ─────────────────────────────────

    fn confidence_values() -> Vec<ConfidenceValue> {
        vec![
            ConfidenceValue::new(0.0, 0),
            ConfidenceValue::new(0.25, 1),
            ConfidenceValue::new(0.5, 2),
            ConfidenceValue::new(0.75, 3),
            ConfidenceValue::new(1.0, 5),
        ]
    }

    fn confidence_approx_eq(a: ConfidenceValue, b: ConfidenceValue) -> bool {
        approx_eq(a.confidence, b.confidence) && a.support_count == b.support_count
    }

    #[test]
    fn confidence_add_identity() {
        for a in &confidence_values() {
            assert!(
                confidence_approx_eq(ConfidenceSemiring::add(a, &ConfidenceSemiring::zero()), *a),
                "a + 0 = a"
            );
        }
    }

    #[test]
    fn confidence_mul_identity() {
        for a in &confidence_values() {
            assert!(
                confidence_approx_eq(ConfidenceSemiring::mul(a, &ConfidenceSemiring::one()), *a),
                "a * 1 = a"
            );
        }
    }

    #[test]
    fn confidence_annihilation() {
        for a in &confidence_values() {
            let result = ConfidenceSemiring::mul(a, &ConfidenceSemiring::zero());
            assert!(
                confidence_approx_eq(result, ConfidenceSemiring::zero()),
                "a * 0 = 0"
            );
        }
    }

    #[test]
    fn confidence_add_commutativity() {
        for a in &confidence_values() {
            for b in &confidence_values() {
                assert!(
                    confidence_approx_eq(
                        ConfidenceSemiring::add(a, b),
                        ConfidenceSemiring::add(b, a)
                    ),
                    "add commutative"
                );
            }
        }
    }

    #[test]
    fn confidence_add_associativity() {
        for a in &confidence_values() {
            for b in &confidence_values() {
                for c in &confidence_values() {
                    let left = ConfidenceSemiring::add(&ConfidenceSemiring::add(a, b), c);
                    let right = ConfidenceSemiring::add(a, &ConfidenceSemiring::add(b, c));
                    assert!(confidence_approx_eq(left, right), "add associative");
                }
            }
        }
    }

    #[test]
    fn confidence_mul_associativity() {
        for a in &confidence_values() {
            for b in &confidence_values() {
                for c in &confidence_values() {
                    let left = ConfidenceSemiring::mul(&ConfidenceSemiring::mul(a, b), c);
                    let right = ConfidenceSemiring::mul(a, &ConfidenceSemiring::mul(b, c));
                    assert!(confidence_approx_eq(left, right), "mul associative");
                }
            }
        }
    }

    #[test]
    fn confidence_distributivity() {
        for a in &confidence_values() {
            for b in &confidence_values() {
                for c in &confidence_values() {
                    let left = ConfidenceSemiring::mul(a, &ConfidenceSemiring::add(b, c));
                    let right = ConfidenceSemiring::add(
                        &ConfidenceSemiring::mul(a, b),
                        &ConfidenceSemiring::mul(a, c),
                    );
                    assert!(confidence_approx_eq(left, right), "a*(b+c) = a*b + a*c");
                }
            }
        }
    }

    // ── Label + support ─────────────────────────────────────────────────

    #[test]
    fn semiring_labels_are_stable() {
        assert_eq!(BooleanSemiring::label(), "boolean");
        assert_eq!(TropicalSemiring::label(), "tropical");
        assert_eq!(ProbabilitySemiring::label(), "probability");
        assert_eq!(ConfidenceSemiring::label(), "confidence");
    }

    #[test]
    fn is_supported_matches_zero() {
        assert!(!BooleanSemiring::is_supported(&false));
        assert!(BooleanSemiring::is_supported(&true));

        assert!(!TropicalSemiring::is_supported(&f64::INFINITY));
        assert!(TropicalSemiring::is_supported(&0.0));

        assert!(!ProbabilitySemiring::is_supported(&0.0));
        assert!(ProbabilitySemiring::is_supported(&0.5));

        assert!(!ConfidenceSemiring::is_supported(&ConfidenceValue::new(
            0.0, 0
        )));
        assert!(ConfidenceSemiring::is_supported(&ConfidenceValue::new(
            0.5, 1
        )));
    }

    // ── Encode/decode round-trip ───────────────────────────────────────

    #[test]
    fn boolean_encode_decode_roundtrip() {
        for &v in &bool_values() {
            let s = BooleanSemiring::encode(&v).unwrap();
            let back = BooleanSemiring::decode(&s).unwrap();
            assert_eq!(back, v);
        }
    }

    #[test]
    fn tropical_encode_decode_roundtrip() {
        for &v in &tropical_values() {
            let s = TropicalSemiring::encode(&v).unwrap();
            let back = TropicalSemiring::decode(&s).unwrap();
            assert!(approx_eq(back, v));
        }
    }

    #[test]
    fn probability_encode_decode_roundtrip() {
        for &v in &prob_values() {
            let s = ProbabilitySemiring::encode(&v).unwrap();
            let back = ProbabilitySemiring::decode(&s).unwrap();
            assert!(approx_eq(back, v));
        }
    }

    #[test]
    fn confidence_encode_decode_roundtrip() {
        for v in &confidence_values() {
            let s = ConfidenceSemiring::encode(v).unwrap();
            let back = ConfidenceSemiring::decode(&s).unwrap();
            assert!(confidence_approx_eq(back, *v));
        }
    }

    // ── Support chain merge ─────────────────────────────────────────────

    #[test]
    fn merge_support_chains_preserves_order_and_dedups() {
        let existing = vec!["a".to_string(), "b".to_string()];
        let new = vec!["b".to_string(), "c".to_string()];
        let merged = merge_support_chains(&existing, &new);
        assert_eq!(merged, vec!["a", "b", "c"]);
    }

    #[test]
    fn merge_support_chains_empty_existing() {
        let merged = merge_support_chains(&[], &["x".to_string()]);
        assert_eq!(merged, vec!["x"]);
    }

    #[test]
    fn merge_support_chains_empty_new() {
        let merged = merge_support_chains(&["x".to_string()], &[]);
        assert_eq!(merged, vec!["x"]);
    }
}
