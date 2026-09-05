//! Connection pool for per-notebook SQLite databases.
//!
//! Each notebook gets its own `.db` file.  Because SQLite only supports one
//! writer at a time we maintain:
//! - a single **write** connection guarded by a `Mutex`, and
//! - a small pool of **read** connections (up to `MAX_READ_CONNS`) that can be
//!   checked out concurrently.
//!
//! WAL mode (set by `migrations::apply_pragmas`) allows readers and the single
//! writer to operate concurrently without blocking each other.  The
//! `busy_timeout=5000` pragma ensures that a writer that cannot acquire the
//! database lock immediately will retry for up to 5 s rather than failing
//! right away.

use crate::db::migrations;
use crate::db::notebook_db::NotebookDb;
use crate::error::GlossError;
use log::{info, warn};
use rusqlite::Connection;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

/// Maximum number of read connections the pool will create for a single
/// notebook database.  Configurable at pool construction time.
pub const DEFAULT_MAX_READ_CONNS: usize = 4;

// ---------------------------------------------------------------------------
// Pool
// ---------------------------------------------------------------------------

/// A lightweight connection pool for a single notebook database file.
///
/// The pool provides `read()` and `write()` convenience methods that acquire
/// a connection, wrap it as a `NotebookDb`, invoke a caller-supplied closure,
/// and return the connection to the pool.  This is the recommended API.
///
/// Write callbacks retain the exclusive mutex guard for their entire lifetime.
#[allow(dead_code)]
pub struct NotebookDbPool {
    db_path: PathBuf,
    max_read_conns: usize,
    /// The single write connection.  Guarded by a `Mutex` so only one writer
    /// at a time — exactly what SQLite needs.
    write_conn: Mutex<Connection>,
    /// Pool of read connections.  When a reader is done it pushes the
    /// connection back; when the pool is empty we open a new one up to
    /// `max_read_conns`.  Connections opened past that limit are opened and
    /// closed per call (fallback behaviour).
    read_conns: Mutex<Vec<Connection>>,
    /// Tracks how many read connections have been created in total so we
    /// stay under `max_read_conns` for the cached pool.
    read_conn_count: Mutex<usize>,
}

impl NotebookDbPool {
    /// Create a new pool for the database at `db_path`.
    ///
    /// Opens the write connection immediately (running migrations if this is a
    /// fresh database) and leaves the read pool empty (read connections are
    /// opened lazily on first `acquire_read`).
    pub fn new(db_path: &Path) -> Result<Self, GlossError> {
        info!("[notebook-pool] creating pool for {:?}", db_path);
        let pool = Self::with_max_read_conns(db_path, DEFAULT_MAX_READ_CONNS)?;
        info!(
            "[notebook-pool] pool ready for {:?} (max_read_conns={})",
            db_path, DEFAULT_MAX_READ_CONNS
        );
        Ok(pool)
    }

    /// Create a pool with a custom maximum read-connection count.
    pub fn with_max_read_conns(db_path: &Path, max_read_conns: usize) -> Result<Self, GlossError> {
        // The write connection is also the one that runs migrations on first
        // creation so the DB schema is guaranteed to exist before any reader
        // tries to query it.
        let write_conn = Connection::open(db_path)?;
        migrations::apply_pragmas(&write_conn)?;
        migrations::migrate_notebook_db(&write_conn)?;

        Ok(Self {
            db_path: db_path.to_path_buf(),
            max_read_conns: max_read_conns.max(1),
            write_conn: Mutex::new(write_conn),
            read_conns: Mutex::new(Vec::new()),
            read_conn_count: Mutex::new(0),
        })
    }

    /// Execute a read-only closure against a pooled read connection.
    ///
    /// This is the most convenient API for callers that just need to query data.
    /// The connection is automatically returned to the pool when the closure
    /// returns.
    pub fn read<F, T>(&self, f: F) -> Result<T, GlossError>
    where
        F: FnOnce(&NotebookDb) -> Result<T, GlossError>,
    {
        info!("[notebook-pool] read acquire for {:?}", self.db_path);
        let (conn, is_one_shot) = self.take_read_conn()?;
        let db = NotebookDb::from_conn_ref(&conn);
        let result = f(db);
        self.return_read_conn(conn, is_one_shot);
        info!("[notebook-pool] read release for {:?}", self.db_path);
        result
    }

