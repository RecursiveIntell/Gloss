//! Process-boundary recovery using actual SQLite owners; not a desktop/runtime proof.
use gloss_native_contract_tests::notebook_db::NotebookDb;
use std::{collections::HashSet, process::Command};

#[test]
#[ignore = "spawned only by the bounded parent fixture"]
fn interrupted_writer_child() {
    let path = std::env::var("GLOSS_INTERRUPTED_FIXTURE").expect("disposable fixture path");
    let stage = std::env::var("GLOSS_INTERRUPTED_STAGE").unwrap();
    let db = NotebookDb::open(std::path::Path::new(&path)).unwrap();
    db.conn().execute("INSERT INTO sources(id,source_type,title,status) VALUES ('source','paste','Retained','pending')", []).unwrap();
    if stage != "registered" {
        db.update_source_content("source", "preserve this content", 3).unwrap();
    }
    if stage == "chunked" {
        db.conn().execute("INSERT INTO chunks(id,source_id,chunk_index,content) VALUES ('chunk','source',0,'preserve this content')", []).unwrap();
    }
    // Exit skips Connection's destructor/checkpoint, reproducing an interrupted process.
    std::process::exit(27);
}

#[test]
fn restart_recovers_committed_import_stages_without_deleting_canonical_content() {
    for stage in ["registered", "extracted", "chunked"] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notebook.db");
        let output = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "interrupted_writer_child", "--ignored", "--nocapture"])
            .env("GLOSS_INTERRUPTED_FIXTURE", &path).env("GLOSS_INTERRUPTED_STAGE", stage)
            .output().unwrap();
        assert_eq!(output.status.code(), Some(27), "{}", String::from_utf8_lossy(&output.stderr));
        let db = NotebookDb::connect(&path).unwrap();
        assert_eq!(db.get_source("source").unwrap().status, "pending");
        assert_eq!(db.recover_interrupted_ingestions(&HashSet::new()).unwrap(), 1);
        assert_eq!(db.recover_interrupted_ingestions(&HashSet::new()).unwrap(), 0);
        let source = db.get_source("source").unwrap();
        assert_eq!(source.status, "error");
        if stage != "registered" { assert_eq!(source.content_text.as_deref(), Some("preserve this content")); }
        if stage == "chunked" {
            let count: i64 = db.conn().query_row("SELECT COUNT(*) FROM chunks WHERE source_id='source'", [], |row| row.get(0)).unwrap();
            assert_eq!(count, 1);
        }
        // Explicit retry alone owns the destructive projection/chunk reset.
        db.reset_source_for_reingestion("notebook", "source").unwrap();
        assert_eq!(db.get_source("source").unwrap().status, "pending");
    }
}
