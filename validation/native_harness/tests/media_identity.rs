//! Imported source IDs are opaque metadata. This tests actual package/DB and
//! scratch owners; it does not invoke ffmpeg, Whisper, or the Tauri job runner.
use gloss_native_contract_tests::{
    app_db::AppDb, media_workspace::{create_media_workspace, MediaWorkspaceKind},
    notebook_db::NotebookDb, portable::{export_notebook_package, import_notebook_package},
};
use std::{fs, path::Path};

#[test]
fn imported_absolute_source_identity_cannot_choose_a_media_cleanup_target() {
    let root = tempfile::tempdir().unwrap();
    let victim = root.path().join("unrelated-user-folder");
    fs::create_dir(&victim).unwrap();
    fs::write(victim.join("sentinel"), "retain unrelated data").unwrap();
    let source_id = victim.to_string_lossy().to_string();
    let app = AppDb::open(&root.path().join("app.db")).unwrap();
    let original = root.path().join("original");
    fs::create_dir_all(original.join("sources")).unwrap();
    fs::write(original.join("sources/video.mp4"), "disposable fixture").unwrap();
    app.create_notebook("original", "Original", &original.to_string_lossy()).unwrap();
    let db = NotebookDb::open(&original.join("notebook.db")).unwrap();
    db.conn().execute("INSERT INTO sources (id, source_type, title, file_path) VALUES (?1, 'video', 'Video', 'video.mp4')", [&source_id]).unwrap();
    let package = root.path().join("package");
    export_notebook_package(&app, "original", &package).unwrap();
    let restored = import_notebook_package(&app, &package, &root.path().join("imports"), None).unwrap();
    let notebook = Path::new(&restored.imported_notebook_dir);
    let imported = NotebookDb::open(&notebook.join("notebook.db")).unwrap();
    assert_eq!(imported.get_source(&source_id).unwrap().id, source_id);
    // Witness the former path expression without mutating its target.
    assert_eq!(notebook.join("_tmp_frames_").join(&source_id), victim);
    for kind in [MediaWorkspaceKind::VideoFrames, MediaWorkspaceKind::AudioTranscript] {
        let workspace = create_media_workspace(kind).unwrap();
        let path = workspace.path().to_path_buf();
        assert_ne!(path, victim);
        fs::write(path.join("partial-output"), "private fixture").unwrap();
        drop(workspace);
        assert!(!path.exists());
        assert_eq!(fs::read(victim.join("sentinel")).unwrap(), b"retain unrelated data");
    }
}
