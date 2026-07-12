use crate::db::app_db::AppDb;
use crate::db::notebook_db::NotebookDb;
use crate::error::GlossError;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DbDoctorSeverity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbDoctorFinding {
    pub notebook_id: String,
    pub code: String,
    pub severity: DbDoctorSeverity,
    pub count: usize,
    pub repaired: bool,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbDoctorNotebookReport {
    pub notebook_id: String,
    pub notebook_db_present: bool,
    pub source_count_recorded: i32,
    pub source_count_actual: Option<i32>,
    pub orphan_source_processing_state_rows: usize,
    pub orphan_projection_status_rows: usize,
    pub orphan_semantic_memory_link_rows: usize,
    pub failed_import_sources: usize,
    pub quarantined_failed_import_sources: usize,
    pub receipt_id: Option<String>,
    pub supersedes_receipt_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DbDoctorReceipt {
    pub schema: String,
    pub receipt_id: String,
    pub repair: bool,
    pub recorded_utc: String,
    pub notebooks_checked: usize,
    pub findings: Vec<DbDoctorFinding>,
    pub notebook_reports: Vec<DbDoctorNotebookReport>,
    pub repaired_source_count_mismatches: usize,
    pub repaired_orphan_rows: usize,
    pub failed_import_sources: usize,
    pub quarantined_failed_import_sources: usize,
    pub queue_jobs_checked: usize,
    pub stale_queue_jobs: usize,
    pub repaired_stale_queue_jobs: usize,
}

pub fn run_db_doctor(app_db: &AppDb, repair: bool) -> Result<DbDoctorReceipt, GlossError> {
    let receipt_id = uuid::Uuid::new_v4().to_string();
    let recorded_utc = chrono::Utc::now().to_rfc3339();
    let notebooks = app_db.list_notebooks()?;
    let mut findings = Vec::new();
    let mut notebook_reports = Vec::new();
    let mut repaired_source_count_mismatches = 0usize;
    let mut repaired_orphan_rows = 0usize;
    let mut failed_import_sources = 0usize;
    let mut quarantined_failed_import_sources = 0usize;

    for notebook in &notebooks {
        let notebook_db_path = PathBuf::from(&notebook.directory).join("notebook.db");
        if !notebook_db_path.exists() {
            findings.push(DbDoctorFinding {
                notebook_id: notebook.id.clone(),
                code: "missing_notebook_db".to_string(),
                severity: DbDoctorSeverity::Error,
                count: 1,
                repaired: false,
                detail: "Notebook registry entry points at a missing notebook.db; automatic repair is not safe.".to_string(),
            });
            notebook_reports.push(DbDoctorNotebookReport {
                notebook_id: notebook.id.clone(),
                notebook_db_present: false,
                source_count_recorded: notebook.source_count,
                source_count_actual: None,
                orphan_source_processing_state_rows: 0,
                orphan_projection_status_rows: 0,
                orphan_semantic_memory_link_rows: 0,
                failed_import_sources: 0,
                quarantined_failed_import_sources: 0,
                receipt_id: None,
                supersedes_receipt_id: None,
            });
            continue;
        }

        let notebook_db = NotebookDb::open(&notebook_db_path)?;
        let actual_source_count = notebook_db.source_count()?;
        let source_count_repaired = repair && actual_source_count != notebook.source_count;
        if actual_source_count != notebook.source_count {
            if source_count_repaired {
                app_db.update_source_count(&notebook.id, actual_source_count)?;
                repaired_source_count_mismatches += 1;
            }
            findings.push(DbDoctorFinding {
                notebook_id: notebook.id.clone(),
                code: "source_count_mismatch".to_string(),
                severity: DbDoctorSeverity::Warning,
                count: 1,
                repaired: source_count_repaired,
                detail: format!(
                    "Registry source_count={} but notebook DB contains {} sources.",
                    notebook.source_count, actual_source_count
                ),
            });
        }

        let orphan_processing = count_orphan_source_processing_state(&notebook_db)?;
        let orphan_projection = count_orphan_projection_status(&notebook_db)?;
        let orphan_links = count_orphan_semantic_memory_links(&notebook_db)?;
        let failed_import_count = count_failed_import_sources(&notebook_db)?;
        let quarantine_candidates = count_unquarantined_failed_import_sources(&notebook_db)?;
        failed_import_sources += failed_import_count;

        if orphan_processing > 0 {
            findings.push(DbDoctorFinding {
                notebook_id: notebook.id.clone(),
                code: "orphan_source_processing_state".to_string(),
                severity: DbDoctorSeverity::Warning,
                count: orphan_processing,
                repaired: repair,
                detail: "source_processing_state rows without sources rows.".to_string(),
            });
        }
        if orphan_projection > 0 {
            findings.push(DbDoctorFinding {
                notebook_id: notebook.id.clone(),
                code: "orphan_projection_status".to_string(),
                severity: DbDoctorSeverity::Warning,
                count: orphan_projection,
                repaired: repair,
                detail: "semantic_memory_projection_status rows without sources rows.".to_string(),
            });
        }
        if orphan_links > 0 {
            findings.push(DbDoctorFinding {
                notebook_id: notebook.id.clone(),
                code: "orphan_semantic_memory_links".to_string(),
                severity: DbDoctorSeverity::Warning,
                count: orphan_links,
                repaired: repair,
                detail: "semantic_memory_links rows without matching source or chunk rows."
                    .to_string(),
            });
        }
        if failed_import_count > 0 {
            findings.push(DbDoctorFinding {
                notebook_id: notebook.id.clone(),
                code: "failed_import_sources".to_string(),
                severity: DbDoctorSeverity::Warning,
                count: failed_import_count,
                repaired: repair && quarantine_candidates > 0,
                detail: "sources with status='error'; repair deselects them as a failed-import quarantine while leaving retry available.".to_string(),
            });
        }

        let repaired_rows = if repair {
            repair_orphan_rows(&notebook_db)?
        } else {
            0
        };
        repaired_orphan_rows += repaired_rows;
        let quarantined_rows = if repair {
            quarantine_failed_import_sources(&notebook_db)?
        } else {
            0
        };
        quarantined_failed_import_sources += quarantined_rows;

        let (notebook_receipt_id, supersedes_receipt_id) =
            if repair && (source_count_repaired || repaired_rows > 0 || quarantined_rows > 0) {
                let notebook_receipt_id = uuid::Uuid::new_v4().to_string();
                let supersedes = latest_doctor_receipt_id(&notebook_db, &notebook.id)?;
                let raw_receipt = serde_json::json!({
                    "schema": "DbDoctorNotebookRepairReceiptV1",
                    "receipt_id": notebook_receipt_id,
                    "aggregate_receipt_id": receipt_id,
                    "notebook_id": notebook.id,
                    "recorded_utc": recorded_utc,
                    "source_count_before": notebook.source_count,
                    "source_count_after": actual_source_count,
                    "orphan_source_processing_state_rows_removed": orphan_processing,
                    "orphan_projection_status_rows_removed": orphan_projection,
                    "orphan_semantic_memory_link_rows_removed": orphan_links,
                    "failed_import_sources": failed_import_count,
                    "quarantined_failed_import_sources": quarantined_rows,
                    "supersedes_receipt_id": supersedes,
                });
                insert_doctor_receipt(
                    &notebook_db,
                    &notebook_receipt_id,
                    &notebook.id,
                    supersedes.as_deref(),
                    &raw_receipt,
                )?;
                (Some(notebook_receipt_id), supersedes)
            } else {
                (None, None)
            };

        notebook_reports.push(DbDoctorNotebookReport {
            notebook_id: notebook.id.clone(),
            notebook_db_present: true,
            source_count_recorded: notebook.source_count,
            source_count_actual: Some(actual_source_count),
            orphan_source_processing_state_rows: orphan_processing,
            orphan_projection_status_rows: orphan_projection,
            orphan_semantic_memory_link_rows: orphan_links,
            failed_import_sources: failed_import_count,
            quarantined_failed_import_sources: quarantined_rows,
            receipt_id: notebook_receipt_id,
            supersedes_receipt_id,
        });
    }

    Ok(DbDoctorReceipt {
        schema: "DbDoctorReceiptV1".to_string(),
        receipt_id,
        repair,
        recorded_utc,
        notebooks_checked: notebooks.len(),
        findings,
        notebook_reports,
        repaired_source_count_mismatches,
        repaired_orphan_rows,
        failed_import_sources,
        quarantined_failed_import_sources,
        queue_jobs_checked: 0,
        stale_queue_jobs: 0,
        repaired_stale_queue_jobs: 0,
    })
}

fn table_exists(db: &NotebookDb, table: &str) -> Result<bool, GlossError> {
    let exists: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get(0),
    )?;
    Ok(exists > 0)
}

fn count_orphan_source_processing_state(db: &NotebookDb) -> Result<usize, GlossError> {
    if !table_exists(db, "source_processing_state")? {
        return Ok(0);
    }
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*)
         FROM source_processing_state ps
         LEFT JOIN sources s ON s.id = ps.source_id
         WHERE s.id IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

fn count_orphan_projection_status(db: &NotebookDb) -> Result<usize, GlossError> {
    if !table_exists(db, "semantic_memory_projection_status")? {
        return Ok(0);
    }
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*)
         FROM semantic_memory_projection_status ps
         LEFT JOIN sources s ON s.id = ps.source_id
         WHERE s.id IS NULL",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

fn count_orphan_semantic_memory_links(db: &NotebookDb) -> Result<usize, GlossError> {
    if !table_exists(db, "semantic_memory_links")? {
        return Ok(0);
    }
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*)
         FROM semantic_memory_links l
         LEFT JOIN chunks c ON c.id = COALESCE(l.gloss_chunk_id, l.chunk_id)
         LEFT JOIN sources s ON s.id = l.source_id
         WHERE c.id IS NULL
            OR s.id IS NULL
            OR (l.projection_unit_kind IS NOT NULL
                AND l.projection_unit_id IS NULL)",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

fn repair_orphan_rows(db: &NotebookDb) -> Result<usize, GlossError> {
    let mut repaired = 0usize;
    if table_exists(db, "source_processing_state")? {
        repaired += db.conn().execute(
            "DELETE FROM source_processing_state
             WHERE source_id NOT IN (SELECT id FROM sources)",
            [],
        )?;
    }
    if table_exists(db, "semantic_memory_projection_status")? {
        repaired += db.conn().execute(
            "DELETE FROM semantic_memory_projection_status
             WHERE source_id NOT IN (SELECT id FROM sources)",
            [],
        )?;
    }
    if table_exists(db, "semantic_memory_links")? {
        repaired += db.conn().execute(
            "DELETE FROM semantic_memory_links
             WHERE COALESCE(gloss_chunk_id, chunk_id) NOT IN (SELECT id FROM chunks)
                OR source_id NOT IN (SELECT id FROM sources)
                OR (projection_unit_kind IS NOT NULL AND projection_unit_id IS NULL)",
            [],
        )?;
    }
    Ok(repaired)
}

fn count_failed_import_sources(db: &NotebookDb) -> Result<usize, GlossError> {
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM sources WHERE status = 'error'",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

fn count_unquarantined_failed_import_sources(db: &NotebookDb) -> Result<usize, GlossError> {
    let count: i64 = db.conn().query_row(
        "SELECT COUNT(*) FROM sources WHERE status = 'error' AND selected = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(count.max(0) as usize)
}

fn quarantine_failed_import_sources(db: &NotebookDb) -> Result<usize, GlossError> {
    let source_ids = {
        let mut stmt = db
            .conn()
            .prepare("SELECT id FROM sources WHERE status = 'error' AND selected = 1")?;
        let rows = stmt.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    if source_ids.is_empty() {
        return Ok(0);
    }
    let updated = db.conn().execute(
        "UPDATE sources
         SET selected = 0,
             updated_at = datetime('now')
         WHERE status = 'error' AND selected = 1",
        [],
    )?;
    for source_id in source_ids {
        db.conn().execute(
            "INSERT INTO source_processing_state (
                source_id, lifecycle_status, last_error, updated_at
             )
             SELECT id, 'quarantined_failed_import', error_message, datetime('now')
             FROM sources
             WHERE id = ?1
             ON CONFLICT(source_id) DO UPDATE SET
                lifecycle_status = excluded.lifecycle_status,
                last_error = excluded.last_error,
                updated_at = excluded.updated_at",
            [source_id],
        )?;
    }
    Ok(updated)
}

fn latest_doctor_receipt_id(
    db: &NotebookDb,
    notebook_id: &str,
) -> Result<Option<String>, GlossError> {
    if !table_exists(db, "provenance_receipts")? {
        return Ok(None);
    }
    db.conn()
        .query_row(
            "SELECT receipt_id
             FROM provenance_receipts
             WHERE operator_kind = 'db_doctor_repair'
               AND subject_kind = 'notebook'
               AND subject_id = ?1
             ORDER BY recorded_time DESC
             LIMIT 1",
            [notebook_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(GlossError::Database)
}

fn insert_doctor_receipt(
    db: &NotebookDb,
    receipt_id: &str,
    notebook_id: &str,
    supersedes_receipt_id: Option<&str>,
    raw_receipt: &serde_json::Value,
) -> Result<(), GlossError> {
    let raw_receipt_json =
        serde_json::to_string(raw_receipt).map_err(|error| GlossError::Other(error.to_string()))?;
    db.conn().execute(
        "INSERT INTO provenance_receipts (
            receipt_id, operator_kind, subject_kind, subject_id,
            supersedes_receipt_id, raw_receipt_json
         )
         VALUES (?1, 'db_doctor_repair', 'notebook', ?2, ?3, ?4)",
        params![
            receipt_id,
            notebook_id,
            supersedes_receipt_id,
            raw_receipt_json
        ],
    )?;
    if let Some(previous) = supersedes_receipt_id {
        db.conn().execute(
            "UPDATE provenance_receipts
             SET invalidated_by_receipt_id = ?1,
                 valid_time_end = CURRENT_TIMESTAMP
             WHERE receipt_id = ?2",
            params![receipt_id, previous],
        )?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_db_doctor;
    use crate::db::app_db::AppDb;
    use crate::db::notebook_db::{Chunk, NotebookDb, SemanticMemoryProjectionStatusUpdate, Source};
    use tempfile::tempdir;

    fn text_source(id: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: id.to_string(),
            original_filename: None,
            file_hash: None,
            url: None,
            file_path: None,
            content_text: Some("content".to_string()),
            word_count: Some(1),
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
            processing_state: None,
        }
    }

    #[test]
    fn db_doctor_detects_orphans_without_repairing_in_check_mode() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        app_db
            .create_notebook("nb1", "Doctor", &notebook_dir.to_string_lossy())
            .unwrap();
        app_db.update_source_count("nb1", 7).unwrap();

        let notebook_db = NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();
        notebook_db.insert_source(&text_source("s1")).unwrap();
        notebook_db
            .conn()
            .execute(
                "INSERT INTO semantic_memory_links (
                    chunk_id, notebook_id, source_id, content_digest, backend_version, sync_status, synced_at
                 )
                 VALUES ('missing-chunk', 'nb1', 'missing-source', 'digest', 'test', 'synced', datetime('now'))",
                [],
            )
            .unwrap();

        let receipt = run_db_doctor(&app_db, false).unwrap();
        assert_eq!(receipt.schema, "DbDoctorReceiptV1");
        assert_eq!(receipt.repaired_source_count_mismatches, 0);
        assert!(receipt
            .findings
            .iter()
            .any(|finding| finding.code == "source_count_mismatch" && !finding.repaired));
        assert!(receipt
            .findings
            .iter()
            .any(|finding| finding.code == "orphan_semantic_memory_links" && !finding.repaired));
        assert_eq!(app_db.get_notebook("nb1").unwrap().source_count, 7);
    }

    #[test]
    fn db_doctor_repairs_orphans_and_supersedes_prior_receipt() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        app_db
            .create_notebook("nb1", "Doctor", &notebook_dir.to_string_lossy())
            .unwrap();
        app_db.update_source_count("nb1", 7).unwrap();

        let notebook_db = NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();
        notebook_db.insert_source(&text_source("s1")).unwrap();
        notebook_db
            .conn()
            .execute(
                "INSERT INTO semantic_memory_links (
                    chunk_id, notebook_id, source_id, content_digest, backend_version, sync_status, synced_at
                 )
                 VALUES ('missing-chunk', 'nb1', 'missing-source', 'digest', 'test', 'synced', datetime('now'))",
                [],
            )
            .unwrap();

        let first = run_db_doctor(&app_db, true).unwrap();
        assert_eq!(first.repaired_source_count_mismatches, 1);
        assert_eq!(first.repaired_orphan_rows, 1);
        assert_eq!(app_db.get_notebook("nb1").unwrap().source_count, 1);

        notebook_db
            .conn()
            .execute(
                "INSERT INTO semantic_memory_links (
                    chunk_id, notebook_id, source_id, content_digest, backend_version, sync_status, synced_at
                 )
                 VALUES ('missing-again', 'nb1', 'missing-source', 'digest', 'test', 'synced', datetime('now'))",
                [],
            )
            .unwrap();
        app_db.update_source_count("nb1", 9).unwrap();

        let second = run_db_doctor(&app_db, true).unwrap();
        let report = second
            .notebook_reports
            .iter()
            .find(|report| report.notebook_id == "nb1")
            .unwrap();
        assert!(report.receipt_id.is_some());
        assert!(report.supersedes_receipt_id.is_some());
    }

    #[test]
    fn db_doctor_preserves_oversized_semantic_memory_projection_units() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        app_db
            .create_notebook("nb1", "Doctor", &notebook_dir.to_string_lossy())
            .unwrap();
        app_db.update_source_count("nb1", 1).unwrap();

        let notebook_db = NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();
        notebook_db
            .insert_source(&text_source("source-large"))
            .unwrap();
        notebook_db
            .insert_chunk(&Chunk {
                id: "chunk-parent".to_string(),
                source_id: "source-large".to_string(),
                chunk_index: 0,
                content: "oversized semantic memory content ".repeat(400),
                token_count: Some(1200),
                start_offset: Some(0),
                end_offset: Some(13_200),
                metadata: None,
                embedding_id: None,
                embedding_model: None,
            })
            .unwrap();
        for ordinal in 0..3 {
            notebook_db
                .conn()
                .execute(
                    "INSERT INTO semantic_memory_links (
                        chunk_id, gloss_chunk_id, notebook_id, source_id, sm_document_id,
                        sm_chunk_id, content_digest, backend_version, sync_status, synced_at,
                        projection_unit_id, projection_unit_kind, projection_unit_ordinal
                     )
                     VALUES (?1, 'chunk-parent', 'nb1', 'source-large', 'doc-large',
                        ?2, ?3, 'semantic-memory test', 'synced', datetime('now'),
                        ?1, 'projection_unit_subchunk', ?4)",
                    rusqlite::params![
                        format!("chunk-parent::projection-{ordinal}"),
                        format!("sm-subchunk-{ordinal}"),
                        format!("digest-{ordinal}"),
                        ordinal,
                    ],
                )
                .unwrap();
        }
        notebook_db
            .upsert_semantic_memory_projection_status(&SemanticMemoryProjectionStatusUpdate {
                notebook_id: "nb1".to_string(),
                source_id: "source-large".to_string(),
                status: "synced".to_string(),
                chunk_count: 1,
                projected_chunk_count: 1,
                healthy_link_count: 1,
                degraded_link_count: 0,
                last_receipt_id: Some("receipt-oversized".to_string()),
                last_error: None,
                artifact_generation_id: None,
                vector_artifact_manifest_digest: None,
            })
            .unwrap();

        let projection_unit_count: i64 = notebook_db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM semantic_memory_links
                 WHERE gloss_chunk_id = 'chunk-parent'
                   AND projection_unit_kind = 'projection_unit_subchunk'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(projection_unit_count, 3);
        let status = notebook_db
            .get_semantic_memory_projection_status("nb1", "source-large")
            .unwrap()
            .unwrap();
        assert_eq!(status.chunk_count, 1);
        assert_eq!(status.healthy_link_count, 1);

        let check = run_db_doctor(&app_db, false).unwrap();
        assert_eq!(check.repaired_orphan_rows, 0);
        assert!(!check
            .findings
            .iter()
            .any(|finding| finding.code == "orphan_semantic_memory_links"));

        let repair = run_db_doctor(&app_db, true).unwrap();
        assert_eq!(repair.repaired_orphan_rows, 0);
        let remaining_projection_units: i64 = notebook_db
            .conn()
            .query_row(
                "SELECT COUNT(*) FROM semantic_memory_links
                 WHERE gloss_chunk_id = 'chunk-parent'
                   AND projection_unit_kind = 'projection_unit_subchunk'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining_projection_units, 3);
    }

    #[test]
    fn db_doctor_quarantines_failed_imports_without_removing_retry_state() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        app_db
            .create_notebook("nb1", "Doctor", &notebook_dir.to_string_lossy())
            .unwrap();

        let notebook_db = NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();
        let mut failed = text_source("failed");
        failed.status = "error".to_string();
        failed.error_message = Some("strict import failed".to_string());
        notebook_db.insert_source(&failed).unwrap();

        let check = run_db_doctor(&app_db, false).unwrap();
        assert_eq!(check.failed_import_sources, 1);
        assert_eq!(check.quarantined_failed_import_sources, 0);

        let repaired = run_db_doctor(&app_db, true).unwrap();
        assert_eq!(repaired.failed_import_sources, 1);
        assert_eq!(repaired.quarantined_failed_import_sources, 1);
        let source = notebook_db.get_source("failed").unwrap();
        assert_eq!(source.status, "error");
        assert!(!source.selected);
        assert_eq!(
            source.processing_state.unwrap().lifecycle_status,
            "quarantined_failed_import"
        );
    }
}
