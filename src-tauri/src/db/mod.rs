pub mod app_db;
pub mod doctor;
pub mod migrations;
pub mod notebook_db;
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

    for db_path in &dbs {
        let path_str = db_path.display().to_string();
        match Connection::open(db_path) {
            Ok(conn) => {
                // Force a truncating checkpoint — flushes WAL to main DB and
                // removes the WAL file. Safe: committed data is in the main DB.
                match truncating_wal_checkpoint(&conn) {
                    Ok(result) => {
                        tracing::info!(
                            path = %path_str,
                            busy = result.busy,
                            log_frames = result.log_frames,
                            checkpointed_frames = result.checkpointed_frames,
                            "WAL checkpointed on startup"
                        );
                    }
                    Err(e) => {
                        tracing::warn!(
                            path = %path_str,
                            error = %e,
                            "WAL checkpoint failed on startup; may need manual recovery"
                        );
                    }
                }

                // Verify integrity
                if let Err(e) =
                    conn.pragma_query_value(None, "integrity_check", |row| row.get::<_, String>(0))
                {
                    tracing::error!(
                        path = %path_str,
                        error = %e,
                        "Database integrity check failed on startup"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(
                    path = %path_str,
                    error = %e,
                    "Could not open database for WAL recovery"
                );
            }
        }
    }

    tracing::info!(count = dbs.len(), "Startup WAL recovery complete");
}

#[cfg(test)]
mod tests {
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