    /// Execute a write closure against the exclusive write connection.
    ///
    /// Only one writer may hold the connection at a time.  The connection is
    /// released when the closure returns.
    ///
    /// The original connection stays behind the guard throughout the callback
    /// and transaction cleanup. Contending writers wait on that same guard.
    /// Callback errors and panics roll back any unfinished transaction before
    /// another caller may use the connection. Already committed changes are
    /// owned by the callback and are not undone by this cleanup.
    pub fn write<F, T>(&self, f: F) -> Result<T, GlossError>
    where
        F: FnOnce(&NotebookDb) -> Result<T, GlossError>,
    {
        info!("[notebook-pool] write acquire for {:?}", self.db_path);
        let conn = self.take_write_conn()?;
        if !conn.is_autocommit() {
            return Err(GlossError::Other(
                "notebook writer is blocked by an unfinished transaction after failed cleanup"
                    .into(),
            ));
        }
        let db = NotebookDb::from_conn_ref(&conn);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(db)));
        let unfinished_transaction = !conn.is_autocommit();
        if unfinished_transaction {
            conn.execute_batch("ROLLBACK").map_err(|error| {
                GlossError::Other(format!(
                    "notebook writer transaction cleanup failed: {error}"
                ))
            })?;
        }
        info!("[notebook-pool] write release for {:?}", self.db_path);
        match result {
            Ok(Ok(_)) if unfinished_transaction => Err(GlossError::Other(
                "notebook write left an uncommitted transaction and was rolled back".into(),
            )),
            Ok(inner) => inner,
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<&str>()
                    .map(|s| s.to_string())
                    .or_else(|| payload.downcast_ref::<String>().cloned())
                    .unwrap_or_else(|| "<non-string panic>".to_string());
                warn!(
                    "[notebook-pool] write closure panicked for {:?}; transaction cleanup completed (panic: {})",
                    self.db_path, msg
                );
                Err(GlossError::Other(format!(
                    "notebook write closure panicked: {msg}"
                )))
            }
        }
    }

    /// Return the database path this pool manages.
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    // -- internal helpers ---------------------------------------------------

    /// Take a read connection from the pool (or open a new one).
    ///
    /// Returns a tuple `(Connection, is_one_shot)`. When `is_one_shot` is
    /// `true`, the connection was opened past the `max_read_conns` budget
    /// and must be DROPPED (not cached) by `return_read_conn` to avoid
    /// growing the pool beyond the configured cap.
    fn take_read_conn(&self) -> Result<(Connection, bool), GlossError> {
        {
            let mut pool = self
                .read_conns
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if let Some(conn) = pool.pop() {
                info!(
                    "[notebook-pool] reusing cached read conn for {:?}",
                    self.db_path
                );
                return Ok((conn, false));
            }
        }
        // No cached connection available — try to open a new one.
        let mut count = self
            .read_conn_count
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        if *count < self.max_read_conns {
            let conn = self.open_read_conn()?;
            *count += 1;
            info!(
                "[notebook-pool] opened new read conn #{} for {:?}",
                count, self.db_path
            );
            Ok((conn, false))
        } else {
            // Over budget — open a one-shot connection that will be dropped
            // when returned. We deliberately do NOT increment read_conn_count
            // so the budget check stays accurate.
            warn!(
                "[notebook-pool] read pool at capacity ({max}), opening one-shot conn for {:?}",
                self.db_path,
                max = self.max_read_conns
            );
            self.open_read_conn().map(|c| (c, true))
        }
    }

    /// Return a read connection to the pool.
    ///
    /// One-shot connections (past the budget) are NOT cached: they are dropped
    /// here so the pool stays bounded.
    fn return_read_conn(&self, conn: Connection, is_one_shot: bool) {
        if is_one_shot {
            // Drop the connection — don't touch the cached pool. The
            // Connection's Drop impl closes the SQLite handle.
            return;
        }
        let mut pool = self.read_conns.lock().unwrap_or_else(|e| e.into_inner());
        // Defensive: even for cached returns, don't let the pool grow past
        // the configured cap. (This should not happen given take_read_conn's
        // budget check, but it protects against any other path that calls
        // this function.)
        if pool.len() >= self.max_read_conns {
            return;
        }
        pool.push(conn);
    }

    /// Take the write connection (blocks until available).
    fn take_write_conn(&self) -> Result<MutexGuard<'_, Connection>, GlossError> {
        info!(
            "[notebook-pool] acquiring write conn for {:?}",
            self.db_path
        );
        self.write_conn
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))
    }

    fn open_read_conn(&self) -> Result<Connection, GlossError> {
        let conn = Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .or_else(|_| {
            // The database file might not exist yet when called from a test or
            // a fresh notebook that hasn't been written to.  Fall back to a
            // read-write open so WAL can be set up.
            Connection::open(&self.db_path)
        })?;
        migrations::apply_pragmas(&conn)?;
        Ok(conn)
    }
}

// ---------------------------------------------------------------------------
// Multi-notebook pool registry
// ---------------------------------------------------------------------------

