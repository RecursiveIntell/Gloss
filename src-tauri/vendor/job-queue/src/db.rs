use crate::types::{FailureClass, QueueJobDetails, QueueJobStatus, QueuePriority, QueueStats};
use anyhow::{Context, Result};
use rusqlite::{params, Connection};
use serde_json::Value;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS queue_jobs (
    id                  TEXT PRIMARY KEY,
    priority            INTEGER DEFAULT 2,
    status              TEXT CHECK(status IN ('pending', 'processing', 'completed', 'failed', 'cancelled')),
    data_json           TEXT NOT NULL,
    trace_id            TEXT,
    created_at          DATETIME DEFAULT CURRENT_TIMESTAMP,
    started_at          DATETIME,
    completed_at        DATETIME,
    error_message       TEXT,
    worker_id           TEXT,
    heartbeat_at        DATETIME,
    visibility_timeout  INTEGER DEFAULT 300,
    failure_class       TEXT,
    next_run_at         DATETIME,
    attempt_count       INTEGER DEFAULT 0,
    attempt_id          TEXT,
    trial_id            TEXT
);

CREATE INDEX IF NOT EXISTS idx_queue_status_priority ON queue_jobs(status, priority);
"#;

/// Current schema version (bumped with each migration).
const SCHEMA_VERSION: u32 = 4;

/// Open (or create) the queue database. Pass `None` for an in-memory database.
pub fn open_database(path: Option<&std::path::Path>) -> Result<Connection> {
    let conn = match path {
        Some(p) => Connection::open(p).context("Failed to open queue database")?,
        None => Connection::open_in_memory().context("Failed to open in-memory database")?,
    };

    conn.execute_batch(
        "PRAGMA journal_mode = WAL;
         PRAGMA foreign_keys = ON;
         PRAGMA busy_timeout = 5000;",
    )
    .context("Failed to set PRAGMA options")?;

    conn.execute_batch(SCHEMA)
        .context("Failed to create queue schema")?;

    run_migrations(&conn)?;

    Ok(conn)
}

/// Run schema migrations. Idempotent — safe to call multiple times.
fn run_migrations(conn: &Connection) -> Result<()> {
    let version: u32 = conn
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap_or(0);

    if version < 2 {
        // V2 adds lifecycle columns. ALTER TABLE ADD COLUMN is a no-op if the
        // column already exists (which it will for fresh databases using the
        // updated CREATE TABLE). We catch "duplicate column" errors silently.
        let columns = [
            "ALTER TABLE queue_jobs ADD COLUMN worker_id TEXT",
            "ALTER TABLE queue_jobs ADD COLUMN heartbeat_at DATETIME",
            "ALTER TABLE queue_jobs ADD COLUMN visibility_timeout INTEGER DEFAULT 300",
            "ALTER TABLE queue_jobs ADD COLUMN failure_class TEXT",
            "ALTER TABLE queue_jobs ADD COLUMN next_run_at DATETIME",
            "ALTER TABLE queue_jobs ADD COLUMN attempt_count INTEGER DEFAULT 0",
        ];
        for sql in &columns {
            match conn.execute_batch(sql) {
                Ok(_) => {}
                Err(e) if e.to_string().contains("duplicate column") => {}
                Err(e) => return Err(e).context("Migration V2 failed"),
            }
        }
    }

    if version < 3 {
        match conn.execute_batch("ALTER TABLE queue_jobs ADD COLUMN trace_id TEXT") {
            Ok(_) => {}
            Err(e) if e.to_string().contains("duplicate column") => {}
            Err(e) => return Err(e).context("Migration V3 failed"),
        }
    }

    if version < 4 {
        // V4: canonical retry lineage columns
        let columns = [
            "ALTER TABLE queue_jobs ADD COLUMN attempt_id TEXT",
            "ALTER TABLE queue_jobs ADD COLUMN trial_id TEXT",
        ];
        for sql in &columns {
            match conn.execute_batch(sql) {
                Ok(_) => {}
                Err(e) if e.to_string().contains("duplicate column") => {}
                Err(e) => return Err(e).context("Migration V4 failed"),
            }
        }
    }

    conn.execute_batch(&format!("PRAGMA user_version = {SCHEMA_VERSION}"))
        .context("Failed to set schema version")?;

    Ok(())
}

/// Insert a new job into the queue.
pub fn insert_job(conn: &Connection, job_id: &str, priority: i32, data: &Value) -> Result<()> {
    insert_job_with_trace(conn, job_id, priority, data, None)
}

/// Insert a new job into the queue with an optional trace ID.
pub fn insert_job_with_trace(
    conn: &Connection,
    job_id: &str,
    priority: i32,
    data: &Value,
    trace_id: Option<&str>,
) -> Result<()> {
    insert_job_full(conn, job_id, priority, data, trace_id, None, None)
}

/// Insert a new job with full canonical retry lineage.
pub fn insert_job_full(
    conn: &Connection,
    job_id: &str,
    priority: i32,
    data: &Value,
    trace_id: Option<&str>,
    attempt_id: Option<&str>,
    trial_id: Option<&str>,
) -> Result<()> {
    conn.execute(
        "INSERT INTO queue_jobs (id, priority, status, data_json, trace_id, attempt_id, trial_id)
         VALUES (?1, ?2, 'pending', ?3, ?4, ?5, ?6)",
        params![
            job_id,
            priority,
            serde_json::to_string(data)?,
            trace_id,
            attempt_id,
            trial_id,
        ],
    )
    .context("Failed to insert queue job")?;
    Ok(())
}

