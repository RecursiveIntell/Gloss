// Phase 1: Many modules have code that will be wired in later steps
#![allow(dead_code)]

mod commands;
mod db;
mod error;
mod ingestion;
mod jobs;
mod memory;
mod provider_config_store;
mod providers;
mod retrieval;
mod state;
mod studio;

use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;
use tauri_queue::{QueueConfig, QueueEventEmitter, QueueManager, TauriEventEmitter};
use tokio::sync::{Semaphore, SemaphorePermit, TryAcquireError};
use tracing_subscriber::EnvFilter;

/// Idle duration (seconds) after which auto-summarization kicks in.
const IDLE_AUTO_SUMMARIZE_SECS: u64 = 600; // 10 minutes
const SUMMARY_LLM_GATE_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SummaryGateWait {
    Acquired,
    ReleaseForChat,
    Timeout,
    Closed,
}

async fn acquire_summary_llm_gate_while_holding_gpu<'a, F>(
    llm_gate: &'a Semaphore,
    mut should_release_gpu: F,
    timeout: Duration,
) -> (SummaryGateWait, Option<SemaphorePermit<'a>>)
where
    F: FnMut() -> bool,
{
    let started = std::time::Instant::now();
    loop {
        if should_release_gpu() {
            return (SummaryGateWait::ReleaseForChat, None);
        }

        match llm_gate.try_acquire() {
            Ok(permit) => return (SummaryGateWait::Acquired, Some(permit)),
            Err(TryAcquireError::Closed) => return (SummaryGateWait::Closed, None),
            Err(TryAcquireError::NoPermits) => {
                if started.elapsed() >= timeout {
                    return (SummaryGateWait::Timeout, None);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("gloss=debug,tauri_queue=info")),
        )
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let app_handle = app.handle().clone();
            let state = AppState::initialize(&app_handle)?;

            // Initialize job queue with persistent SQLite storage
            let config = QueueConfig::builder()
                .with_db_path(state.data_dir.join("queue.db"))
                .with_poll_interval(Duration::from_secs(3))
                .build();
            let queue =
                QueueManager::new(config).map_err(|e| Box::new(e) as Box<dyn std::error::Error>)?;

            // Prune completed/failed/cancelled jobs older than 7 days to prevent
            // unbounded queue.db growth which slows count_by_status and adds
            // mutex contention with ingestion and the summary loop.
            match queue.prune(7) {
                Ok(pruned) if pruned > 0 => {
                    tracing::info!(pruned, "Pruned old queue jobs on startup");
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to prune old queue jobs");
                }
                _ => {}
            }

            let event_emitter = TauriEventEmitter::arc(app_handle.clone());

            // Don't use queue.spawn() — we drive the loop ourselves so we can
            // check active_chats before processing each job. This prevents
            // starting a summary while chat is active (Ollama serializes requests).
            let queue = Arc::new(queue);

            app.manage(state);
            app.manage(Arc::clone(&queue));

            // Spawn custom job processing loop on Tauri's async runtime
            let q = Arc::clone(&queue);
            tauri::async_runtime::spawn(async move {
                summary_job_loop(q, event_emitter, app_handle).await;
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::notebooks::list_notebooks,
            commands::notebooks::create_notebook,
            commands::notebooks::rename_notebook,
            commands::notebooks::delete_notebook,
            commands::notebooks::set_active_notebook,
            commands::sources::list_sources,
            commands::sources::set_selected_sources,
            commands::sources::add_source_file,
            commands::sources::add_source_files,
            commands::sources::add_source_folder,
            commands::sources::add_source_paste,
            commands::sources::delete_source,
            commands::sources::delete_sources,
            commands::sources::get_source_content,
            commands::sources::retry_source_ingestion,
            commands::sources::get_notebook_stats,
            commands::sources::regenerate_missing_summaries,
            commands::sources::pause_summaries,
            commands::sources::resume_summaries,
            commands::sources::get_queue_status,
            commands::sources::memory_backend_status,
            commands::sources::semantic_memory_link_status,
            commands::sources::semantic_memory_reindex_notebook,
            commands::sources::semantic_memory_reindex_source,
            commands::sources::compare_memory_backends,
            commands::chat::list_conversations,
            commands::chat::create_conversation,
            commands::chat::delete_conversation,
            commands::chat::stop_chat,
            commands::chat::load_messages,
            commands::chat::send_message,
            commands::chat::get_suggested_questions,
            commands::chat::debug_chat_provider_smoke,
            commands::chat::get_last_chat_attempt_trace,
            commands::notes::list_notes,
            commands::notes::create_note,
            commands::notes::save_response_as_note,
            commands::notes::update_note,
            commands::notes::toggle_pin,
            commands::notes::delete_note,
            commands::settings::get_providers,
            commands::settings::update_provider,
            commands::settings::test_provider,
            commands::settings::refresh_models,
            commands::settings::get_all_models,
            commands::settings::get_settings,
            commands::settings::update_setting,
            commands::settings::check_external_tools,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

/// Custom job processing loop that enforces all scheduling contracts:
///
/// 1. **No summaries before notebook selection** — idles when active_notebook_id is None.
/// 2. **Chat grace window** — does not start a summary within 15s of the last user message.
/// 3. **Single-flight LLM gate** — acquires `llm_gate` only after `gpu_gate`,
///    records gate owner state, and releases both gates at safe boundaries so
///    chat can disclose who owns contested runtime capacity.
/// 4. **Notebook switching** — validates job notebook_id + epoch before executing.
/// 5. **Manual pause** — respects `summary_paused` flag.
async fn summary_job_loop(
    queue: Arc<QueueManager>,
    emitter: Arc<dyn QueueEventEmitter>,
    handle: tauri::AppHandle,
) {
    // Brief delay to let the app finish startup
    tokio::time::sleep(Duration::from_secs(5)).await;
    tracing::info!("Summary job loop started");

    loop {
        let state = handle.state::<AppState>();

        // 1. No summaries before notebook selection
        let active_nb = state.get_active_notebook_id();
        if active_nb.is_none() {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }

        // 2. Manual pause
        if state
            .summary_paused
            .load(std::sync::atomic::Ordering::SeqCst)
        {
            tokio::time::sleep(Duration::from_secs(2)).await;
            continue;
        }

        // 3. Chat grace window — don't start a new summary within 15s of last user message
        if state.is_in_chat_grace() {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        // 3b. Don't start summaries while sources are actively being ingested.
        // The atomic counter is set by run_ingestion_inner (text/code sources).
        // The DB check catches image/video sources in 'describing'/'described' status.
        if state
            .ingestion_active
            .load(std::sync::atomic::Ordering::SeqCst)
            > 0
        {
            tracing::debug!(
                active = state
                    .ingestion_active
                    .load(std::sync::atomic::Ordering::SeqCst),
                "Ingestion active, deferring summaries"
            );
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        }
        if let Some(ref nb_id) = active_nb {
            let nb_id_clone = nb_id.clone();
            let handle_clone = handle.clone();
            let has_describing = tokio::task::spawn_blocking(move || {
                let state = handle_clone.state::<AppState>();
                state
                    .with_notebook_db(&nb_id_clone, |db| {
                        let count: i64 = db.conn.query_row(
                        "SELECT COUNT(*) FROM sources WHERE status IN ('describing', 'described')",
                        [],
                        |row| row.get(0),
                    ).unwrap_or(0);
                        Ok(count > 0)
                    })
                    .unwrap_or(false)
            })
            .await
            .unwrap_or(false);
            if has_describing {
                tokio::time::sleep(Duration::from_secs(5)).await;
                continue;
            }
        }

        let active_epoch = state.get_active_epoch();
        let cancelled = jobs::cancel_jobs_not_matching_active_notebook(
            &queue,
            active_nb.as_deref(),
            active_epoch,
        );
        if cancelled > 0 {
            tracing::info!(cancelled, "Cancelled stale queue jobs before processing");
        }

        // 4. Acquire GPU before LLM so a background job never holds LLM capacity
        // while merely waiting for GPU.
        let gpu_permit = match state.gpu_gate.acquire().await {
            Ok(p) => p,
            Err(_) => {
                tracing::info!("GPU gate closed, summary loop exiting");
                return;
            }
        };
        state.set_gate_owner("GPU gate", "background_summary", "queue.process_one");

        if state.is_in_chat_grace() {
            state.clear_gate_owner("GPU gate", "background_summary");
            drop(gpu_permit);
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }

        let (llm_gate_wait, permit) = acquire_summary_llm_gate_while_holding_gpu(
            &state.llm_gate,
            || {
                state
                    .summary_paused
                    .load(std::sync::atomic::Ordering::SeqCst)
                    || state.is_in_chat_grace()
                    || active_nb
                        .as_deref()
                        .map(|nb_id| !state.is_active_notebook_epoch(nb_id, active_epoch))
                        .unwrap_or(true)
            },
            SUMMARY_LLM_GATE_WAIT_TIMEOUT,
        )
        .await;
        let permit = match (llm_gate_wait, permit) {
            (SummaryGateWait::Acquired, Some(permit)) => permit,
            (SummaryGateWait::Closed, _) => {
                state.clear_gate_owner("GPU gate", "background_summary");
                drop(gpu_permit);
                tracing::info!("LLM gate closed, summary loop exiting");
                return;
            }
            (SummaryGateWait::ReleaseForChat, _) => {
                state.clear_gate_owner("GPU gate", "background_summary");
                drop(gpu_permit);
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            (SummaryGateWait::Timeout, _) => {
                state.clear_gate_owner("GPU gate", "background_summary");
                drop(gpu_permit);
                tracing::debug!("Released GPU gate after summary waited for LLM gate slice");
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
            (SummaryGateWait::Acquired, None) => {
                state.clear_gate_owner("GPU gate", "background_summary");
                drop(gpu_permit);
                tracing::warn!("LLM gate reported acquired without a permit");
                tokio::time::sleep(Duration::from_millis(250)).await;
                continue;
            }
        };
        state.set_gate_owner("LLM gate", "background_summary", "queue.process_one");

        // Re-check conditions after acquiring the permit (chat may have arrived
        // while we were waiting, or notebook may have changed).
        if state
            .summary_paused
            .load(std::sync::atomic::Ordering::SeqCst)
            || state.is_in_chat_grace()
            || active_nb
                .as_deref()
                .map(|nb_id| !state.is_active_notebook_epoch(nb_id, active_epoch))
                .unwrap_or(true)
        {
            state.clear_gate_owner("GPU gate", "background_summary");
            state.clear_gate_owner("LLM gate", "background_summary");
            drop(gpu_permit);
            drop(permit);
            tokio::time::sleep(Duration::from_millis(250)).await;
            continue;
        }

        // Snapshot epoch before processing
        let epoch_before = state.get_active_epoch();

        let job_result = tokio::time::timeout(
            Duration::from_secs(180),
            queue.process_one::<jobs::GlossJob>(&emitter),
        )
        .await;

        match job_result {
            Err(_timeout) => {
                tracing::error!("Summary job timed out after 180s — releasing LLM gate");
                state.clear_gate_owner("GPU gate", "background_summary");
                state.clear_gate_owner("LLM gate", "background_summary");
                drop(gpu_permit);
                drop(permit);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
            Ok(Ok(Some(job))) => {
                let job_meta = serde_json::from_value::<jobs::GlossJob>(job.job_data.clone()).ok();

                // Validate that the job was for the active notebook + epoch
                let epoch_after = state.get_active_epoch();
                if epoch_after != epoch_before {
                    tracing::debug!(
                        job_id = %job.job_id,
                        "Notebook changed during summary job (epoch {} -> {}), result may be stale",
                        epoch_before, epoch_after
                    );
                }

                if job.success {
                    tracing::debug!(job_id = %job.job_id, "Summary job completed");

                    // Check if this was a DescribeImage/Video job that needs follow-up finalization
                    if let Some(ref output_str) = job.output {
                        if let Ok(output) = serde_json::from_str::<serde_json::Value>(output_str) {
                            if output.get("needs_finalization").and_then(|v| v.as_bool())
                                == Some(true)
                            {
                                let nb_id = output
                                    .get("notebook_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let src_id = output
                                    .get("source_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                if !nb_id.is_empty() && !src_id.is_empty() {
                                    let job_is_current = job_meta
                                        .as_ref()
                                        .map(|job| {
                                            state.is_active_notebook_epoch(
                                                job.notebook_id(),
                                                job.epoch(),
                                            )
                                        })
                                        .unwrap_or(false);
                                    let job_epoch =
                                        job_meta.as_ref().map(|job| job.epoch()).unwrap_or(0);
                                    if !job_is_current {
                                        tracing::info!(
                                            job_id = %job.job_id,
                                            notebook_id = nb_id,
                                            source_id = src_id,
                                            "Skipping follow-up finalization for stale job"
                                        );
                                        state.clear_gate_owner("GPU gate", "background_summary");
                                        state.clear_gate_owner("LLM gate", "background_summary");
                                        drop(gpu_permit);
                                        drop(permit);
                                        tokio::time::sleep(Duration::from_millis(250)).await;
                                        continue;
                                    }

                                    tracing::info!(
                                        source_id = src_id,
                                        "Finalizing described source"
                                    );

                                    // Drop runtime gates — described-source finalization only
                                    // updates DB status and queues a summary.
                                    state.clear_gate_owner("LLM gate", "background_summary");
                                    drop(permit);
                                    state.clear_gate_owner("GPU gate", "background_summary");
                                    drop(gpu_permit);

                                    let nb_for_err = nb_id.to_string();
                                    let src_for_err = src_id.to_string();
                                    let handle2 = handle.clone();
                                    let nb = nb_for_err.clone();
                                    let src = src_for_err.clone();
                                    let q2 = Arc::clone(&queue);
                                    let embed_result = tokio::task::spawn_blocking(move || {
                                        let state = handle2.state::<AppState>();
                                        if !state.is_active_notebook_epoch(&nb, job_epoch) {
                                            return;
                                        }
                                        commands::sources::finalize_described_source(
                                            &state, &nb, &src, &handle2, &q2,
                                        );
                                    })
                                    .await;
                                    if let Err(e) = embed_result {
                                        tracing::error!(
                                            source_id = %src_for_err,
                                            error = %e,
                                            "finalize_described_source panicked"
                                        );
                                        let _ = state.with_notebook_db(&nb_for_err, |db| {
                                            db.update_source_status(
                                                &src_for_err,
                                                "error",
                                                Some(&format!("Finalization panicked: {}", e)),
                                            )
                                        });
                                    }

                                    // permits already dropped above
                                    tokio::time::sleep(Duration::from_secs(3)).await;
                                    continue; // Skip the drops below
                                }
                            }
                        }
                    }
                } else {
                    tracing::warn!(
                        job_id = %job.job_id,
                        error = ?job.error,
                        "Summary job failed"
                    );
                }
                state.clear_gate_owner("GPU gate", "background_summary");
                state.clear_gate_owner("LLM gate", "background_summary");
                drop(gpu_permit);
                drop(permit);
                // Cool-down between jobs to prevent GPU thermal throttling / CUDA errors
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Ok(Ok(None)) => {
                state.clear_gate_owner("GPU gate", "background_summary");
                state.clear_gate_owner("LLM gate", "background_summary");
                drop(gpu_permit);
                drop(permit);

                // No pending jobs — check if we should auto-queue missing summaries.
                // Triggers when user has been idle for 10+ minutes.
                let idle_secs = state.idle_seconds();
                let auto_summary_enabled = match state.summary_mode_is_auto() {
                    Ok(enabled) => enabled,
                    Err(e) => {
                        tracing::warn!(error = %e, "Failed to read summary mode; skipping idle auto-queue");
                        false
                    }
                };
                if auto_summary_enabled && idle_secs >= IDLE_AUTO_SUMMARIZE_SECS {
                    if let Some(ref nb_id) = active_nb {
                        let queued_result =
                            commands::sources::auto_queue_notebook_summaries(&state, &queue, nb_id);
                        for diagnostic in &queued_result.diagnostics {
                            tracing::warn!(notebook_id = %nb_id, diagnostic = %diagnostic, "Summary auto-queue skipped");
                        }
                        if queued_result.queued > 0 {
                            tracing::info!(
                                idle_secs,
                                queued = queued_result.queued,
                                "Auto-queued summaries after idle period"
                            );
                            // Process immediately instead of sleeping
                            continue;
                        }
                    }
                }

                // Poll less frequently when idle
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Ok(Err(e)) => {
                tracing::error!("Job processing error: {}", e);
                state.clear_gate_owner("GPU gate", "background_summary");
                state.clear_gate_owner("LLM gate", "background_summary");
                drop(gpu_permit);
                drop(permit);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[tokio::test]
    async fn summary_llm_wait_releases_gpu_when_chat_arrives() {
        let gpu_gate = Semaphore::new(1);
        let llm_gate = Semaphore::new(1);
        let _llm_held_by_other_work = llm_gate.acquire().await.unwrap();
        let gpu_permit = gpu_gate.acquire().await.unwrap();
        let chat_arrived = AtomicBool::new(false);

        let wait = tokio::time::timeout(Duration::from_secs(1), async {
            let chat_arrived_ref = &chat_arrived;
            tokio::join!(
                async {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    chat_arrived_ref.store(true, Ordering::SeqCst);
                },
                async {
                    acquire_summary_llm_gate_while_holding_gpu(
                        &llm_gate,
                        || chat_arrived.load(Ordering::SeqCst),
                        Duration::from_secs(30),
                    )
                    .await
                }
            )
            .1
        })
        .await
        .expect("summary LLM wait should observe chat arrival promptly");

        assert_eq!(wait.0, SummaryGateWait::ReleaseForChat);
        assert!(wait.1.is_none());

        drop(gpu_permit);
        let chat_gpu = gpu_gate
            .try_acquire()
            .expect("chat should be able to acquire GPU after summary yields");
        drop(chat_gpu);
    }

    #[tokio::test]
    async fn summary_llm_wait_times_out_without_holding_gpu_forever() {
        let gpu_gate = Semaphore::new(1);
        let llm_gate = Semaphore::new(1);
        let _llm_held_by_other_work = llm_gate.acquire().await.unwrap();
        let gpu_permit = gpu_gate.acquire().await.unwrap();

        let (wait, permit) = acquire_summary_llm_gate_while_holding_gpu(
            &llm_gate,
            || false,
            Duration::from_millis(25),
        )
        .await;

        assert_eq!(wait, SummaryGateWait::Timeout);
        assert!(permit.is_none());

        drop(gpu_permit);
        let chat_gpu = gpu_gate
            .try_acquire()
            .expect("summary timeout path must release GPU for chat");
        drop(chat_gpu);
    }
}