/// A registry that holds one `NotebookDbPool` per notebook ID.
///
/// This replaces the previous `notebook_dbs: Mutex<HashMap<String, PathBuf>>`
/// in `AppState`.  The pools are wrapped in `Arc` so we can hand out cloned
/// references without holding the registry lock for the entire operation.
pub struct NotebookDbPools {
    pools: Mutex<HashMap<String, Arc<NotebookDbPool>>>,
    #[allow(dead_code)]
    data_dir: PathBuf,
}

impl NotebookDbPools {
    pub fn new(data_dir: &Path) -> Self {
        Self {
            pools: Mutex::new(HashMap::new()),
            data_dir: data_dir.to_path_buf(),
        }
    }

    /// Get or create a pool for the given notebook.
    ///
    /// Accepts a closure `resolve_path` that returns the DB path by looking up
    /// the notebook metadata (typically from `app_db`).  The closure is only
    /// invoked when the pool has not been created yet.
    pub fn get_or_create<F>(
        &self,
        notebook_id: &str,
        resolve_path: F,
    ) -> Result<Arc<NotebookDbPool>, GlossError>
    where
        F: FnOnce() -> Result<PathBuf, GlossError>,
    {
        {
            let pools = self
                .pools
                .lock()
                .map_err(|e| GlossError::Other(e.to_string()))?;
            if let Some(pool) = pools.get(notebook_id) {
                return Ok(Arc::clone(pool));
            }
        }

        // Not found — resolve the path and create the pool outside the lock
        // to avoid holding it while doing I/O.
        let db_path = resolve_path()?;
        let pool = Arc::new(NotebookDbPool::new(&db_path)?);

        let mut pools = self
            .pools
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;

        // Another thread might have inserted between our first check and now.
        let canonical_pool = pools.entry(notebook_id.to_string()).or_insert(pool);
        Ok(Arc::clone(canonical_pool))
    }

    /// Remove a pool from the registry (e.g. when a notebook is deleted).
    pub fn remove(&self, notebook_id: &str) {
        let mut pools = self.pools.lock().unwrap_or_else(|e| e.into_inner());
        pools.remove(notebook_id);
    }

