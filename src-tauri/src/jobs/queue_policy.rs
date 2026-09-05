//! Canonical persisted job identity, admission and queue cancellation policy.
use crate::queue_core::{QueueError, QueueManager};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Background jobs for Gloss.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum GlossJob {
    /// Generate a summary for a single source using the LLM.
    SummarizeSource {
        /// Explicit one-shot request, permitted even while automatic summaries are off.
        #[serde(default)]
        explicit_requested: bool,
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        ollama_url: String,
        model: String,
    },
    /// Describe an image using a vision-capable LLM.
    DescribeImage {
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        ollama_url: String,
        model: String,
        #[serde(default = "default_chunk_target_tokens")]
        chunk_target_tokens: usize,
    },
    /// Extract frames from a video and describe them using a vision model.
    DescribeVideo {
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        ollama_url: String,
        model: String,
        #[serde(default = "default_chunk_target_tokens")]
        chunk_target_tokens: usize,
    },
    /// Extract bounded audio metadata through ffprobe and cached Whisper transcript when available.
    ExtractAudioMetadata {
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        source_title: String,
        data_dir: String,
        #[serde(default = "default_chunk_target_tokens")]
        chunk_target_tokens: usize,
    },
    /// Background embedding/index job using the configured native provider.
    IndexChunks {
        #[serde(default)]
        epoch: u64,
        notebook_id: String,
        source_id: String,
        data_dir: String,
    },
}

fn default_chunk_target_tokens() -> usize {
    1100
}

/// Job ownership/cancellation requirements. The current queue core cannot claim
/// by kind, so the worker conservatively defers every job while no notebook is
/// selected, during chat grace, or during synchronous import. These flags do not
/// claim fully independent dispatch. Manual summary mode never vetoes ingestion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobResourcePolicy {
    pub requires_active_notebook: bool,
    pub respects_summary_pause: bool,
    pub respects_chat_grace: bool,
    pub requires_gpu_gate: bool,
    pub requires_llm_gate: bool,
}

impl JobResourcePolicy {
    const SUMMARY: Self = Self {
        requires_active_notebook: true,
        respects_summary_pause: true,
        respects_chat_grace: true,
        requires_gpu_gate: true,
        requires_llm_gate: true,
    };
    const INGESTION: Self = Self {
        requires_active_notebook: false,
        respects_summary_pause: false,
        respects_chat_grace: false,
        requires_gpu_gate: false,
        requires_llm_gate: false,
    };
}

impl GlossJob {
    pub fn resource_policy(&self) -> JobResourcePolicy {
        match self {
            GlossJob::SummarizeSource {
                explicit_requested, ..
            } => JobResourcePolicy {
                respects_summary_pause: !explicit_requested,
                ..JobResourcePolicy::SUMMARY
            },
            GlossJob::DescribeImage { .. }
            | GlossJob::DescribeVideo { .. }
            | GlossJob::ExtractAudioMetadata { .. }
            | GlossJob::IndexChunks { .. } => JobResourcePolicy::INGESTION,
        }
    }

    pub fn notebook_id(&self) -> &str {
        match self {
            GlossJob::SummarizeSource { notebook_id, .. }
            | GlossJob::DescribeImage { notebook_id, .. }
            | GlossJob::DescribeVideo { notebook_id, .. }
            | GlossJob::ExtractAudioMetadata { notebook_id, .. }
            | GlossJob::IndexChunks { notebook_id, .. } => notebook_id,
        }
    }

    pub fn source_id(&self) -> &str {
        match self {
            GlossJob::SummarizeSource { source_id, .. }
            | GlossJob::DescribeImage { source_id, .. }
            | GlossJob::DescribeVideo { source_id, .. }
            | GlossJob::ExtractAudioMetadata { source_id, .. }
            | GlossJob::IndexChunks { source_id, .. } => source_id,
        }
    }

    pub fn epoch(&self) -> u64 {
        match self {
            GlossJob::SummarizeSource { epoch, .. }
            | GlossJob::DescribeImage { epoch, .. }
            | GlossJob::DescribeVideo { epoch, .. }
            | GlossJob::ExtractAudioMetadata { epoch, .. }
            | GlossJob::IndexChunks { epoch, .. } => *epoch,
        }
    }
}

pub fn summary_allowed_by_mode(
    db: &crate::db::app_db::AppDb,
    explicit_requested: bool,
) -> Result<bool, crate::error::GlossError> {
    match db
        .get_setting("summary_mode")?
        .as_deref()
        .unwrap_or("manual")
    {
        "auto" => Ok(true),
        "manual" => Ok(explicit_requested),
        _ => Err(crate::error::GlossError::Config(
            "Invalid summary mode".into(),
        )),
    }
}