/// Get the next pending job (highest priority, oldest first).
/// Returns the job ID and its data as a JSON value.
///
/// **Note:** Prefer [`claim_next_job`] for executor use — it atomically
/// selects and marks the job as processing in a single transaction,
/// preventing race conditions when multiple executors share a database.
pub fn get_next_pending(conn: &Connection) -> Result<Option<(String, Value)>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, data_json FROM queue_jobs
             WHERE status = 'pending'
             ORDER BY priority ASC, created_at ASC
             LIMIT 1",
        )
        .context("Failed to prepare get_next_pending query")?;

    let mut rows = stmt.query([]).context("Failed to query next pending job")?;

    if let Some(row) = rows.next().context("Failed to read next pending row")? {
        let id: String = row.get(0)?;
        let data_json: String = row.get(1)?;
        let data: Value =
            serde_json::from_str(&data_json).context("Failed to parse job data JSON")?;
        Ok(Some((id, data)))
    } else {
        Ok(None)
    }
}

/// Atomically claim the next pending job by selecting and marking it as
/// processing in a single transaction. Returns the job ID and its data,
/// or `None` if no pending jobs are available.
///
/// This prevents race conditions where two executors could claim the same job.
pub fn claim_next_job(conn: &Connection) -> Result<Option<(String, Value)>> {
    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn
        .unchecked_transaction()
        .context("Failed to begin transaction")?;

    let job = {
        let mut stmt = tx
            .prepare(
                "SELECT id, data_json FROM queue_jobs
                 WHERE status = 'pending'
                 ORDER BY priority ASC, created_at ASC
                 LIMIT 1",
            )
            .context("Failed to prepare claim_next_job SELECT")?;

        let mut rows = stmt.query([]).context("Failed to query next pending job")?;

        if let Some(row) = rows.next().context("Failed to read next pending row")? {
            let id: String = row.get(0)?;
            let data_json: String = row.get(1)?;
            Some((id, data_json))
        } else {
            None
        }
    };

    let Some((id, data_json)) = job else {
        tx.commit()
            .context("Failed to commit empty claim transaction")?;
        return Ok(None);
    };

    let affected = tx
        .execute(
            "UPDATE queue_jobs SET status = 'processing', started_at = ?1
             WHERE id = ?2 AND status = 'pending'",
            params![now, id],
        )
        .context("Failed to update job status to processing")?;

    tx.commit().context("Failed to commit claim transaction")?;

    if affected == 0 {
        // Another executor claimed it between SELECT and UPDATE
        return Ok(None);
    }

    let data: Value = serde_json::from_str(&data_json).context("Failed to parse job data JSON")?;
    Ok(Some((id, data)))
}

/// Atomically claim the next eligible job for a specific worker.
///
/// Like [`claim_next_job`] but records the `worker_id` and sets `heartbeat_at`.
/// Only considers jobs whose `next_run_at` is `NULL` or in the past.
pub fn claim(conn: &Connection, worker_id: &str) -> Result<Option<(String, Value)>> {
    claim_with_lease(conn, worker_id, 300)
}

/// Claim the next eligible job for a worker and record the configured lease timeout.
pub fn claim_with_lease(
    conn: &Connection,
    worker_id: &str,
    visibility_timeout_secs: u64,
) -> Result<Option<(String, Value)>> {
    let now = chrono::Utc::now().to_rfc3339();
    let tx = conn
        .unchecked_transaction()
        .context("Failed to begin claim transaction")?;

    let job = {
        let mut stmt = tx
            .prepare(
                "SELECT id, data_json FROM queue_jobs
                 WHERE status = 'pending'
                   AND (next_run_at IS NULL OR next_run_at <= ?1)
                 ORDER BY priority ASC, created_at ASC
                 LIMIT 1",
            )
            .context("Failed to prepare claim SELECT")?;

        let mut rows = stmt
            .query(params![now])
            .context("Failed to query for claimable job")?;

        if let Some(row) = rows.next().context("Failed to read claim row")? {
            let id: String = row.get(0)?;
            let data_json: String = row.get(1)?;
            Some((id, data_json))
        } else {
            None
        }
    };

    let Some((id, data_json)) = job else {
        tx.commit()
            .context("Failed to commit empty claim transaction")?;
        return Ok(None);
    };

    let affected = tx
        .execute(
            "UPDATE queue_jobs
             SET status = 'processing', started_at = ?1, worker_id = ?2,
                 heartbeat_at = ?1, visibility_timeout = ?3, attempt_count = attempt_count + 1
             WHERE id = ?4 AND status = 'pending'",
            params![now, worker_id, visibility_timeout_secs as i64, id],
        )
        .context("Failed to claim job for worker")?;

    tx.commit().context("Failed to commit claim transaction")?;

    if affected == 0 {
        return Ok(None);
    }

    let data: Value = serde_json::from_str(&data_json).context("Failed to parse job data JSON")?;
    Ok(Some((id, data)))
}

