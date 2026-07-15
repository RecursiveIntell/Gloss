//! SQLite-backed bitemporal store.
//!
//! This is the durable counterpart to [`InMemoryDb`]. It implements
//! the same surface — `insert`, `snapshot_at` — against a single
//! SQLite table `bitemporal_records`:
//!
//! ```sql
//! CREATE TABLE bitemporal_records (
//!     record_id      TEXT NOT NULL,
//!     valid_time     INTEGER NOT NULL,  -- unix epoch seconds
//!     recorded_time  INTEGER NOT NULL,  -- unix epoch seconds
//!     superseded_by  TEXT,              -- NULL if current
//!     value_json     BLOB NOT NULL,     -- the payload, JSON-encoded
//!     PRIMARY KEY (record_id, valid_time, recorded_time)
//! );
//! ```
//!
//! `insert` is the only mutating operation. It inserts a new row and
//! marks all prior non-superseded rows for the same `record_id` with
//! `superseded_by = <new_record_id>`. This is the SQLite-side analogue
//! of the in-memory `append_supersede`.

use crate::BitemporalError;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use std::collections::BTreeMap;
use std::path::Path;

/// Durable receipt proving a legacy schema migration occurred.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MigrationReceipt {
    /// Receipt wire schema.
    pub schema_version: String,
    /// Source schema identifier.
    pub from_schema_version: String,
    /// Destination schema identifier.
    pub to_schema_version: String,
    /// Number of rows migrated.
    pub migrated_row_count: usize,
    /// Digest binding the sorted canonical event IDs produced by migration.
    pub migrated_event_ids_digest: String,
}

/// SQLite-backed bitemporal store. Wraps a single `rusqlite::Connection`.
#[derive(Debug)]
pub struct SqliteDb {
    conn: Connection,
}

