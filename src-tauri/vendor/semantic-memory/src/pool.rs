//! Small SQLite connection pool tailored to rusqlite.
//!
//! SQLite still serializes writers at the database level, so the pool keeps a
//! single writer connection plus a bounded set of independent reader
//! connections. Under WAL mode, those readers can proceed concurrently while
//! the writer is idle or between write transactions.

use crate::config::{MemoryLimits, PoolConfig};
use crate::db;
use crate::error::MemoryError;
use rusqlite::Connection;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Default reader acquisition timeout (30 seconds).
const DEFAULT_READER_TIMEOUT: Duration = Duration::from_secs(30);

/// Internal SQLite pool with one writer connection and N reader connections.
pub(crate) struct SqlitePool {
    writer: Mutex<Connection>,
    readers: Vec<Mutex<Connection>>,
    available_readers: Mutex<Vec<usize>>,
    available_cv: Condvar,
    reader_count: usize,
    reader_timeout: Duration,
    health: PoolHealth,
}

#[derive(Debug, Default)]
struct PoolHealth {
    reader_timeouts: AtomicU64,
    writer_poison_recoveries: AtomicU64,
    reader_poison_recoveries: AtomicU64,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct PoolHealthSnapshot {
    pub reader_timeouts: u64,
    pub writer_poison_recoveries: u64,
    pub reader_poison_recoveries: u64,
}

impl PoolHealth {
    #[allow(dead_code)]
    fn snapshot(&self) -> PoolHealthSnapshot {
        PoolHealthSnapshot {
            reader_timeouts: self.reader_timeouts.load(Ordering::SeqCst),
            writer_poison_recoveries: self.writer_poison_recoveries.load(Ordering::SeqCst),
            reader_poison_recoveries: self.reader_poison_recoveries.load(Ordering::SeqCst),
        }
    }
}

/// RAII guard that returns a reader index to the pool on drop.
///
/// This ensures the reader slot is always returned even if the closure panics.
struct ReaderGuard<'a> {
    pool: &'a SqlitePool,
    idx: Option<usize>,
}

impl<'a> ReaderGuard<'a> {
    fn new(pool: &'a SqlitePool, idx: usize) -> Self {
        Self {
            pool,
            idx: Some(idx),
        }
    }
}

impl Drop for ReaderGuard<'_> {
    fn drop(&mut self) {
        if let Some(idx) = self.idx.take() {
            let mut available = self
                .pool
                .available_readers
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            available.push(idx);
            self.pool.available_cv.notify_one();
        }
    }
}

#[cfg(test)]
#[allow(clippy::items_after_test_module)]
mod tests {
    use super::*;
    use crate::config::{MemoryLimits, PoolConfig};
    use std::panic::{self, AssertUnwindSafe};
    use std::sync::mpsc;
    use std::time::Duration;
    use tempfile::TempDir;

    #[test]
    fn writer_mutex_poison_recovery_rolls_back_open_txn() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("poison-mutex.db");
        let pool =
            SqlitePool::open(&db_path, &PoolConfig::default(), &MemoryLimits::default()).unwrap();

        let panic_result: Result<Result<(), MemoryError>, Box<dyn std::any::Any + Send>> =
            panic::catch_unwind(AssertUnwindSafe(|| {
                pool.with_write_conn(|conn| {
                    conn.execute_batch(
                        "BEGIN IMMEDIATE;
                     CREATE TABLE IF NOT EXISTS poison_sync (value INTEGER);
                     INSERT INTO poison_sync (value) VALUES (7);",
                    )?;
                    panic!("simulated panic during write");
                })
            }));
        assert!(
            panic_result.is_err(),
            "simulated panic should propagate as panic"
        );
        assert_eq!(pool.health_snapshot().writer_poison_recoveries, 0);

        let table_exists_after_recovery = pool
            .with_write_conn(|conn| {
                let count: i64 = conn.query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='poison_sync'",
                    [],
                    |row| row.get(0),
                )?;
                Ok(count)
            })
            .unwrap();
        assert_eq!(
            table_exists_after_recovery, 0,
            "poisoned mutex recovery should rollback speculative session writes"
        );

        let healthy = pool.with_write_conn(|conn| {
            conn.execute_batch("CREATE TABLE safe_sync(value INTEGER)")?;
            Ok(())
        });
        assert!(
            healthy.is_ok(),
            "writer connection should be usable after recovery"
        );
        assert!(
            pool.health_snapshot().writer_poison_recoveries >= 1,
            "poisoned mutex remains marked by std::sync; every later lock records recovery"
        );
    }

    #[test]
    fn reader_timeout_is_typed_and_recorded_in_pool_health() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("reader-timeout.db");
        let pool_config = PoolConfig {
            max_read_connections: 1,
            reader_timeout_secs: 1,
            ..PoolConfig::default()
        };
        let pool = SqlitePool::open(&db_path, &pool_config, &MemoryLimits::default()).unwrap();
        let (tx, rx) = mpsc::channel();

        std::thread::scope(|scope| {
            scope.spawn(|| {
                pool.with_read_conn(|_| {
                    tx.send(()).unwrap();
                    std::thread::sleep(Duration::from_millis(1500));
                    Ok(())
                })
                .unwrap();
            });
            rx.recv_timeout(Duration::from_secs(2)).unwrap();
            let error = pool.with_read_conn(|_| Ok(())).unwrap_err();
            match error {
                MemoryError::PoolTimeout {
                    elapsed_ms,
                    pool_size,
                } => {
                    assert!(elapsed_ms >= 900);
                    assert_eq!(pool_size, 1);
                }
                other => panic!("expected pool timeout, got {other:?}"),
            }
        });

        assert_eq!(pool.health_snapshot().reader_timeouts, 1);
    }
}