/// Update the heartbeat timestamp for a processing job.
///
/// Returns `true` if the heartbeat was recorded, `false` if the job is no
/// longer processing or does not belong to this worker.
pub fn heartbeat(conn: &Connection, job_id: &str, worker_id: &str) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let affected = conn
        .execute(
            "UPDATE queue_jobs SET heartbeat_at = ?1
             WHERE id = ?2 AND status = 'processing' AND worker_id = ?3",
            params![now, job_id, worker_id],
        )
        .context("Failed to update heartbeat")?;
    Ok(affected > 0)
}

/// Reclaim jobs that have been processing but have not sent a heartbeat
/// within `stale_secs` seconds. Resets them to `pending` with `worker_id`
/// cleared. Returns the number of jobs reclaimed.
pub fn reclaim_stale(conn: &Connection, stale_secs: u64) -> Result<u32> {
    let cutoff = chrono::Utc::now() - chrono::Duration::seconds(stale_secs as i64);
    let cutoff_str = cutoff.to_rfc3339();
    let count = conn
        .execute(
            "UPDATE queue_jobs
             SET status = 'pending', worker_id = NULL, heartbeat_at = NULL
             WHERE status = 'processing'
               AND heartbeat_at IS NOT NULL
               AND heartbeat_at < ?1",
            params![cutoff_str],
        )
        .context("Failed to reclaim stale jobs")?;
    Ok(count as u32)
}

/// Mark a failed job for retry with backoff. Sets `next_run_at` based on
/// the failure class and attempt count, then resets status to `pending`.
pub fn mark_failed_with_retry(
    conn: &Connection,
    job_id: &str,
    error: &str,
    failure_class: &FailureClass,
) -> Result<bool> {
    mark_failed_with_retry_owned(conn, job_id, None, error, failure_class, u32::MAX)
}

/// Mark a failed job for retry or permanent failure while validating worker ownership.
pub fn mark_failed_with_retry_owned(
    conn: &Connection,
    job_id: &str,
    worker_id: Option<&str>,
    error: &str,
    failure_class: &FailureClass,
    max_retries: u32,
) -> Result<bool> {
    let now = chrono::Utc::now();
    let now_str = now.to_rfc3339();
    let fc_str = failure_class.as_str();
    let attempt_count = get_attempt_count(conn, job_id).unwrap_or(1);
    let ownership_clause = if worker_id.is_some() {
        " AND worker_id = ?5"
    } else {
        ""
    };
    let permanent_failure =
        matches!(failure_class, FailureClass::Permanent) || attempt_count >= max_retries;

    if permanent_failure {
        let sql = format!(
            "UPDATE queue_jobs
             SET status = 'failed', completed_at = ?1, error_message = ?2,
                 failure_class = ?3
             WHERE id = ?4 AND status = 'processing'{}",
            ownership_clause
        );
        let affected = match worker_id {
            Some(worker_id) => {
                conn.execute(&sql, params![now_str, error, fc_str, job_id, worker_id])
            }
            None => conn.execute(&sql, params![now_str, error, fc_str, job_id]),
        }
        .context("Failed to mark job as permanently failed")?;
        return Ok(affected > 0);
    }

    let delay_secs = match failure_class {
        FailureClass::Transient => {
            // Exponential backoff: 2^(attempt-1) * 5 seconds, capped at 5 minutes
            let exponent = attempt_count.saturating_sub(1);
            let base = 5u64.saturating_mul(2u64.saturating_pow(exponent));
            base.min(300)
        }
        FailureClass::RateLimited { retry_after_secs } => *retry_after_secs,
        FailureClass::Permanent => unreachable!(),
    };

    let next_run = now + chrono::Duration::seconds(delay_secs as i64);
    let next_run_str = next_run.to_rfc3339();
    let sql = format!(
        "UPDATE queue_jobs
         SET status = 'pending', error_message = ?1, failure_class = ?2,
             worker_id = NULL, heartbeat_at = NULL, next_run_at = ?3
         WHERE id = ?4 AND status = 'processing'{}",
        ownership_clause
    );

    let affected = match worker_id {
        Some(worker_id) => conn.execute(
            &sql,
            params![error, fc_str, next_run_str, job_id, worker_id],
        ),
        None => conn.execute(&sql, params![error, fc_str, next_run_str, job_id]),
    }
    .context("Failed to mark job for retry")?;
    Ok(affected > 0)
}

/// Mark a job as processing and set started_at.
///
/// Only transitions jobs that are currently `pending`. Returns `true` if the
/// update was applied, `false` if the job was no longer pending (e.g., cancelled).
///
/// **Note:** Prefer [`claim_next_job`] or [`claim`] for executor use — they
/// atomically select and mark the job as processing in a single transaction.
pub fn mark_processing(conn: &Connection, job_id: &str) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let affected = conn
        .execute(
            "UPDATE queue_jobs SET status = 'processing', started_at = ?1
             WHERE id = ?2 AND status = 'pending'",
            params![now, job_id],
        )
        .context("Failed to mark job as processing")?;
    Ok(affected > 0)
}

/// Mark a job as completed and set completed_at.
///
/// Only transitions jobs that are currently `processing`. Returns `true` if the
/// update was applied, `false` if the job's status had already changed (e.g., cancelled).
pub fn mark_completed(conn: &Connection, job_id: &str) -> Result<bool> {
    mark_completed_owned(conn, job_id, None)
}

