//! Actual DB, native dense and SSE modules, with no source copying or stubs.
//! This does not compile the Tauri IPC, job runner, Candle or desktop runtime.
#![allow(dead_code)]
#[path = "../../../src-tauri/src/db/app_db.rs"]
pub mod app_db;
#[path = "../../../src-tauri/src/error.rs"]
pub mod error;
#[path = "../../../src-tauri/src/memory/types.rs"]
pub mod memory_types;
#[path = "../../../src-tauri/src/db/migrations.rs"]
pub mod migrations;
#[path = "../../../src-tauri/src/db/notebook_db/mod.rs"]
pub mod notebook_db;
#[path = "../../../src-tauri/src/retrieval/source_scope.rs"]
pub mod source_scope;
pub mod memory {
    pub use crate::memory_types as types;
}
pub mod retrieval {
    pub use crate::source_scope;
}
pub mod db {
    pub use crate::{app_db, migrations, notebook_db};
}
#[path = "../../../src-tauri/src/ingestion/dense.rs"]
pub mod dense;
#[path = "../../../src-tauri/src/providers/sse.rs"]
pub mod sse;