pub fn summary_dispatch_allowed(
    data_dir: &std::path::Path,
    explicit_requested: bool,
) -> Result<bool, crate::error::GlossError> {
    let db = crate::db::app_db::AppDb::open(&data_dir.join("gloss.db"))?;
    summary_allowed_by_mode(&db, explicit_requested)
}

pub fn cancel_summary_jobs(queue: &Arc<QueueManager>) -> Result<u32, QueueError> {
    let mut cancelled = 0;
    for (id, status, data) in queue.list_jobs_with_data()? {
        if !matches!(status.as_str(), "pending" | "processing") {
            continue;
        }
        let job: GlossJob = serde_json::from_str(&data)
            .map_err(|error| QueueError::Execution(error.to_string()))?;
        if matches!(job, GlossJob::SummarizeSource { .. }) {
            queue.cancel(&id)?;
            cancelled += 1;
        }
    }
    Ok(cancelled)
}

pub fn cancel_disallowed_auto_summaries(queue: &Arc<QueueManager>, automatic_enabled: bool) -> u32 {
    if automatic_enabled {
        return 0;
    }
    cancel_jobs_matching(queue, |job, _| {
        matches!(
            job,
            GlossJob::SummarizeSource {
                explicit_requested: false,
                ..
            }
        )
    })
}

pub fn has_summary_for_source(
    queue: &Arc<QueueManager>,
    notebook_id: &str,
    source_id: &str,
) -> Result<bool, QueueError> {
    for (_, status, data) in queue.list_jobs_with_data()? {
        if !matches!(status.as_str(), "pending" | "processing") {
            continue;
        }
        let job: GlossJob = serde_json::from_str(&data)
            .map_err(|error| QueueError::Execution(error.to_string()))?;
        if matches!(job, GlossJob::SummarizeSource { .. })
            && job.notebook_id() == notebook_id
            && job.source_id() == source_id
        {
            return Ok(true);
        }
    }
    Ok(false)
}

pub(crate) fn cancel_jobs_matching<F>(queue: &Arc<QueueManager>, mut should_cancel: F) -> u32
where
    F: FnMut(&GlossJob, &str) -> bool,
{
    let jobs = match queue.list_jobs_with_data() {
        Ok(jobs) => jobs,
        Err(e) => {
            tracing::warn!(error = %e, "Failed to inspect queue jobs");
            return 0;
        }
    };

    let mut cancelled = 0u32;
    for (job_id, status, data_json) in jobs {
        if !matches!(status.as_str(), "pending" | "processing") {
            continue;
        }

        let job = match serde_json::from_str::<GlossJob>(&data_json) {
            Ok(job) => job,
            Err(e) => {
                tracing::warn!(job_id, error = %e, "Failed to deserialize queue job");
                continue;
            }
        };

        if !should_cancel(&job, &status) {
            continue;
        }

        match queue.cancel(&job_id) {
            Ok(()) => cancelled += 1,
            Err(e) => tracing::debug!(job_id, error = %e, "Queue cancellation skipped"),
        }
    }

    cancelled
}

