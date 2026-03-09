// Phase 1: Many modules have code that will be wired in later steps
#![allow(dead_code)]

mod commands;
mod db;
mod error;
mod ingestion;
mod jobs;
mod providers;
mod retrieval;
mod state;
mod studio;

use state::AppState;
use std::sync::Arc;
use std::time::Duration;
use tauri::Manager;
use tauri_queue::{QueueConfig, QueueEventEmitter, QueueManager, TauriEventEmitter};
use tracing_subscriber::EnvFilter;

/// Idle duration (seconds) after which auto-summarization kicks in.
const IDLE_AUTO_SUMMARIZE_SECS: u64 = 600; // 10 minutes

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
            commands::notebooks::delete_notebook,
            commands::notebooks::set_active_notebook,
            commands::sources::list_sources,
            commands::sources::add_source_file,
            commands::sources::add_source_folder,
            commands::sources::add_source_paste,
            commands::sources::delete_source,
            commands::sources::get_source_content,
            commands::sources::retry_source_ingestion,
            commands::sources::get_notebook_stats,
            commands::sources::regenerate_missing_summaries,
            commands::sources::pause_summaries,
            commands::sources::resume_summaries,
            commands::sources::get_queue_status,
            commands::chat::list_conversations,
            commands::chat::create_conversation,
            commands::chat::delete_conversation,
            commands::chat::load_messages,
            commands::chat::send_message,
            commands::chat::get_suggested_questions,
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
/// 3. **Single-flight LLM gate** — acquires `llm_gate` semaphore before LLM work,
///    so chat (which also acquires it) naturally serializes with summaries.
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

        // 4. Acquire LLM gate before processing (ensures no concurrent GPU inference).
        //    We acquire here so that if chat arrives while we wait, the chat will
        //    also try to acquire and one will block. The semaphore enforces ordering.
        let permit = state.llm_gate.acquire().await;
        let permit = match permit {
            Ok(p) => p,
            Err(_) => {
                // Semaphore closed — app is shutting down
                tracing::info!("LLM gate closed, summary loop exiting");
                return;
            }
        };

        // Also acquire GPU gate to prevent running while embedding is active
        let gpu_permit = match state.gpu_gate.acquire().await {
            Ok(p) => p,
            Err(_) => {
                drop(permit);
                tracing::info!("GPU gate closed, summary loop exiting");
                return;
            }
        };

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

                    // Check if this was a DescribeImage/Video job that needs follow-up embedding
                    if let Some(ref output_str) = job.output {
                        if let Ok(output) = serde_json::from_str::<serde_json::Value>(output_str) {
                            if output.get("needs_embedding").and_then(|v| v.as_bool()) == Some(true)
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
                                            "Skipping follow-up embedding for stale job"
                                        );
                                        drop(gpu_permit);
                                        drop(permit);
                                        tokio::time::sleep(Duration::from_millis(250)).await;
                                        continue;
                                    }

                                    tracing::info!(
                                        source_id = src_id,
                                        "Running embedding for described source"
                                    );

                                    // Drop LLM gate — embedding doesn't need LLM
                                    drop(permit);

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
                                        commands::sources::embed_described_source(
                                            &state, &nb, &src, &handle2, &q2,
                                        );
                                    })
                                    .await;
                                    if let Err(e) = embed_result {
                                        tracing::error!(
                                            source_id = %src_for_err,
                                            error = %e,
                                            "embed_described_source panicked"
                                        );
                                        let _ = state.with_notebook_db(&nb_for_err, |db| {
                                            db.update_source_status(
                                                &src_for_err,
                                                "error",
                                                Some(&format!("Embedding panicked: {}", e)),
                                            )
                                        });
                                    }

                                    drop(gpu_permit);
                                    // permit already dropped above
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
                drop(gpu_permit);
                drop(permit);
                // Cool-down between jobs to prevent GPU thermal throttling / CUDA errors
                tokio::time::sleep(Duration::from_secs(3)).await;
            }
            Ok(Ok(None)) => {
                drop(gpu_permit);
                drop(permit);

                // No pending jobs — check if we should auto-queue missing summaries.
                // Triggers when user has been idle for 10+ minutes.
                let idle_secs = state.idle_seconds();
                if idle_secs >= IDLE_AUTO_SUMMARIZE_SECS {
                    if let Some(ref nb_id) = active_nb {
                        let queued =
                            commands::sources::auto_queue_notebook_summaries(&state, &queue, nb_id);
                        if queued > 0 {
                            tracing::info!(
                                idle_secs,
                                queued,
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
                drop(gpu_permit);
                drop(permit);
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}