/// Mark a job as completed, optionally validating worker ownership.
pub fn mark_completed_owned(
    conn: &Connection,
    job_id: &str,
    worker_id: Option<&str>,
) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let sql = if worker_id.is_some() {
        "UPDATE queue_jobs SET status = 'completed', completed_at = ?1
         WHERE id = ?2 AND status = 'processing' AND worker_id = ?3"
    } else {
        "UPDATE queue_jobs SET status = 'completed', completed_at = ?1
         WHERE id = ?2 AND status = 'processing'"
    };
    let affected = match worker_id {
        Some(worker_id) => conn.execute(sql, params![now, job_id, worker_id]),
        None => conn.execute(sql, params![now, job_id]),
    }
    .context("Failed to mark job as completed")?;
    Ok(affected > 0)
}

/// Mark a job as failed with an error message and set completed_at.
///
/// Only transitions jobs that are currently `processing`. Returns `true` if the
/// update was applied, `false` if the job's status had already changed (e.g., cancelled).
pub fn mark_failed(conn: &Connection, job_id: &str, error: &str) -> Result<bool> {
    mark_failed_owned(conn, job_id, None, error)
}

/// Mark a job as failed, optionally validating worker ownership.
pub fn mark_failed_owned(
    conn: &Connection,
    job_id: &str,
    worker_id: Option<&str>,
    error: &str,
) -> Result<bool> {
    let now = chrono::Utc::now().to_rfc3339();
    let sql = if worker_id.is_some() {
        "UPDATE queue_jobs SET status = 'failed', completed_at = ?1, error_message = ?2
         WHERE id = ?3 AND status = 'processing' AND worker_id = ?4"
    } else {
        "UPDATE queue_jobs SET status = 'failed', completed_at = ?1, error_message = ?2
         WHERE id = ?3 AND status = 'processing'"
    };
    let affected = match worker_id {
        Some(worker_id) => conn.execute(sql, params![now, error, job_id, worker_id]),
        None => conn.execute(sql, params![now, error, job_id]),
    }
    .context("Failed to mark job as failed")?;
    Ok(affected > 0)
}

/// Check if a job has been cancelled (used by executor during execution).
pub fn is_cancelled(conn: &Connection, job_id: &str) -> Result<bool> {
    let status: String = conn
        .query_row(
            "SELECT status FROM queue_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|_| anyhow::anyhow!("Job '{}' not found", job_id))?;
    Ok(status == "cancelled")
}

/// Cancel a pending or processing job. Returns the previous status.
pub fn cancel_job(conn: &Connection, job_id: &str) -> Result<String> {
    let prev_status: String = conn
        .query_row(
            "SELECT status FROM queue_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .map_err(|_| anyhow::anyhow!("Job '{}' not found", job_id))?;

    if prev_status != "pending" && prev_status != "processing" {
        anyhow::bail!(
            "Job '{}' is not cancellable (status: {})",
            job_id,
            prev_status
        );
    }

    let now = chrono::Utc::now().to_rfc3339();
    let affected = conn
        .execute(
            "UPDATE queue_jobs SET status = 'cancelled', completed_at = ?1
         WHERE id = ?2 AND status IN ('pending', 'processing')",
            params![now, job_id],
        )
        .context("Failed to cancel job")?;

    if affected == 0 {
        anyhow::bail!(
            "Job '{}' changed state before cancellation could be applied (previous status: {})",
            job_id,
            prev_status
        );
    }

    Ok(prev_status)
}

/// Re-queue any jobs that were mid-processing when the app crashed.
/// Returns the number of jobs requeued.
pub fn requeue_interrupted(conn: &Connection) -> Result<u32> {
    let count = conn
        .execute(
            "UPDATE queue_jobs SET status = 'pending' WHERE status = 'processing'",
            [],
        )
        .context("Failed to requeue interrupted jobs")?;
    Ok(count as u32)
}

/// Update the priority of a job.
pub fn update_priority(conn: &Connection, job_id: &str, priority: i32) -> Result<()> {
    conn.execute(
        "UPDATE queue_jobs SET priority = ?1 WHERE id = ?2",
        params![priority, job_id],
    )
    .context("Failed to update job priority")?;
    Ok(())
}

/// Atomically update the priority of a pending job.
///
/// Returns `Ok(true)` if the job was updated, `Ok(false)` if the job exists but
/// is not pending, or `Err` with a not-found message if the job doesn't exist.
pub fn reorder_pending(conn: &Connection, job_id: &str, priority: i32) -> Result<bool> {
    let affected = conn
        .execute(
            "UPDATE queue_jobs SET priority = ?1 WHERE id = ?2 AND status = 'pending'",
            params![priority, job_id],
        )
        .context("Failed to reorder job")?;

    if affected > 0 {
        return Ok(true);
    }

    // Distinguish "not found" from "not pending"
    let exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM queue_jobs WHERE id = ?1",
            params![job_id],
            |row| row.get(0),
        )
        .context("Failed to check job existence")?;

    if exists {
        Ok(false)
    } else {
        anyhow::bail!("Job '{}' not found", job_id)
    }
}

/// List all jobs ordered by status then priority then creation time.
/// Returns tuples of (id, status, data_json).
pub fn list_all_jobs(conn: &Connection) -> Result<Vec<(String, String, String)>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, status, data_json FROM queue_jobs
             ORDER BY
                CASE status
                    WHEN 'processing' THEN 0
                    WHEN 'pending' THEN 1
                    WHEN 'completed' THEN 2
                    WHEN 'failed' THEN 3
                    WHEN 'cancelled' THEN 4
                END,
                priority ASC,
                created_at ASC",
        )
        .context("Failed to prepare list_all_jobs query")?;

    let rows = stmt
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .context("Failed to execute list_all_jobs query")?;

    let mut jobs = Vec::new();
    for row in rows {
        jobs.push(row.context("Failed to read job row")?);
    }
    Ok(jobs)
}

