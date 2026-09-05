//! Actual DB, native dense and provider modules, with no source copying or stubs.
//! This does not compile the Tauri IPC, job runner, Candle or desktop runtime.
#![allow(dead_code)]
#[path = "../../../src-tauri/src/db/app_db.rs"]
pub mod app_db;
#[path = "../../../src-tauri/src/commands/chat/history.rs"]
pub mod chat_history;
#[path = "../../../src-tauri/src/error.rs"]
pub mod error;
#[path = "../../../src-tauri/src/memory/types.rs"]
pub mod memory_types;
#[path = "../../../src-tauri/src/db/migrations.rs"]
pub mod migrations;
#[path = "../../../src-tauri/src/db/notebook_db/mod.rs"]
pub mod notebook_db;
#[path = "../../../src-tauri/src/db/notebook_pool.rs"]
pub mod notebook_pool;
#[path = "../../../src-tauri/src/retrieval/source_scope.rs"]
pub mod source_scope;
pub mod memory {
    pub use crate::memory_types as types;
}
pub mod retrieval {
    pub use crate::source_scope;
}
pub mod db {
    pub use crate::{app_db, migrations, notebook_db, notebook_pool};
}
#[path = "../../../src-tauri/src/ingestion/chunk.rs"]
pub mod chunk;
#[path = "../../../src-tauri/src/ingestion/dense.rs"]
pub mod dense;
#[path = "../../../src-tauri/src/ingestion/embedding_contract.rs"]
pub mod embedding_contract;
#[path = "../../../src-tauri/src/features.rs"]
pub mod features;
#[path = "../../../src-tauri/src/ingestion/native_gates.rs"]
pub mod native_gates;
#[path = "../../../src-tauri/src/provider_config_store.rs"]
pub mod provider_config_store;
#[path = "../../../src-tauri/src/providers/mod.rs"]
pub mod providers;
#[path = "../../../src-tauri/src/redaction.rs"]
pub mod redaction;
#[path = "../../../src-tauri/src/settings_contract.rs"]
pub mod settings_contract;
pub(crate) use agent_queue as queue_core;
#[path = "../../../src-tauri/src/jobs/queue_policy.rs"]
pub mod queue_policy;
#[path = "../../../src-tauri/src/queue_task.rs"]
pub mod queue_task;
