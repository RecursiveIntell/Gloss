pub mod app_db;
pub mod doctor;
pub mod migrations;
pub mod notebook_db;
pub mod notebook_lifecycle;
pub mod notebook_pool;
pub mod portable;

#[derive(Debug, PartialEq, Eq)]
struct WalCheckpointResult {
    busy: i64,
    log_frames: i64,
    checkpointed_frames: i64,
}

/// Execute SQLite's truncating WAL checkpoint and decode its three integer
/// result columns (`busy`, `log`, and `checkpointed`) without assuming they are
/// strings. This is the canonical checkpoint operation for startup recovery.
fn truncating_wal_checkpoint(
    connection: &rusqlite::Connection,
) -> rusqlite::Result<WalCheckpointResult> {
    connection.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |row| {
        Ok(WalCheckpointResult {
            busy: row.get(0)?,
            log_frames: row.get(1)?,
            checkpointed_frames: row.get(2)?,
        })
    })
}

/// Executing the pragma successfully does not prove integrity: SQLite returns
/// diagnostics as rows. Accept only the complete, single `ok` result.
fn verify_database_integrity(connection: &rusqlite::Connection) -> rusqlite::Result<()> {
    let mut statement = connection.prepare("PRAGMA integrity_check")?;
    let diagnoses = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if diagnoses.len() == 1 && diagnoses[0] == "ok" {
        return Ok(());
    }
    Err(rusqlite::Error::SqliteFailure(
        rusqlite::ffi::Error::new(rusqlite::ffi::SQLITE_CORRUPT),
        Some(format!(
            "Database integrity check failed: {}",
            diagnoses.join("; ")
        )),
    ))
}