impl SqliteDb {
    /// Open (or create) a SQLite-backed bitemporal store at the given path.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, BitemporalError> {
        let conn = Connection::open(path).map_err(|e| {
            BitemporalError::DatabaseError(format!("failed to open sqlite db: {e}"))
        })?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Open an in-memory SQLite-backed bitemporal store. Useful for tests
    /// that want the persistence semantics of SQLite without touching disk.
    pub fn open_in_memory() -> Result<Self, BitemporalError> {
        let conn = Connection::open_in_memory().map_err(|e| {
            BitemporalError::DatabaseError(format!("failed to open in-memory sqlite db: {e}"))
        })?;
        let db = Self { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> Result<(), BitemporalError> {
        const CANONICAL_SCHEMA: &str = "canonical_event_v3";
        let has_table: bool = self.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='bitemporal_records')",
            [], |row| row.get(0),
        ).map_err(|e| BitemporalError::DatabaseError(format!("migration probe failed: {e}")))?;
        if has_table {
            let has_event_id: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM pragma_table_info('bitemporal_records') WHERE name='event_id')",
                [], |row| row.get(0),
            ).map_err(|e| BitemporalError::DatabaseError(format!("migration probe failed: {e}")))?;
            if !has_event_id {
                self.migrate_seconds_v1()?;
            } else {
                let has_schema_table: bool = self.conn.query_row(
                    "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='bitemporal_schema')",
                    [], |row| row.get(0),
                ).map_err(|e| BitemporalError::DatabaseError(format!("schema probe failed: {e}")))?;
                let schema_version = if has_schema_table {
                    self.conn
                        .query_row(
                            "SELECT schema_version FROM bitemporal_schema WHERE singleton = 1",
                            [],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()
                        .map_err(|e| {
                            BitemporalError::DatabaseError(format!("schema query failed: {e}"))
                        })?
                } else {
                    None
                };
                if schema_version.as_deref() != Some(CANONICAL_SCHEMA) {
                    self.migrate_event_rows(schema_version.as_deref().unwrap_or("event_v2"))?;
                }
            }
        }
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS bitemporal_records (
                    event_id       TEXT NOT NULL PRIMARY KEY,
                    record_id      TEXT NOT NULL,
                    valid_time_ns  INTEGER NOT NULL,
                    recorded_time_ns INTEGER NOT NULL,
                    superseded_by  TEXT,
                    value_json     BLOB NOT NULL
                );
                CREATE TABLE IF NOT EXISTS bitemporal_schema (
                    singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
                    schema_version TEXT NOT NULL
                );
                INSERT OR REPLACE INTO bitemporal_schema VALUES (1, 'canonical_event_v3');
                CREATE TABLE IF NOT EXISTS bitemporal_migration_receipts (
                    receipt_id TEXT NOT NULL PRIMARY KEY,
                    receipt_json BLOB NOT NULL
                );
                CREATE INDEX IF NOT EXISTS idx_bt_recorded ON bitemporal_records(recorded_time_ns, event_id);
                CREATE INDEX IF NOT EXISTS idx_bt_superseded ON bitemporal_records(superseded_by);",
            )
            .map_err(|e| BitemporalError::DatabaseError(format!("migration failed: {e}")))?;
        Ok(())
    }

    fn migrate_seconds_v1(&self) -> Result<(), BitemporalError> {
        let tx = self.conn.unchecked_transaction().map_err(|error| {
            BitemporalError::DatabaseError(format!("legacy migration transaction failed: {error}"))
        })?;
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_bt_recorded;
             DROP INDEX IF EXISTS idx_bt_superseded;
             ALTER TABLE bitemporal_records RENAME TO bitemporal_records_seconds_v1;",
        )
        .map_err(|error| {
            BitemporalError::DatabaseError(format!("legacy migration failed: {error}"))
        })?;
        let raw_rows = {
            let mut statement = tx.prepare("SELECT record_id, valid_time, recorded_time, superseded_by, value_json FROM bitemporal_records_seconds_v1 ORDER BY record_id, valid_time, recorded_time, value_json")
                .map_err(|error| BitemporalError::DatabaseError(format!("legacy migration read failed: {error}")))?;
            let mapped = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, Option<String>>(3)?,
                        row.get::<_, Vec<u8>>(4)?,
                    ))
                })
                .map_err(|error| {
                    BitemporalError::DatabaseError(format!("legacy migration read failed: {error}"))
                })?;
            mapped.collect::<Result<Vec<_>, _>>().map_err(|error| {
                BitemporalError::DatabaseError(format!("legacy migration decode failed: {error}"))
            })?
        };
        let rows = raw_rows
            .into_iter()
            .map(|(record_id, valid, recorded, superseded_by, value_json)| {
                MigrationRow::new(
                    None,
                    record_id,
                    valid.checked_mul(1_000_000_000).ok_or_else(|| {
                        BitemporalError::SerializationError("legacy valid_time overflow".into())
                    })?,
                    recorded.checked_mul(1_000_000_000).ok_or_else(|| {
                        BitemporalError::SerializationError("legacy recorded_time overflow".into())
                    })?,
                    superseded_by,
                    value_json,
                )
            })
            .collect::<Result<Vec<_>, _>>()?;
        rebuild_migrated_table(&tx, "bitemporal_records_seconds_v1", "seconds_v1", &rows)?;
        tx.commit().map_err(|error| {
            BitemporalError::DatabaseError(format!("legacy migration commit failed: {error}"))
        })
    }

    fn migrate_event_rows(&self, from_schema: &str) -> Result<(), BitemporalError> {
        let tx = self.conn.unchecked_transaction().map_err(|error| {
            BitemporalError::DatabaseError(format!("event migration transaction failed: {error}"))
        })?;
        let raw_rows = {
            let mut statement = tx.prepare("SELECT event_id, record_id, valid_time_ns, recorded_time_ns, superseded_by, value_json FROM bitemporal_records ORDER BY record_id, valid_time_ns, recorded_time_ns, event_id")
                .map_err(|error| BitemporalError::DatabaseError(format!("event migration read failed: {error}")))?;
            let mapped = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                        row.get::<_, Option<String>>(4)?,
                        row.get::<_, Vec<u8>>(5)?,
                    ))
                })
                .map_err(|error| {
                    BitemporalError::DatabaseError(format!("event migration read failed: {error}"))
                })?;
            mapped.collect::<Result<Vec<_>, _>>().map_err(|error| {
                BitemporalError::DatabaseError(format!("event migration decode failed: {error}"))
            })?
        };
        let rows = raw_rows
            .into_iter()
            .map(
                |(old_id, record_id, valid_ns, recorded_ns, superseded_by, value_json)| {
                    MigrationRow::new(
                        Some(old_id),
                        record_id,
                        valid_ns,
                        recorded_ns,
                        superseded_by,
                        value_json,
                    )
                },
            )
            .collect::<Result<Vec<_>, _>>()?;
        tx.execute_batch(
            "DROP INDEX IF EXISTS idx_bt_recorded;
             DROP INDEX IF EXISTS idx_bt_superseded;
             ALTER TABLE bitemporal_records RENAME TO bitemporal_records_event_legacy;",
        )
        .map_err(|error| {
            BitemporalError::DatabaseError(format!("event migration rename failed: {error}"))
        })?;
        rebuild_migrated_table(&tx, "bitemporal_records_event_legacy", from_schema, &rows)?;
        tx.commit().map_err(|error| {
            BitemporalError::DatabaseError(format!("event migration commit failed: {error}"))
        })
    }

    /// Read the canonical event identity for the latest version of a logical record.
    pub fn event_id_for_record(&self, record_id: &str) -> Result<Option<String>, BitemporalError> {
        self.conn.query_row("SELECT event_id FROM bitemporal_records WHERE record_id = ?1 ORDER BY recorded_time_ns DESC, event_id DESC LIMIT 1", [record_id], |row| row.get(0))
            .optional().map_err(|error| BitemporalError::DatabaseError(format!("event identity query failed: {error}")))
    }

    /// Read all durable migration receipts in deterministic order.
    pub fn migration_receipts(&self) -> Result<Vec<MigrationReceipt>, BitemporalError> {
        let mut statement = self
            .conn
            .prepare("SELECT receipt_json FROM bitemporal_migration_receipts ORDER BY receipt_id")
            .map_err(|error| {
                BitemporalError::DatabaseError(format!("migration receipt query failed: {error}"))
            })?;
        let rows = statement
            .query_map([], |row| row.get::<_, Vec<u8>>(0))
            .map_err(|error| {
                BitemporalError::DatabaseError(format!("migration receipt query failed: {error}"))
            })?;
        rows.map(|row| {
            let bytes = row.map_err(|error| {
                BitemporalError::DatabaseError(format!("migration receipt decode failed: {error}"))
            })?;
            serde_json::from_slice(&bytes)
                .map_err(|error| BitemporalError::SerializationError(error.to_string()))
        })
        .collect()
    }

    /// Insert a bitemporal record. Returns the number of prior rows
    /// that were marked superseded (0 if this is the first version).
    pub fn insert(
        &self,
        record: crate::BitemporalRecord<serde_json::Value>,
    ) -> Result<usize, BitemporalError> {
        let value_bytes = serde_json::to_vec(&record.value).map_err(|e| {
            BitemporalError::SerializationError(format!("value serialization failed: {e}"))
        })?;
        let valid_ns = record.valid_time.timestamp_nanos_opt().ok_or_else(|| {
            BitemporalError::SerializationError("valid_time outside nanosecond range".into())
        })?;
        let recorded_ns = record.recorded_time.timestamp_nanos_opt().ok_or_else(|| {
            BitemporalError::SerializationError("recorded_time outside nanosecond range".into())
        })?;
        let event_id = record.try_event_id()?;

        let tx = self
            .conn
            .unchecked_transaction()
            .map_err(|e| BitemporalError::DatabaseError(format!("failed to begin tx: {e}")))?;

        // Step 1: mark all prior non-superseded rows superseded.
        // Must run BEFORE the insert so the new row doesn't get
        // marked superseded by itself.
        let superseded_count = tx
            .execute(
                "UPDATE bitemporal_records
                 SET superseded_by = ?1
                 WHERE record_id = ?2
                   AND superseded_by IS NULL",
                params![event_id, record.id],
            )
            .map_err(|e| {
                BitemporalError::DatabaseError(format!("supersession update failed: {e}"))
            })?;

        // Step 2: insert the new row.
        tx.execute(
            "INSERT INTO bitemporal_records
             (event_id, record_id, valid_time_ns, recorded_time_ns, superseded_by, value_json)
             VALUES (?1, ?2, ?3, ?4, NULL, ?5)",
            params![event_id, record.id, valid_ns, recorded_ns, value_bytes,],
        )
        .map_err(|e| BitemporalError::DatabaseError(format!("insert failed: {e}")))?;

        tx.commit()
            .map_err(|e| BitemporalError::DatabaseError(format!("commit failed: {e}")))?;

        Ok(superseded_count)
    }

    /// Snapshot of the bitemporal state at `recorded_time`. Returns one
    /// record per `record_id` — the latest version of that record whose
    /// `recorded_time` is `<= recorded_time`, regardless of whether it
    /// has been superseded since. This is the "as of" semantics: what
    /// did we believe at time T, even if a later version has
    /// superseded it.
    pub fn snapshot_at(
        &self,
        recorded_time: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<crate::BitemporalRecord<serde_json::Value>>, BitemporalError> {
        let query_ns = recorded_time.timestamp_nanos_opt().ok_or_else(|| {
            BitemporalError::DatabaseError("snapshot timestamp outside nanosecond range".into())
        })?;
        let mut stmt = self
            .conn
            .prepare(
                "SELECT record_id, valid_time_ns, recorded_time_ns, value_json
                 FROM bitemporal_records
                 WHERE recorded_time_ns <= ?1
                 ORDER BY record_id, recorded_time_ns DESC, valid_time_ns DESC, event_id DESC",
            )
            .map_err(|e| BitemporalError::DatabaseError(format!("snapshot prepare failed: {e}")))?;

        let rows = stmt
            .query_map(params![query_ns], |row| {
                let id: String = row.get(0)?;
                let valid_ts: i64 = row.get(1)?;
                let recorded_ts: i64 = row.get(2)?;
                let value_bytes: Vec<u8> = row.get(3)?;
                let value: serde_json::Value =
                    serde_json::from_slice(&value_bytes).map_err(|e| {
                        rusqlite::Error::FromSqlConversionFailure(
                            3,
                            rusqlite::types::Type::Blob,
                            Box::new(e),
                        )
                    })?;
                let valid_time = chrono::DateTime::<chrono::Utc>::from_timestamp(
                    valid_ts.div_euclid(1_000_000_000),
                    valid_ts.rem_euclid(1_000_000_000) as u32,
                )
                .ok_or_else(|| rusqlite::Error::InvalidQuery)?;
                let recorded_time = chrono::DateTime::<chrono::Utc>::from_timestamp(
                    recorded_ts.div_euclid(1_000_000_000),
                    recorded_ts.rem_euclid(1_000_000_000) as u32,
                )
                .ok_or_else(|| rusqlite::Error::InvalidQuery)?;
                Ok(crate::BitemporalRecord {
                    id,
                    valid_time,
                    recorded_time,
                    value,
                })
            })
            .map_err(|e| BitemporalError::DatabaseError(format!("snapshot query failed: {e}")))?;

        // Dedup: keep only the latest recorded_time per record_id.
        let mut by_id: std::collections::BTreeMap<
            String,
            crate::BitemporalRecord<serde_json::Value>,
        > = std::collections::BTreeMap::new();
        for row in rows {
            let r = row.map_err(|e| BitemporalError::DatabaseError(format!("row decode: {e}")))?;
            by_id
                .entry(r.id.clone())
                .and_modify(|existing| {
                    if r.recorded_time > existing.recorded_time {
                        *existing = r.clone();
                    }
                })
                .or_insert(r);
        }
        Ok(by_id.into_values().collect())
    }
}

