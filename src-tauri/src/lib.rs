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
use tauri::Manager;
use tracing_subscriber::EnvFilter;

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
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::notebooks::list_notebooks,
            commands::notebooks::create_notebook,
            commands::notebooks::delete_notebook,
            commands::sources::list_sources,
            commands::sources::add_source_file,
            commands::sources::add_source_folder,
            commands::sources::add_source_paste,
            commands::sources::delete_source,
            commands::sources::get_source_content,
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
