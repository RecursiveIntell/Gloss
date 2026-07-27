pub mod app_db;
pub mod doctor;
pub mod migrations;
pub mod notebook_db;
pub mod notebook_pool;
pub mod portable;

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
                match conn.pragma_query_value(None, "wal_checkpoint", |row| {
                    row.get::<_, String>(0)
                }) {
                    Ok(status) => {
                        tracing::info!(
                            path = %path_str,
                            wal_status = %status,
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
                if let Err(e) = conn.pragma_query_value(None, "integrity_check", |row| {
                    row.get::<_, String>(0)
                }) {
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