    /// Check whether a pool exists for the given notebook.
    #[allow(dead_code)]
    pub fn contains(&self, notebook_id: &str) -> bool {
        let pools = self.pools.lock().unwrap_or_else(|e| e.into_inner());
        pools.contains_key(notebook_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tempfile::tempdir;

    /// B-1 regression: opening more than `max_read_conns` concurrent read
    /// closures must NOT cause the cached pool to grow past the cap. Each
    /// over-budget connection should be opened and dropped, leaving the
    /// pool at exactly `max_read_conns` entries.
    #[test]
    fn read_pool_does_not_grow_past_max_read_conns_under_burst() {
        let dir = tempdir().unwrap();
        let pool =
            Arc::new(NotebookDbPool::with_max_read_conns(&dir.path().join("nb.db"), 2).unwrap());
        // Issue 10 concurrent reads, way past the cap of 2. Each one must
        // succeed (using a one-shot conn) and the pool must stay bounded.
        let mut handles = Vec::new();
        for i in 0..10 {
            let pool = Arc::clone(&pool);
            handles.push(std::thread::spawn(move || {
                pool.read(|_db| Ok::<_, GlossError>(i)).unwrap();
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        let cached = pool.read_conns.lock().unwrap();
        assert!(
            cached.len() <= 2,
            "read pool grew past cap: {} entries (max=2)",
            cached.len()
        );
    }

    /// A write closure that panics must NOT strand the connection: the write
    /// method's `catch_unwind` returns the connection to the pool, so the
    /// next write succeeds.  This is the regression that previously produced
    /// "write connection not available" on every retry after a single panic.
    #[test]
    fn write_closure_panic_does_not_strand_connection() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("test_nb.db");
        let pool = NotebookDbPool::new(&db_path).unwrap();

        // First closure panics inside the write — it is caught.
        let res: Result<(), GlossError> = pool.write(|_db| {
            panic!("simulated ingestion panic (e.g. blocking reqwest)");
        });
        assert!(res.is_err());
        let err_str = format!("{}", res.unwrap_err());
        assert!(
            err_str.contains("panic"),
            "expected panic in error, got: {err_str}"
        );

        // The write connection must still be available for a second write.
        let ok: i64 = pool
            .write(|db| Ok(db.conn().query_row("SELECT 42", [], |row| row.get(0))?))
            .unwrap();
        assert_eq!(ok, 42);
    }

    #[test]
    fn concurrent_writer_waits_for_active_write_and_then_succeeds() {
        use std::sync::mpsc;
        use std::time::Duration;
        let dir = tempdir().unwrap();
        let pool = Arc::new(NotebookDbPool::new(&dir.path().join("writers.db")).unwrap());
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let first_pool = Arc::clone(&pool);
        let first = std::thread::spawn(move || {
            first_pool.write(|db| {
                db.conn().execute(
                    "INSERT INTO _meta(key, value) VALUES('first', 'committed')",
                    [],
                )?;
                entered_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
        });
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (attempt_tx, attempt_rx) = mpsc::channel();
        let (result_tx, result_rx) = mpsc::channel();
        let second_pool = Arc::clone(&pool);
        let second = std::thread::spawn(move || {
            attempt_tx.send(()).unwrap();
            let result = second_pool.write(|db| {
                db.conn().execute(
                    "INSERT INTO _meta(key, value) VALUES('second', 'committed')",
                    [],
                )?;
                Ok(())
            });
            result_tx
                .send(result.map_err(|error| error.to_string()))
                .unwrap();
        });
        attempt_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let early = result_rx.recv_timeout(Duration::from_millis(100));
        let returned_while_first_owned_connection = early.is_ok();
        release_tx.send(()).unwrap();
        first.join().unwrap().unwrap();
        let result =
            early.unwrap_or_else(|_| result_rx.recv_timeout(Duration::from_secs(5)).unwrap());
        second.join().unwrap();
        assert!(
            !returned_while_first_owned_connection,
            "second writer returned before first released its connection: {result:?}"
        );
        result.unwrap();
        let count: i64 = pool
            .read(|db| {
                Ok(db.conn().query_row(
                    "SELECT COUNT(*) FROM _meta WHERE key IN ('first', 'second')",
                    [],
                    |row| row.get(0),
                )?)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn panic_rolls_back_uncommitted_work_before_reusing_writer() {
        let dir = tempdir().unwrap();
        let pool = NotebookDbPool::new(&dir.path().join("panic.db")).unwrap();
        let failed: Result<(), GlossError> = pool.write(|db| {
            db.conn().execute_batch(
                "BEGIN IMMEDIATE; INSERT INTO _meta(key,value) VALUES('uncommitted','value')",
            )?;
            panic!("injected writer panic");
        });
        assert!(failed.is_err());
        pool.write(|db| {
            assert!(
                db.conn().is_autocommit(),
                "prior callback left its transaction attached to the shared writer"
            );
            let count: i64 = db.conn().query_row(
                "SELECT COUNT(*) FROM _meta WHERE key='uncommitted'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
            db.conn().execute(
                "INSERT INTO _meta(key,value) VALUES('recovered','value')",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn error_rolls_back_uncommitted_work_and_preserves_original_error() {
        let dir = tempdir().unwrap();
        let pool = NotebookDbPool::new(&dir.path().join("error.db")).unwrap();
        let failed: Result<(), GlossError> = pool.write(|db| {
            db.conn().execute_batch(
                "BEGIN IMMEDIATE; INSERT INTO _meta(key,value) VALUES('uncommitted','value')",
            )?;
            Err(GlossError::Config("injected callback failure".into()))
        });
        assert!(
            matches!(failed, Err(GlossError::Config(message)) if message == "injected callback failure")
        );
        pool.write(|db| {
            assert!(db.conn().is_autocommit());
            let count: i64 = db.conn().query_row(
                "SELECT COUNT(*) FROM _meta WHERE key='uncommitted'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn successful_callback_cannot_leave_a_transaction_for_the_next_writer() {
        let dir = tempdir().unwrap();
        let pool = NotebookDbPool::new(&dir.path().join("uncommitted.db")).unwrap();
        let failed = pool.write(|db| {
            db.conn().execute_batch(
                "BEGIN IMMEDIATE; INSERT INTO _meta(key,value) VALUES('uncommitted','value')",
            )?;
            Ok(())
        });
        assert!(failed.unwrap_err().to_string().contains("rolled back"));
        pool.write(|db| {
            assert!(db.conn().is_autocommit());
            let count: i64 = db.conn().query_row(
                "SELECT COUNT(*) FROM _meta WHERE key='uncommitted'",
                [],
                |row| row.get(0),
            )?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn simultaneous_first_access_returns_the_same_registered_pool() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("canonical.db");
        drop(NotebookDbPool::new(&db_path).unwrap());
        let pools = Arc::new(NotebookDbPools::new(dir.path()));
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let mut handles = Vec::new();
        for _ in 0..2 {
            let pools = Arc::clone(&pools);
            let barrier = Arc::clone(&barrier);
            let path = db_path.clone();
            handles.push(std::thread::spawn(move || {
                pools
                    .get_or_create("nb", || {
                        barrier.wait();
                        Ok(path)
                    })
                    .unwrap()
            }));
        }
        let first = handles.remove(0).join().unwrap();
        let second = handles.remove(0).join().unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "racing first access returned a noncanonical writer pool"
        );
        let canonical = pools
            .get_or_create("nb", || {
                panic!("existing pool must not resolve a path again")
            })
            .unwrap();
        assert!(Arc::ptr_eq(&first, &canonical));
    }
}