#[derive(Debug)]
struct MigrationRow {
    old_event_id: Option<String>,
    event_id: String,
    record_id: String,
    valid_time_ns: i64,
    recorded_time_ns: i64,
    superseded_by: Option<String>,
    value_json: Vec<u8>,
}

impl MigrationRow {
    fn new(
        old_event_id: Option<String>,
        record_id: String,
        valid_time_ns: i64,
        recorded_time_ns: i64,
        superseded_by: Option<String>,
        value_json: Vec<u8>,
    ) -> Result<Self, BitemporalError> {
        let value: serde_json::Value = serde_json::from_slice(&value_json).map_err(|error| {
            BitemporalError::SerializationError(format!("legacy value is not valid JSON: {error}"))
        })?;
        let valid_time = chrono::DateTime::from_timestamp(
            valid_time_ns.div_euclid(1_000_000_000),
            valid_time_ns.rem_euclid(1_000_000_000) as u32,
        )
        .ok_or_else(|| BitemporalError::SerializationError("invalid legacy valid_time".into()))?;
        let recorded_time = chrono::DateTime::from_timestamp(
            recorded_time_ns.div_euclid(1_000_000_000),
            recorded_time_ns.rem_euclid(1_000_000_000) as u32,
        )
        .ok_or_else(|| {
            BitemporalError::SerializationError("invalid legacy recorded_time".into())
        })?;
        let event_id = crate::BitemporalRecord {
            id: record_id.clone(),
            valid_time,
            recorded_time,
            value,
        }
        .try_event_id()?;
        Ok(Self {
            old_event_id,
            event_id,
            record_id,
            valid_time_ns,
            recorded_time_ns,
            superseded_by,
            value_json,
        })
    }
}