/// Recover WAL state on all databases at startup.
///
/// Stale WAL files from a previous crash can cause "read-only database" errors
/// on Linux. This function checkpoints and truncates the WAL on every known
/// database before any application logic runs, then verifies integrity.
///
/// Failures here are logged but not fatal — the application can still start
/// with degraded WAL state and will retry on the next write.
pub fn recover_all_wal_on_startup(data_dir: &std::path::Path) {
    use rusqlite::Connection;

    let mut dbs: Vec<std::path::PathBuf> = Vec::new();

    // Application-level databases
    dbs.push(data_dir.join("gloss.db"));
    dbs.push(data_dir.join("queue.db"));

    // Per-notebook databases
    if let Ok(entries) = std::fs::read_dir(data_dir.join("notebooks")) {
        for entry in entries.flatten() {
            let nb_db = entry.path().join("notebook.db");
            if nb_db.exists() {
                dbs.push(nb_db);
            }
        }
    }

    let mut verified = 0usize;
    let mut failed = 0usize;
    for db_path in &dbs {
        if !db_path.exists() {
            continue;
        }
        let path_str = db_path.display().to_string();
        let connection = match db_path.file_name().and_then(|name| name.to_str()) {
            Some("notebook.db") => {
                migrations::open_connection(db_path, migrations::DatabaseSchema::Notebook)
            }
            Some("gloss.db") => {
                migrations::open_connection(db_path, migrations::DatabaseSchema::App)
            }
            _ => Connection::open(db_path), // Queue schema remains dependency-owned.
        };
        match connection {
            Ok(conn) => {
                // Recovery is a write boundary too. Unsupported notebook/app
                // stores must be quarantined before checkpointing their WAL.
                let supported = match db_path.file_name().and_then(|name| name.to_str()) {
                    Some("notebook.db") => migrations::validate_schema_version(
                        &conn,
                        migrations::DatabaseSchema::Notebook,
                    ),
                    Some("gloss.db") => {
                        migrations::validate_schema_version(&conn, migrations::DatabaseSchema::App)
                    }
                    _ => Ok(0), // queue schema belongs to the queue dependency.
                };
                if let Err(error) = supported {
                    failed += 1;
                    tracing::error!(path = %path_str, %error,
                        "Unsupported database retained without startup WAL checkpoint");
                    continue;
                }
                if let Err(error) = conn.set_db_config(
                    rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                    false,
                ) {
                    failed += 1;
                    tracing::error!(path = %path_str, %error, "Could not configure admitted startup connection");
                    continue;
                }
                // Force a truncating checkpoint — flushes WAL to main DB and
                // removes the WAL file. Safe: committed data is in the main DB.
                let checkpoint_ok = match truncating_wal_checkpoint(&conn) {
                    Ok(result) => {
                        tracing::info!(
                            path = %path_str,
                            busy = result.busy,
                            log_frames = result.log_frames,
                            checkpointed_frames = result.checkpointed_frames,
                            "WAL checkpointed on startup"
                        );
                        result.busy == 0
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path_str,
                            error = %e,
                            "WAL checkpoint failed on startup; may need manual recovery"
                        );
                        false
                    }
                };

                // Verify integrity
                if let Err(e) = verify_database_integrity(&conn) {
                    failed += 1;
                    tracing::error!(
                        path = %path_str,
                        error = %e,
                        "Database integrity check failed on startup"
                    );
                } else if checkpoint_ok {
                    verified += 1;
                } else {
                    failed += 1;
                }
            }
            Err(e) => {
                failed += 1;
                tracing::warn!(
                    path = %path_str,
                    error = %e,
                    "Could not open database for WAL recovery"
                );
            }
        }
    }

    tracing::info!(
        count = dbs.len(),
        verified,
        failed,
        "Startup WAL recovery finished"
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn on_disk_checkpoint_reports_busy_then_recovers_after_reader_release() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("checkpoint.db");
        let writer = rusqlite::Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA journal_mode=WAL; PRAGMA wal_autocheckpoint=0; CREATE TABLE proof(value TEXT); INSERT INTO proof VALUES ('first');").unwrap();
        let reader = rusqlite::Connection::open(&path).unwrap();
        reader.execute_batch("BEGIN;").unwrap();
        assert_eq!(
            reader
                .query_row("SELECT COUNT(*) FROM proof", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            1
        );
        writer
            .execute("INSERT INTO proof VALUES ('second')", [])
            .unwrap();
        writer
            .busy_timeout(std::time::Duration::from_millis(25))
            .unwrap();
        let busy = super::truncating_wal_checkpoint(&writer).unwrap();
        assert_eq!(busy.busy, 1);
        assert!(busy.log_frames > 0);
        assert!(busy.checkpointed_frames < busy.log_frames);
        reader.execute_batch("ROLLBACK").unwrap();
        drop(reader);
        let recovered = super::truncating_wal_checkpoint(&writer).unwrap();
        assert_eq!(recovered.busy, 0);
        super::verify_database_integrity(&writer).unwrap();
        assert_eq!(
            writer
                .query_row("SELECT COUNT(*) FROM proof", [], |r| r.get::<_, i64>(0))
                .unwrap(),
            2
        );
        println!("WAL_CHECKPOINT_BUSY_THEN_RELEASE {busy:?} {recovered:?}");
    }

    #[test]
    fn repeated_supported_startup_recovery_is_idempotent() {
        use super::migrations::wal_admission_fixtures::{file_state, CopiedWal};
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("gloss.db");
        let _fixture = CopiedWal::create(&path, super::migrations::DatabaseSchema::App, "4", false);
        super::recover_all_wal_on_startup(root.path());
        let recovered = file_state(&path);
        super::recover_all_wal_on_startup(root.path());
        assert_eq!(recovered, file_state(&path));
        CopiedWal::assert_readable(&path, "4");
    }

    #[test]
    fn startup_rejects_copied_committed_wal_without_changing_any_store_file() {
        use super::migrations::wal_admission_fixtures::{file_state, record_file_state, CopiedWal};
        use super::migrations::DatabaseSchema;
        for (name, version) in [
            ("gloss.db", "5"),
            ("gloss.db", "malformed"),
            ("gloss.db", "-1"),
            ("notebook.db", "8"),
            ("notebook.db", "malformed"),
            ("notebook.db", "-1"),
        ] {
            for include_shm in [true, false] {
                let root = tempfile::tempdir().unwrap();
                let path = if name == "notebook.db" {
                    let folder = root.path().join("notebooks/fixture");
                    std::fs::create_dir_all(&folder).unwrap();
                    folder.join(name)
                } else {
                    root.path().join(name)
                };
                let schema = if name == "gloss.db" {
                    DatabaseSchema::App
                } else {
                    DatabaseSchema::Notebook
                };
                let _fixture = CopiedWal::create(&path, schema, version, include_shm);
                let before = file_state(&path);
                let label = format!("startup-{name}-{version}-shm-{include_shm}");
                record_file_state(&format!("{label}-before"), &before);
                super::recover_all_wal_on_startup(root.path());
                record_file_state(&format!("{label}-after"), &file_state(&path));
                assert_eq!(
                    file_state(&path),
                    before,
                    "startup altered rejected {name} schema {version}, SHM present={include_shm}"
                );
                CopiedWal::assert_readable(&path, version);
                assert_eq!(file_state(&path), before);
            }
        }
    }

    #[test]
    fn startup_recovers_supported_committed_wal_records() {
        use super::migrations::wal_admission_fixtures::{file_state, CopiedWal};
        use super::migrations::DatabaseSchema;
        for (name, version) in [("gloss.db", "4"), ("notebook.db", "7")] {
            for include_shm in [true, false] {
                let root = tempfile::tempdir().unwrap();
                let path = if name == "notebook.db" {
                    let folder = root.path().join("notebooks/fixture");
                    std::fs::create_dir_all(&folder).unwrap();
                    folder.join(name)
                } else {
                    root.path().join(name)
                };
                let schema = if name == "gloss.db" {
                    DatabaseSchema::App
                } else {
                    DatabaseSchema::Notebook
                };
                let _fixture = CopiedWal::create(&path, schema, version, include_shm);
                let before = file_state(&path);
                super::recover_all_wal_on_startup(root.path());
                assert_ne!(
                    file_state(&path)[0],
                    before[0],
                    "startup did not checkpoint supported {name}"
                );
                CopiedWal::assert_readable(&path, version);
                let connection = if name == "gloss.db" {
                    super::migrations::open_connection(&path, DatabaseSchema::App).unwrap()
                } else {
                    super::migrations::open_connection(&path, DatabaseSchema::Notebook).unwrap()
                };
                connection
                    .execute(
                        "INSERT INTO wal_retained VALUES ('startup owner remains writable')",
                        [],
                    )
                    .unwrap();
                drop(connection);
                let reopened = if name == "gloss.db" {
                    super::migrations::open_connection(&path, DatabaseSchema::App).unwrap()
                } else {
                    super::migrations::open_connection(&path, DatabaseSchema::Notebook).unwrap()
                };
                assert_eq!(
                    reopened
                        .query_row("SELECT COUNT(*) FROM wal_retained", [], |r| r
                            .get::<_, i64>(0))
                        .unwrap(),
                    2
                );
            }
        }
    }

    #[test]
    fn startup_recovery_does_not_checkpoint_unsupported_or_malformed_stores() {
        for (name, version) in [
            ("gloss.db", "5"),
            ("notebook.db", "8"),
            ("notebook.db", "malformed"),
        ] {
            let root = tempfile::tempdir().unwrap();
            let path = if name == "notebook.db" {
                let folder = root.path().join("notebooks/fixture");
                std::fs::create_dir_all(&folder).unwrap();
                folder.join(name)
            } else {
                root.path().join(name)
            };
            let connection = rusqlite::Connection::open(&path).unwrap();
            connection.execute_batch("PRAGMA journal_mode=DELETE; CREATE TABLE _meta(key TEXT PRIMARY KEY, value TEXT NOT NULL); CREATE TABLE retained(value TEXT);").unwrap();
            connection
                .execute("INSERT INTO _meta VALUES ('schema_version', ?1)", [version])
                .unwrap();
            drop(connection);
            let before = std::fs::read(&path).unwrap();
            super::recover_all_wal_on_startup(root.path());
            assert_eq!(std::fs::read(&path).unwrap(), before, "{name} {version}");
            assert!(!std::path::PathBuf::from(format!("{}-wal", path.display())).exists());
            assert!(!std::path::PathBuf::from(format!("{}-shm", path.display())).exists());
        }
    }

    #[test]
    fn integrity_accepts_only_a_complete_ok_result() {
        let connection = rusqlite::Connection::open_in_memory().unwrap();
        connection
            .execute_batch("CREATE TABLE proof (value INTEGER CHECK(value > 0));")
            .unwrap();
        super::verify_database_integrity(&connection).unwrap();
        connection.execute_batch("PRAGMA ignore_check_constraints=ON; INSERT INTO proof VALUES (-1); PRAGMA ignore_check_constraints=OFF;").unwrap();
        let diagnosis: String = connection
            .pragma_query_value(None, "integrity_check", |row| row.get(0))
            .unwrap();
        assert_ne!(diagnosis, "ok");
        let error = super::verify_database_integrity(&connection).unwrap_err();
        assert!(error.to_string().contains("CHECK constraint failed"));
        assert_eq!(
            connection
                .query_row("SELECT value FROM proof", [], |row| row.get::<_, i64>(0))
                .unwrap(),
            -1
        );
    }

    #[test]
    fn truncating_wal_checkpoint_reads_sqlite_integer_result_columns() {
        let connection = rusqlite::Connection::open_in_memory().expect("in-memory database");
        connection
            .execute_batch("PRAGMA journal_mode=WAL; CREATE TABLE proof (id INTEGER PRIMARY KEY);")
            .expect("WAL-backed fixture");

        let result = super::truncating_wal_checkpoint(&connection)
            .expect("SQLite WAL checkpoint result must decode its integer columns");

        assert_eq!(result.busy, 0);
        // SQLite returns -1 for frame counts when an in-memory fixture cannot
        // enter WAL mode; on-disk WAL databases return non-negative counts.
        assert!(result.log_frames >= -1);
        assert!(result.checkpointed_frames >= -1);
    }
}