/// Delete completed/failed/cancelled jobs older than the specified number of days.
/// Returns the number of jobs deleted.
pub fn prune_old_jobs(conn: &Connection, days: u32) -> Result<u32> {
    let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
    let cutoff_str = cutoff.to_rfc3339();

    let count = conn
        .execute(
            "DELETE FROM queue_jobs
             WHERE status IN ('completed', 'failed', 'cancelled')
             AND completed_at < ?1",
            params![cutoff_str],
        )
        .context("Failed to prune old queue jobs")?;

    Ok(count as u32)
}

/// Count jobs by status.
pub fn count_by_status(conn: &Connection) -> Result<QueueStats> {
    let mut stats = QueueStats::default();

    let mut stmt = conn
        .prepare("SELECT status, COUNT(*) FROM queue_jobs GROUP BY status")
        .context("Failed to prepare count_by_status query")?;

    let rows = stmt
        .query_map([], |row| {
            let status: String = row.get(0)?;
            let count: u32 = row.get(1)?;
            Ok((status, count))
        })
        .context("Failed to execute count_by_status query")?;

    for row in rows {
        let (status, count) = row.context("Failed to read status count row")?;
        match status.as_str() {
            "pending" => stats.pending = count,
            "processing" => stats.processing = count,
            "completed" => stats.completed = count,
            "failed" => stats.failed = count,
            "cancelled" => stats.cancelled = count,
            _ => {}
        }
    }

    Ok(stats)
}

/// Row data for a single job.
pub type JobRow = (String, i32, String, String, Option<String>);

/// Get the recorded attempt count for a job.
pub fn get_attempt_count(conn: &Connection, job_id: &str) -> Result<u32> {
    conn.query_row(
        "SELECT attempt_count FROM queue_jobs WHERE id = ?1",
        params![job_id],
        |row| row.get(0),
    )
    .context("Failed to fetch attempt count")
}

/// Get a single job by ID. Returns (id, priority, status, data_json, error_message).
pub fn get_job(conn: &Connection, job_id: &str) -> Result<Option<JobRow>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, priority, status, data_json, error_message
             FROM queue_jobs WHERE id = ?1",
        )
        .context("Failed to prepare get_job query")?;

    let mut rows = stmt.query(params![job_id])?;

    if let Some(row) = rows.next()? {
        Ok(Some((
            row.get(0)?,
            row.get(1)?,
            row.get(2)?,
            row.get(3)?,
            row.get(4)?,
        )))
    } else {
        Ok(None)
    }
}

/// Get a structured view of a job for debugging and UI inspection.
pub fn get_job_details(conn: &Connection, job_id: &str) -> Result<Option<QueueJobDetails>> {
    let mut stmt = conn
        .prepare(
            "SELECT id, priority, status, data_json, trace_id, created_at, started_at,
                    completed_at, error_message, worker_id, heartbeat_at, visibility_timeout,
                    failure_class, next_run_at, attempt_count, attempt_id, trial_id
             FROM queue_jobs WHERE id = ?1",
        )
        .context("Failed to prepare get_job_details query")?;

    let mut rows = stmt.query(params![job_id])?;
    if let Some(row) = rows.next()? {
        let priority: i32 = row.get(1)?;
        let status: String = row.get(2)?;
        Ok(Some(QueueJobDetails {
            id: row.get(0)?,
            priority: QueuePriority::from_i32(priority),
            status: QueueJobStatus::parse(&status).unwrap_or(QueueJobStatus::Failed),
            data_json: row.get(3)?,
            trace_id: row.get(4)?,
            created_at: row.get(5)?,
            started_at: row.get(6)?,
            completed_at: row.get(7)?,
            error_message: row.get(8)?,
            worker_id: row.get(9)?,
            heartbeat_at: row.get(10)?,
            visibility_timeout_secs: row.get::<_, i64>(11)? as u64,
            failure_class: row.get(12)?,
            next_run_at: row.get(13)?,
            attempt_count: row.get::<_, i64>(14)? as u32,
            attempt_id: row.get(15)?,
            trial_id: row.get(16)?,
        }))
    } else {
        Ok(None)
    }
}