fn rebuild_migrated_table(
    tx: &Transaction<'_>,
    legacy_table: &str,
    from_schema: &str,
    rows: &[MigrationRow],
) -> Result<(), BitemporalError> {
    tx.execute_batch(
        "CREATE TABLE bitemporal_records (
           event_id TEXT NOT NULL PRIMARY KEY,
           record_id TEXT NOT NULL,
           valid_time_ns INTEGER NOT NULL,
           recorded_time_ns INTEGER NOT NULL,
           superseded_by TEXT,
           value_json BLOB NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bitemporal_schema (
           singleton INTEGER PRIMARY KEY CHECK(singleton = 1),
           schema_version TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS bitemporal_migration_receipts (
           receipt_id TEXT NOT NULL PRIMARY KEY,
           receipt_json BLOB NOT NULL
         );",
    )
    .map_err(|error| {
        BitemporalError::DatabaseError(format!("migration rebuild failed: {error}"))
    })?;

    let old_to_new: BTreeMap<&str, &str> = rows
        .iter()
        .filter_map(|row| {
            row.old_event_id
                .as_deref()
                .map(|old| (old, row.event_id.as_str()))
        })
        .collect();
    let mut latest_by_record: BTreeMap<&str, &MigrationRow> = BTreeMap::new();
    for row in rows {
        latest_by_record
            .entry(&row.record_id)
            .and_modify(|current| {
                if (row.recorded_time_ns, row.valid_time_ns, &row.event_id)
                    > (
                        current.recorded_time_ns,
                        current.valid_time_ns,
                        &current.event_id,
                    )
                {
                    *current = row;
                }
            })
            .or_insert(row);
    }

    for row in rows {
        let superseded_by = row.superseded_by.as_deref().and_then(|target| {
            old_to_new.get(target).copied().or_else(|| {
                latest_by_record
                    .get(target)
                    .map(|row| row.event_id.as_str())
            })
        });
        tx.execute(
            "INSERT INTO bitemporal_records
             (event_id, record_id, valid_time_ns, recorded_time_ns, superseded_by, value_json)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                row.event_id,
                row.record_id,
                row.valid_time_ns,
                row.recorded_time_ns,
                superseded_by,
                row.value_json
            ],
        )
        .map_err(|error| {
            BitemporalError::DatabaseError(format!("migration row insert failed: {error}"))
        })?;
    }

    let receipt = migration_receipt(
        from_schema,
        rows.iter().map(|row| row.event_id.clone()).collect(),
    );
    let receipt_json = serde_json::to_vec(&receipt)
        .map_err(|error| BitemporalError::SerializationError(error.to_string()))?;
    tx.execute(
        "INSERT OR REPLACE INTO bitemporal_schema VALUES (1, 'canonical_event_v3')",
        [],
    )
    .and_then(|_| {
        tx.execute(
            "INSERT OR REPLACE INTO bitemporal_migration_receipts (receipt_id, receipt_json)
             VALUES (?1, ?2)",
            params![receipt.migrated_event_ids_digest, receipt_json],
        )
    })
    .map_err(|error| {
        BitemporalError::DatabaseError(format!("migration receipt write failed: {error}"))
    })?;
    tx.execute_batch(&format!("DROP TABLE {legacy_table};"))
        .map_err(|error| {
            BitemporalError::DatabaseError(format!("legacy table cleanup failed: {error}"))
        })?;
    Ok(())
}

fn migration_receipt(from_schema: &str, mut event_ids: Vec<String>) -> MigrationReceipt {
    use sha2::{Digest, Sha256};
    event_ids.sort();
    let mut digest = Sha256::new();
    digest.update(b"recursiveintell:bitemporal-migration-receipt:v1\0");
    for event_id in &event_ids {
        digest.update((event_id.len() as u64).to_be_bytes());
        digest.update(event_id.as_bytes());
    }
    MigrationReceipt {
        schema_version: "bitemporal_migration_receipt_v1".into(),
        from_schema_version: from_schema.into(),
        to_schema_version: "canonical_event_v3".into(),
        migrated_row_count: event_ids.len(),
        migrated_event_ids_digest: format!("{:x}", digest.finalize()),
    }
}