impl SqlitePool {
    /// Open a writer connection, run migrations once, and populate the reader pool.
    pub(crate) fn open(
        path: &Path,
        pool: &PoolConfig,
        limits: &MemoryLimits,
    ) -> Result<Self, MemoryError> {
        let writer = db::open_database(path, pool, limits)?;
        let mut readers = Vec::with_capacity(pool.max_read_connections);
        for _ in 0..pool.max_read_connections {
            readers.push(Mutex::new(db::open_pool_member_connection(
                path, pool, limits, true,
            )?));
        }

        let reader_count = pool.max_read_connections;
        let reader_timeout = if pool.reader_timeout_secs > 0 {
            Duration::from_secs(pool.reader_timeout_secs)
        } else {
            DEFAULT_READER_TIMEOUT
        };
        Ok(Self {
            writer: Mutex::new(writer),
            readers,
            available_readers: Mutex::new((0..reader_count).rev().collect()),
            available_cv: Condvar::new(),
            reader_count,
            reader_timeout,
            health: PoolHealth::default(),
        })
    }

    #[allow(dead_code)]
    pub(crate) fn health_snapshot(&self) -> PoolHealthSnapshot {
        self.health.snapshot()
    }

    /// Run work against the single writer connection.
    pub(crate) fn with_write_conn<F, T>(&self, f: F) -> Result<T, MemoryError>
    where
        F: FnOnce(&Connection) -> Result<T, MemoryError>,
    {
        let conn = match self.writer.lock() {
            Ok(conn) => conn,
            Err(err) => {
                let conn = err.into_inner();
                self.health
                    .writer_poison_recoveries
                    .fetch_add(1, Ordering::SeqCst);
                tracing::warn!("Writer lock was poisoned; entering recovery path");
                if !conn.is_autocommit() {
                    if let Err(rollback_err) = conn.execute("ROLLBACK", []) {
                        tracing::error!(
                            rollback_error = rollback_err.to_string(),
                            "Writer connection was poisoned and rollback during recovery failed"
                        );
                    } else {
                        tracing::warn!("Rolled back writer connection transaction after poisoned mutex recovery");
                    }
                } else {
                    tracing::warn!(
                        "Writer connection recovered from poison while already in autocommit mode"
                    );
                }
                conn
            }
        };
        f(&conn)
    }

    /// Run work against one reader connection with bounded wait time.
    ///
    /// Reader slots are returned via an RAII guard, so even if `f` panics
    /// the slot is returned to the available pool (preventing permanent leaks).
    pub(crate) fn with_read_conn<F, T>(&self, f: F) -> Result<T, MemoryError>
    where
        F: FnOnce(&Connection) -> Result<T, MemoryError>,
    {
        let start = Instant::now();
        let reader_idx = {
            let mut available = self
                .available_readers
                .lock()
                .unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(idx) = available.pop() {
                    break idx;
                }
                let (new_available, wait_result) = self
                    .available_cv
                    .wait_timeout(available, self.reader_timeout)
                    .unwrap_or_else(|e| e.into_inner());
                available = new_available;
                if wait_result.timed_out() {
                    if let Some(idx) = available.pop() {
                        break idx;
                    }
                    let elapsed = start.elapsed();
                    tracing::error!(
                        elapsed_ms = elapsed.as_millis() as u64,
                        pool_size = self.reader_count,
                        "Reader pool acquisition timed out"
                    );
                    self.health.reader_timeouts.fetch_add(1, Ordering::SeqCst);
                    return Err(MemoryError::PoolTimeout {
                        elapsed_ms: elapsed.as_millis() as u64,
                        pool_size: self.reader_count,
                    });
                }
            }
        };

        let wait_duration = start.elapsed();
        if wait_duration > Duration::from_millis(100) {
            tracing::warn!(
                wait_ms = wait_duration.as_millis() as u64,
                pool_size = self.reader_count,
                "Reader pool contention: waited {}ms for a reader slot",
                wait_duration.as_millis()
            );
        }

        // RAII guard ensures the reader index is returned even on panic
        let _guard = ReaderGuard::new(self, reader_idx);
        let conn = self.readers[reader_idx].lock().unwrap_or_else(|e| {
            self.health
                .reader_poison_recoveries
                .fetch_add(1, Ordering::SeqCst);
            tracing::warn!(
                reader_idx,
                "Reader lock was poisoned; recovering connection and marking pool health"
            );
            e.into_inner()
        });
        f(&conn)
    }
}