/// Persist canonical retry lineage fields for a job.
///
/// Updates `attempt_id` and/or `trial_id` on an existing job row.
/// Returns `true` if the row was updated, `false` if the job was not found.
pub fn update_canonical_lineage(
    conn: &Connection,
    job_id: &str,
    attempt_id: Option<&str>,
    trial_id: Option<&str>,
) -> Result<bool> {
    let affected = conn
        .execute(
            "UPDATE queue_jobs SET attempt_id = COALESCE(?1, attempt_id),
                                   trial_id = COALESCE(?2, trial_id)
             WHERE id = ?3",
            params![attempt_id, trial_id, job_id],
        )
        .context("Failed to update canonical lineage")?;
    Ok(affected > 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> Connection {
        open_database(None).unwrap()
    }

    #[test]
    fn test_open_in_memory() {
        let conn = open_database(None);
        assert!(conn.is_ok());
    }

    #[test]
    fn test_insert_and_get_next_pending() {
        let conn = setup();
        let data = serde_json::json!({"task": "send email"});
        insert_job(&conn, "job-1", 2, &data).unwrap();

        let next = get_next_pending(&conn).unwrap();
        assert!(next.is_some());
        let (id, val) = next.unwrap();
        assert_eq!(id, "job-1");
        assert_eq!(val["task"], "send email");
    }

    #[test]
    fn test_priority_ordering() {
        let conn = setup();
        insert_job(&conn, "low-1", 3, &serde_json::json!({"p": "low"})).unwrap();
        insert_job(&conn, "high-1", 1, &serde_json::json!({"p": "high"})).unwrap();
        insert_job(&conn, "normal-1", 2, &serde_json::json!({"p": "normal"})).unwrap();

        let next = get_next_pending(&conn).unwrap().unwrap();
        assert_eq!(next.0, "high-1");
    }

    #[test]
    fn test_mark_processing() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        assert!(mark_processing(&conn, "job-1").unwrap());

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "processing");

        // No more pending jobs
        assert!(get_next_pending(&conn).unwrap().is_none());
    }

    #[test]
    fn test_mark_processing_guards_status() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        // Cancel the job first
        cancel_job(&conn, "job-1").unwrap();
        // Trying to mark a cancelled job as processing should be a no-op
        assert!(!mark_processing(&conn, "job-1").unwrap());
    }

    #[test]
    fn test_mark_completed() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        assert!(mark_completed(&conn, "job-1").unwrap());

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "completed");
    }

    #[test]
    fn test_mark_completed_guards_status() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        cancel_job(&conn, "job-1").unwrap();
        // Should be a no-op because job is cancelled, not processing
        assert!(!mark_completed(&conn, "job-1").unwrap());
        // Status should still be cancelled
        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "cancelled");
    }

    #[test]
    fn test_mark_failed() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        assert!(mark_failed(&conn, "job-1", "something broke").unwrap());

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "failed");
        assert_eq!(job.4.as_deref(), Some("something broke"));
    }

    #[test]
    fn test_mark_failed_guards_status() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        cancel_job(&conn, "job-1").unwrap();
        // Should be a no-op because job is cancelled, not processing
        assert!(!mark_failed(&conn, "job-1", "error").unwrap());
        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "cancelled");
    }

    #[test]
    fn test_cancel_pending() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        let prev = cancel_job(&conn, "job-1").unwrap();
        assert_eq!(prev, "pending");

        assert!(is_cancelled(&conn, "job-1").unwrap());
    }

    #[test]
    fn test_cancel_processing() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        let prev = cancel_job(&conn, "job-1").unwrap();
        assert_eq!(prev, "processing");

        assert!(is_cancelled(&conn, "job-1").unwrap());
    }

    #[test]
    fn test_cancel_completed_fails() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        mark_completed(&conn, "job-1").unwrap();

        let result = cancel_job(&conn, "job-1");
        assert!(result.is_err());
    }

    #[test]
    fn test_requeue_interrupted() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();

        let count = requeue_interrupted(&conn).unwrap();
        assert_eq!(count, 1);

        let next = get_next_pending(&conn).unwrap();
        assert!(next.is_some());
        assert_eq!(next.unwrap().0, "job-1");
    }

    #[test]
    fn test_update_priority() {
        let conn = setup();
        insert_job(&conn, "job-1", 3, &serde_json::json!({})).unwrap();
        update_priority(&conn, "job-1", 1).unwrap();

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.1, 1);
    }

    #[test]
    fn test_list_all_jobs() {
        let conn = setup();
        insert_job(&conn, "a", 2, &serde_json::json!({"n": 1})).unwrap();
        insert_job(&conn, "b", 1, &serde_json::json!({"n": 2})).unwrap();
        insert_job(&conn, "c", 3, &serde_json::json!({"n": 3})).unwrap();

        let jobs = list_all_jobs(&conn).unwrap();
        assert_eq!(jobs.len(), 3);
        // All pending, so ordered by priority: b(1), a(2), c(3)
        assert_eq!(jobs[0].0, "b");
        assert_eq!(jobs[1].0, "a");
        assert_eq!(jobs[2].0, "c");
    }

    #[test]
    fn test_prune_old_jobs() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        mark_completed(&conn, "job-1").unwrap();

        // Job completed just now -- pruning with 30 days should NOT remove it
        let count = prune_old_jobs(&conn, 30).unwrap();
        assert_eq!(count, 0);

        // Set completed_at to 10 days ago manually
        let old_date = (chrono::Utc::now() - chrono::Duration::days(10)).to_rfc3339();
        conn.execute(
            "UPDATE queue_jobs SET completed_at = ?1 WHERE id = 'job-1'",
            params![old_date],
        )
        .unwrap();

        // Pruning with 5 days should remove it (10 days old > 5 day cutoff)
        let count = prune_old_jobs(&conn, 5).unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn test_get_job_not_found() {
        let conn = setup();
        let result = get_job(&conn, "nonexistent").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_claim_next_job() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({"task": "test"})).unwrap();
        insert_job(&conn, "job-2", 1, &serde_json::json!({"task": "high"})).unwrap();

        // Should claim the highest priority job
        let claimed = claim_next_job(&conn).unwrap();
        assert!(claimed.is_some());
        let (id, data) = claimed.unwrap();
        assert_eq!(id, "job-2");
        assert_eq!(data["task"], "high");

        // job-2 should now be processing
        let job = get_job(&conn, "job-2").unwrap().unwrap();
        assert_eq!(job.2, "processing");

        // Next claim should get job-1
        let claimed2 = claim_next_job(&conn).unwrap();
        assert!(claimed2.is_some());
        assert_eq!(claimed2.unwrap().0, "job-1");

        // No more pending
        assert!(claim_next_job(&conn).unwrap().is_none());
    }

    #[test]
    fn test_claim_next_job_empty_queue() {
        let conn = setup();
        assert!(claim_next_job(&conn).unwrap().is_none());
    }

    #[test]
    fn test_claim_skips_cancelled() {
        let conn = setup();
        insert_job(&conn, "job-1", 1, &serde_json::json!({})).unwrap();
        cancel_job(&conn, "job-1").unwrap();

        // Cancelled job should not be claimed
        assert!(claim_next_job(&conn).unwrap().is_none());
    }

    #[test]
    fn test_reorder_pending() {
        let conn = setup();
        insert_job(&conn, "job-1", 3, &serde_json::json!({})).unwrap();

        // Reorder a pending job — should succeed
        assert!(reorder_pending(&conn, "job-1", 1).unwrap());
        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.1, 1);
    }

    #[test]
    fn test_reorder_non_pending_returns_false() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();

        // Job is processing, not pending
        assert!(!reorder_pending(&conn, "job-1", 1).unwrap());
    }

    #[test]
    fn test_reorder_nonexistent_errors() {
        let conn = setup();
        assert!(reorder_pending(&conn, "nope", 1).is_err());
    }

    // ── VG-6 new tests ──────────────────────────────────────────

    #[test]
    fn test_claim_with_worker_id() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({"x": 1})).unwrap();

        let claimed = claim(&conn, "worker-A").unwrap();
        assert!(claimed.is_some());
        let (id, _data) = claimed.unwrap();
        assert_eq!(id, "job-1");

        // Verify worker_id is recorded
        let worker: String = conn
            .query_row(
                "SELECT worker_id FROM queue_jobs WHERE id = 'job-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(worker, "worker-A");
    }

    #[test]
    fn test_heartbeat() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "w1").unwrap();

        // Heartbeat from correct worker
        assert!(heartbeat(&conn, "job-1", "w1").unwrap());
        // Heartbeat from wrong worker
        assert!(!heartbeat(&conn, "job-1", "w2").unwrap());
    }

    #[test]
    fn test_reclaim_stale_jobs() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "w1").unwrap();

        // Set heartbeat to 10 minutes ago
        let old = (chrono::Utc::now() - chrono::Duration::minutes(10)).to_rfc3339();
        conn.execute(
            "UPDATE queue_jobs SET heartbeat_at = ?1 WHERE id = 'job-1'",
            params![old],
        )
        .unwrap();

        // Reclaim jobs stale for > 5 minutes (300 seconds)
        let count = reclaim_stale(&conn, 300).unwrap();
        assert_eq!(count, 1);

        // Job should be pending again
        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "pending");
    }

    #[test]
    fn test_reclaim_stale_ignores_fresh() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "w1").unwrap();
        heartbeat(&conn, "job-1", "w1").unwrap();

        // Jobs with fresh heartbeat should NOT be reclaimed
        let count = reclaim_stale(&conn, 300).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_retry_backoff_transient() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "w1").unwrap();

        // Fail with transient error → should be requeued with next_run_at
        let ok =
            mark_failed_with_retry(&conn, "job-1", "timeout", &FailureClass::Transient).unwrap();
        assert!(ok);

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "pending"); // re-queued, not failed

        // next_run_at should be set
        let next_run: Option<String> = conn
            .query_row(
                "SELECT next_run_at FROM queue_jobs WHERE id = 'job-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(next_run.is_some());
    }

    #[test]
    fn test_retry_backoff_permanent() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "w1").unwrap();

        let ok =
            mark_failed_with_retry(&conn, "job-1", "bad input", &FailureClass::Permanent).unwrap();
        assert!(ok);

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "failed"); // NOT re-queued
    }

    #[test]
    fn test_retry_rate_limited() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "w1").unwrap();

        let ok = mark_failed_with_retry(
            &conn,
            "job-1",
            "rate limited",
            &FailureClass::RateLimited {
                retry_after_secs: 60,
            },
        )
        .unwrap();
        assert!(ok);

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "pending"); // re-queued

        // failure_class should be recorded
        let fc: Option<String> = conn
            .query_row(
                "SELECT failure_class FROM queue_jobs WHERE id = 'job-1'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(fc, Some("rate_limited".to_string()));
    }

    #[test]
    fn test_retry_exhausted_remains_failed() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "w1").unwrap();

        let ok = mark_failed_with_retry_owned(
            &conn,
            "job-1",
            Some("w1"),
            "still failing",
            &FailureClass::Transient,
            1,
        )
        .unwrap();
        assert!(ok);

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "failed");
    }

    #[test]
    fn test_worker_mismatch_rejects_complete_and_fail() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        claim(&conn, "worker-a").unwrap();

        assert!(!mark_completed_owned(&conn, "job-1", Some("worker-b")).unwrap());
        assert!(!mark_failed_owned(&conn, "job-1", Some("worker-b"), "wrong worker").unwrap());

        let job = get_job(&conn, "job-1").unwrap().unwrap();
        assert_eq!(job.2, "processing");
    }

    #[test]
    fn test_trace_id_column_and_details_round_trip() {
        let conn = setup();
        insert_job_with_trace(
            &conn,
            "job-1",
            2,
            &serde_json::json!({"task": "trace"}),
            Some("trace-001"),
        )
        .unwrap();

        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert_eq!(details.trace_id.as_deref(), Some("trace-001"));
        assert_eq!(details.status, QueueJobStatus::Pending);
    }

    #[test]
    fn test_invalid_transition_completed_to_processing() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();
        mark_processing(&conn, "job-1").unwrap();
        mark_completed(&conn, "job-1").unwrap();

        // Trying to mark a completed job as processing should return false
        assert!(!mark_processing(&conn, "job-1").unwrap());
    }

    #[test]
    fn test_claim_skips_future_next_run() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();

        // Set next_run_at to 1 hour in the future
        let future = (chrono::Utc::now() + chrono::Duration::hours(1)).to_rfc3339();
        conn.execute(
            "UPDATE queue_jobs SET next_run_at = ?1 WHERE id = 'job-1'",
            params![future],
        )
        .unwrap();

        // claim() should skip this job
        let claimed = claim(&conn, "w1").unwrap();
        assert!(claimed.is_none());
    }

    // ── V4 canonical retry lineage tests ──────────────────────────

    #[test]
    fn test_insert_job_full_with_canonical_fields() {
        let conn = setup();
        insert_job_full(
            &conn,
            "job-1",
            2,
            &serde_json::json!({"task": "lineage"}),
            Some("trace-001"),
            Some("attempt-abc"),
            Some("trial-xyz"),
        )
        .unwrap();

        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert_eq!(details.trace_id.as_deref(), Some("trace-001"));
        assert_eq!(details.attempt_id.as_deref(), Some("attempt-abc"));
        assert_eq!(details.trial_id.as_deref(), Some("trial-xyz"));
    }

    #[test]
    fn test_insert_job_full_with_none_canonical_fields() {
        let conn = setup();
        insert_job_full(&conn, "job-1", 2, &serde_json::json!({}), None, None, None).unwrap();

        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert!(details.attempt_id.is_none());
        assert!(details.trial_id.is_none());
    }

    #[test]
    fn test_update_canonical_lineage() {
        let conn = setup();
        insert_job(&conn, "job-1", 2, &serde_json::json!({})).unwrap();

        // Initially NULL
        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert!(details.attempt_id.is_none());
        assert!(details.trial_id.is_none());

        // Update both
        let ok = update_canonical_lineage(&conn, "job-1", Some("att-1"), Some("trial-1")).unwrap();
        assert!(ok);

        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert_eq!(details.attempt_id.as_deref(), Some("att-1"));
        assert_eq!(details.trial_id.as_deref(), Some("trial-1"));
    }

    #[test]
    fn test_update_canonical_lineage_partial() {
        let conn = setup();
        insert_job_full(
            &conn,
            "job-1",
            2,
            &serde_json::json!({}),
            None,
            Some("att-original"),
            None,
        )
        .unwrap();

        // Update only trial_id — attempt_id should remain
        let ok = update_canonical_lineage(&conn, "job-1", None, Some("trial-new")).unwrap();
        assert!(ok);

        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert_eq!(details.attempt_id.as_deref(), Some("att-original"));
        assert_eq!(details.trial_id.as_deref(), Some("trial-new"));
    }

    #[test]
    fn test_update_canonical_lineage_nonexistent_job() {
        let conn = setup();
        let ok =
            update_canonical_lineage(&conn, "nonexistent", Some("att-1"), Some("trial-1")).unwrap();
        assert!(!ok);
    }

    #[test]
    fn test_canonical_fields_survive_retry_cycle() {
        let conn = setup();
        insert_job_full(
            &conn,
            "job-1",
            2,
            &serde_json::json!({}),
            None,
            Some("att-1"),
            Some("trial-1"),
        )
        .unwrap();

        // Claim the job (simulates executor picking it up)
        claim(&conn, "w1").unwrap();

        // Fail with transient error → requeued
        mark_failed_with_retry(&conn, "job-1", "timeout", &FailureClass::Transient).unwrap();

        // Canonical fields should survive the retry cycle
        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert_eq!(details.attempt_id.as_deref(), Some("att-1"));
        assert_eq!(details.trial_id.as_deref(), Some("trial-1"));
        assert_eq!(details.status, QueueJobStatus::Pending);
    }

    #[test]
    fn test_legacy_insert_without_canonical_fields() {
        let conn = setup();
        // Use the legacy insert path (no canonical fields)
        insert_job_with_trace(
            &conn,
            "job-1",
            2,
            &serde_json::json!({}),
            Some("trace-legacy"),
        )
        .unwrap();

        let details = get_job_details(&conn, "job-1").unwrap().unwrap();
        assert_eq!(details.trace_id.as_deref(), Some("trace-legacy"));
        // canonical fields should be None for legacy inserts
        assert!(details.attempt_id.is_none());
        assert!(details.trial_id.is_none());
    }
}