pub(crate) fn cancel_jobs_not_matching_active_notebook(
    queue: &Arc<QueueManager>,
    active_notebook_id: Option<&str>,
    active_epoch: u64,
) -> u32 {
    cancel_jobs_matching(queue, |job, _status| match active_notebook_id {
        Some(active_notebook_id) => {
            job.resource_policy().requires_active_notebook
                && (job.notebook_id() != active_notebook_id || job.epoch() != active_epoch)
        }
        None => job.resource_policy().requires_active_notebook,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::app_db::AppDb;
    use crate::queue_core::{db as queue_db, QueueConfig};
    use rusqlite::Connection;

    fn summary(explicit_requested: bool, source: &str) -> GlossJob {
        GlossJob::SummarizeSource {
            explicit_requested,
            epoch: 1,
            notebook_id: "nb".into(),
            source_id: source.into(),
            source_title: source.into(),
            data_dir: "/fixture".into(),
            ollama_url: "http://localhost:11434".into(),
            model: "fixture".into(),
        }
    }

    fn queue_fixture() -> (tempfile::TempDir, Arc<QueueManager>, Connection) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("queue.db");
        let queue = Arc::new(
            QueueManager::new(QueueConfig::builder().with_db_path(path.clone()).build()).unwrap(),
        );
        let conn = Connection::open(path).unwrap();
        (dir, queue, conn)
    }

    // Use the canonical queue DB admission primitive with the real Gloss
    // payload. No replacement JobHandler or simulated inference is involved.
    fn insert(conn: &Connection, id: &str, job: &GlossJob) {
        queue_db::insert_job_full(
            conn,
            id,
            2,
            &serde_json::to_value(job).unwrap(),
            None,
            None,
            None,
        )
        .unwrap();
    }

    fn ingestion(source: &str) -> GlossJob {
        GlossJob::IndexChunks {
            epoch: 0,
            notebook_id: "other".into(),
            source_id: source.into(),
            data_dir: "/fixture".into(),
        }
    }

    #[test]
    fn legacy_summary_payload_is_automatic_not_an_explicit_request() {
        let mut payload = serde_json::to_value(summary(true, "source")).unwrap();
        payload["SummarizeSource"]
            .as_object_mut()
            .unwrap()
            .remove("explicit_requested");
        let legacy: GlossJob = serde_json::from_value(payload).unwrap();
        assert!(matches!(
            legacy,
            GlossJob::SummarizeSource {
                explicit_requested: false,
                ..
            }
        ));
        assert!(legacy.resource_policy().respects_summary_pause);
        assert!(
            !summary(true, "source")
                .resource_policy()
                .respects_summary_pause
        );
        assert!(!ingestion("source").resource_policy().respects_summary_pause);
    }

    #[test]
    fn manual_mode_rejects_auto_but_preserves_explicit_and_media_claims() {
        let (dir, queue, conn) = queue_fixture();
        let db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        db.set_setting("summary_mode", "manual").unwrap();
        assert!(!summary_allowed_by_mode(&db, false).unwrap());
        assert!(summary_allowed_by_mode(&db, true).unwrap());
        insert(&conn, "auto", &summary(false, "auto"));
        insert(&conn, "explicit", &summary(true, "explicit"));
        insert(&conn, "index", &ingestion("index"));
        let media = GlossJob::DescribeImage {
            epoch: 0,
            notebook_id: "other".into(),
            source_id: "image".into(),
            source_title: "image".into(),
            data_dir: "/fixture".into(),
            ollama_url: "http://localhost:11434".into(),
            model: "fixture".into(),
            chunk_target_tokens: 1100,
        };
        insert(&conn, "media", &media);
        assert_eq!(cancel_disallowed_auto_summaries(&queue, false), 1);
        let states: std::collections::HashMap<_, _> =
            queue.list_jobs().unwrap().into_iter().collect();
        assert_eq!(states["auto"], "cancelled");
        for id in ["explicit", "index", "media"] {
            assert_eq!(states[id], "pending");
        }
        let (id, _) = queue_db::claim_next_job(&conn).unwrap().unwrap();
        assert_ne!(id, "auto");
        assert_eq!(
            db.get_setting("summary_mode").unwrap().as_deref(),
            Some("manual")
        );
    }

    #[test]
    fn pause_cancels_pending_and_claimed_summaries_only() {
        let (_dir, queue, conn) = queue_fixture();
        insert(&conn, "explicit", &summary(true, "explicit"));
        assert_eq!(
            queue_db::claim_next_job(&conn).unwrap().unwrap().0,
            "explicit"
        );
        insert(&conn, "auto", &summary(false, "auto"));
        insert(&conn, "index", &ingestion("index"));
        let audio = GlossJob::ExtractAudioMetadata {
            epoch: 0,
            notebook_id: "other".into(),
            source_id: "audio".into(),
            source_title: "audio".into(),
            data_dir: "/fixture".into(),
            chunk_target_tokens: 1100,
        };
        insert(&conn, "audio", &audio);
        assert_eq!(cancel_summary_jobs(&queue).unwrap(), 2);
        let states: std::collections::HashMap<_, _> =
            queue.list_jobs().unwrap().into_iter().collect();
        assert_eq!(states["explicit"], "cancelled");
        assert_eq!(states["auto"], "cancelled");
        assert_eq!(states["index"], "pending");
        assert_eq!(states["audio"], "pending");
    }

    #[test]
    fn mode_change_before_dispatch_and_notebook_switch_fail_closed_for_summaries() {
        let (dir, queue, conn) = queue_fixture();
        let db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        db.set_setting("summary_mode", "auto").unwrap();
        assert!(summary_dispatch_allowed(dir.path(), false).unwrap());
        db.set_setting("summary_mode", "manual").unwrap();
        assert!(!summary_dispatch_allowed(dir.path(), false).unwrap());
        assert!(summary_dispatch_allowed(dir.path(), true).unwrap());
        insert(&conn, "explicit", &summary(true, "explicit"));
        insert(&conn, "index", &ingestion("index"));
        assert_eq!(
            cancel_jobs_not_matching_active_notebook(&queue, Some("new"), 2),
            1
        );
        let states: std::collections::HashMap<_, _> =
            queue.list_jobs().unwrap().into_iter().collect();
        assert_eq!(states["index"], "pending");
    }

    #[test]
    fn unrelated_ingestion_does_not_block_per_source_summary_admission() {
        let (_dir, queue, conn) = queue_fixture();
        insert(&conn, "index", &ingestion("index"));
        assert!(!has_summary_for_source(&queue, "nb", "summary").unwrap());
        insert(&conn, "summary", &summary(true, "summary"));
        assert!(has_summary_for_source(&queue, "nb", "summary").unwrap());
        assert!(!has_summary_for_source(&queue, "nb", "another").unwrap());
    }
}
