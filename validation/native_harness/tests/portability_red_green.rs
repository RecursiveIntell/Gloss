use gloss_native_contract_tests::{app_db::AppDb, doctor, notebook_db::NotebookDb, portable};
use sha2::{Digest, Sha256};
use tempfile::tempdir;

#[test]
fn live_wal_saved_note_survives_export() {
    let root = tempdir().unwrap();
    let app = AppDb::open(&root.path().join("app.db")).unwrap();
    let original = root.path().join("original");
    std::fs::create_dir(&original).unwrap();
    app.create_notebook("n", "Notebook", &original.to_string_lossy())
        .unwrap();
    let db = NotebookDb::open(&original.join("notebook.db")).unwrap();
    db.conn().execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint=0; INSERT INTO notes (id, content, note_type) VALUES ('n', 'committed after checkpoint', 'manual');").unwrap();
    let package = root.path().join("package");
    portable::export_notebook_package(&app, "n", &package).unwrap();
    let snapshot = rusqlite::Connection::open(package.join("notebook.db")).unwrap();
    assert_eq!(
        snapshot
            .query_row("SELECT COUNT(*) FROM notes", [], |r| r.get::<_, i64>(0))
            .unwrap(),
        1
    );
}

#[test]
fn empty_manifest_does_not_validate_missing_database() {
    let root = tempdir().unwrap();
    let manifest = serde_json::json!({"schema":"NotebookPortableManifestV1", "package_id":"p", "exported_utc":"fixture", "source_notebook_id":"s", "notebook_name":"N", "files":[], "manifest_digest":format!("{:x}", Sha256::digest(b""))});
    std::fs::write(
        root.path().join("manifest.json"),
        serde_json::to_vec(&manifest).unwrap(),
    )
    .unwrap();
    assert!(portable::validate_notebook_package(root.path()).is_err());
}

#[test]
fn diagnostic_check_does_not_commit_a_database_write() {
    let root = tempdir().unwrap();
    let app = AppDb::open(&root.path().join("app.db")).unwrap();
    let original = root.path().join("original");
    std::fs::create_dir(&original).unwrap();
    app.create_notebook("n", "Notebook", &original.to_string_lossy())
        .unwrap();
    let db = NotebookDb::open(&original.join("notebook.db")).unwrap();
    let observer = rusqlite::Connection::open(original.join("notebook.db")).unwrap();
    let before: i64 = observer
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    doctor::run_db_doctor(&app, false).unwrap();
    let after: i64 = observer
        .query_row("PRAGMA data_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(before, after);
    drop(db);
}
