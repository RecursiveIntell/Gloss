//! Filesystem/registry publication is staged; deleted canonical data is retained
//! in a same-filesystem quarantine. No recursive purge belongs to this owner.
use crate::db::{app_db::AppDb, notebook_db::NotebookDb};
use crate::error::GlossError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub(crate) struct StagedNotebook {
    pub id: String,
    pub path: PathBuf,
    pub destination: PathBuf,
}

impl StagedNotebook {
    pub fn new(notebooks_dir: &Path) -> Result<Self, GlossError> {
        fs::create_dir_all(notebooks_dir)?;
        let id = uuid::Uuid::new_v4().to_string();
        let path = notebooks_dir.join(format!(".staging-{id}"));
        fs::create_dir(&path)?;
        let staged = Self {
            destination: notebooks_dir.join(&id),
            id,
            path,
        };
        let result = (|| {
            for directory in ["sources", "embeddings", "audio", "exports"] {
                fs::create_dir(staged.path.join(directory))?;
            }
            Ok(())
        })();
        staged.finish(result)?;
        Ok(staged)
    }

    pub fn finish<T>(&self, result: Result<T, GlossError>) -> Result<T, GlossError> {
        if let Err(error) = result {
            if self.path.exists() {
                if let Err(cleanup) = fs::remove_dir_all(&self.path) {
                    return Err(GlossError::Other(format!(
                        "Notebook operation failed: {error}; staging cleanup failed: {cleanup}. Recover operation {} at {}.",
                        self.id, self.path.display()
                    )));
                }
            }
            return Err(error);
        }
        result
    }

    /// Called only after the staged DB and receipt are complete and closed.
    /// Registry insertion/count update form one transaction. A failed insert
    /// moves the complete payload back to its staging identity before return.
    pub fn publish(&self, app_db: &AppDb, name: &str, source_count: i32) -> Result<(), GlossError> {
        if self.destination.exists() {
            return Err(GlossError::Config(
                "Notebook publication destination exists".into(),
            ));
        }
        let tx = app_db.conn().unchecked_transaction()?;
        publish_directory(&self.path, &self.destination)?;
        let result = (|| {
            sync_directory(self.destination.parent().expect("notebook parent"))?;
            app_db.create_notebook(&self.id, name, &self.destination.to_string_lossy())?;
            app_db.update_source_count(&self.id, source_count)?;
            tx.commit()?;
            Ok::<_, GlossError>(())
        })();
        if let Err(error) = result {
            if let Err(rollback) = fs::rename(&self.destination, &self.path) {
                return Err(GlossError::Other(format!(
                    "Notebook registration failed: {error}; rollback failed: {rollback}. Recover complete notebook at {} (operation {}).",
                    self.destination.display(), self.id
                )));
            }
            return Err(error);
        }
        Ok(())
    }
}

impl Drop for StagedNotebook {
    fn drop(&mut self) {
        // Only newly created, unpublished staging is disposable. Never touch
        // destination here: a registry transaction may already have committed.
        if self.path.exists() {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                tracing::warn!(path = %self.path.display(), %error, "Unpublished notebook staging retained for recovery");
            }
        }
    }
}

pub fn create_notebook_staged(
    app_db: &AppDb,
    notebooks_dir: &Path,
    name: &str,
) -> Result<String, GlossError> {
    let name = name.trim();
    if name.is_empty() {
        return Err(GlossError::Config("Notebook name cannot be empty".into()));
    }
    let staged = StagedNotebook::new(notebooks_dir)?;
    let result = (|| {
        // Failure cannot expose an unmigrated registered notebook.
        drop(NotebookDb::open(&staged.path.join("notebook.db"))?);
        write_json(
            &staged.path.join("exports/notebook_create_receipt.json"),
            &serde_json::json!({
                "schema": "NotebookCreateReceiptV1", "notebook_id": staged.id,
                "directory": staged.destination, "recorded_utc": chrono::Utc::now().to_rfc3339()
            }),
        )?;
        staged.publish(app_db, name, 0)?;
        Ok(staged.id.clone())
    })();
    staged.finish(result)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotebookDeleteReceipt {
    pub schema: String,
    pub notebook_id: String,
    pub original_directory: String,
    pub quarantine_directory: String,
    pub projection_directory: String,
    pub recorded_utc: String,
}

/// Call only after runtime connection admission is fenced and active owners
/// have drained. The receipt is intent, not a second registry: registry presence
/// decides whether a crash interrupted deletion or completed it.
pub fn quarantine_notebook(
    app_db: &AppDb,
    id: &str,
    projection: &Path,
) -> Result<NotebookDeleteReceipt, GlossError> {
    let notebook = app_db.get_notebook(id)?;
    let original = PathBuf::from(&notebook.directory);
    let parent = original
        .parent()
        .ok_or_else(|| GlossError::Config("Notebook directory has no parent".into()))?;
    let quarantine = parent.join(format!(".deleted-{id}"));
    let retained = quarantine.join("notebook");
    if quarantine.exists() {
        return Err(GlossError::Config(format!(
            "Notebook deletion recovery is pending at {}; restore its notebook directory to {} before retrying (registry still owns {}).",
            quarantine.display(), original.display(), id
        )));
    }
    fs::create_dir(&quarantine)?;
    let receipt = NotebookDeleteReceipt {
        schema: "NotebookDeleteReceiptV1".into(),
        notebook_id: id.into(),
        original_directory: notebook.directory,
        quarantine_directory: quarantine.to_string_lossy().into_owned(),
        projection_directory: projection.to_string_lossy().into_owned(),
        recorded_utc: chrono::Utc::now().to_rfc3339(),
    };
    if let Err(error) = write_json(&quarantine.join("delete_intent.json"), &receipt) {
        let _ = fs::remove_dir(&quarantine);
        return Err(error);
    }
    let mut canonical_moved = false;
    let mut projection_moved = false;
    let result = (|| {
        let tx = app_db.conn().unchecked_transaction()?;
        fs::rename(&original, &retained)?;
        canonical_moved = true;
        if projection.exists() {
            fs::rename(projection, quarantine.join("projection"))?;
            projection_moved = true;
        }
        sync_directory(&quarantine)?;
        sync_directory(parent)?;
        app_db.delete_notebook(id)?;
        tx.commit()?;
        Ok::<_, GlossError>(())
    })();
    if let Err(error) = result {
        let rollback = (|| {
            if projection_moved {
                fs::rename(quarantine.join("projection"), projection)?;
            }
            if canonical_moved {
                fs::rename(&retained, &original)?;
            }
            Ok::<_, std::io::Error>(())
        })();
        if let Err(rollback) = rollback {
            return Err(GlossError::Other(format!(
                "Notebook deletion failed: {error}; rollback failed: {rollback}. Data and intent retained at {} for notebook {id}.", quarantine.display()
            )));
        }
        let _ = fs::remove_file(quarantine.join("delete_intent.json"));
        let _ = fs::remove_dir(&quarantine);
        return Err(error);
    }
    // No fallible operation follows registry commit. Quarantine intentionally
    // remains until a separate, explicitly authorized permanent purge.
    Ok(receipt)
}

/// Publish a complete directory without replacing a pre-existing destination.
/// Unix rename can replace an empty directory, so reserve it exclusively first.
/// Windows directory rename requires a nonexistent destination.
pub(crate) fn publish_directory(source: &Path, destination: &Path) -> Result<(), GlossError> {
    #[cfg(unix)]
    {
        fs::create_dir(destination)?;
        if let Err(error) = fs::rename(source, destination) {
            let _ = fs::remove_dir(destination);
            return Err(error.into());
        }
    }
    #[cfg(not(unix))]
    {
        if destination.exists() {
            return Err(GlossError::Config(
                "Notebook publication destination exists".into(),
            ));
        }
        fs::rename(source, destination)?;
    }
    Ok(())
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<(), GlossError> {
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?;
    file.write_all(serde_json::to_string_pretty(value)?.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), GlossError> {
    #[cfg(unix)]
    fs::File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path; // std does not provide portable directory handles on Windows.
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn staged_create_registers_only_a_usable_database_and_receipt() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        let id = create_notebook_staged(&app, &root.path().join("notebooks"), " Ready ").unwrap();
        let notebook = app.get_notebook(&id).unwrap();
        assert_eq!(notebook.name, "Ready");
        assert_eq!(
            NotebookDb::open_read_only(&Path::new(&notebook.directory).join("notebook.db"))
                .unwrap()
                .source_count()
                .unwrap(),
            0
        );
        assert!(Path::new(&notebook.directory)
            .join("exports/notebook_create_receipt.json")
            .exists());
    }

    #[test]
    fn staged_create_registry_failure_leaves_no_registered_or_partial_notebook() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        app.conn().execute_batch("CREATE TRIGGER fail_insert BEFORE INSERT ON notebooks BEGIN SELECT RAISE(ABORT, 'injected registry failure'); END;").unwrap();
        let notebooks = root.path().join("notebooks");
        assert!(create_notebook_staged(&app, &notebooks, "Rejected").is_err());
        assert!(app.list_notebooks().unwrap().is_empty());
        assert_eq!(fs::read_dir(notebooks).unwrap().count(), 0);
    }

    #[test]
    fn staged_create_filesystem_failure_leaves_registry_unchanged() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        let notebooks = root.path().join("notebooks");
        fs::write(&notebooks, b"blocked directory").unwrap();
        assert!(create_notebook_staged(&app, &notebooks, "Rejected").is_err());
        assert!(app.list_notebooks().unwrap().is_empty());
        assert_eq!(fs::read(notebooks).unwrap(), b"blocked directory");
    }

    #[test]
    fn unpublished_migration_or_receipt_failure_drops_only_staging() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        let notebooks = root.path().join("notebooks");
        {
            let staged = StagedNotebook::new(&notebooks).unwrap();
            fs::write(staged.path.join("notebook.db"), b"not sqlite").unwrap();
            assert!(NotebookDb::open(&staged.path.join("notebook.db")).is_err());
            fs::create_dir(staged.path.join("exports/receipt.json")).unwrap();
            assert!(write_json(
                &staged.path.join("exports/receipt.json"),
                &serde_json::json!({})
            )
            .is_err());
        }
        assert!(app.list_notebooks().unwrap().is_empty());
        assert_eq!(fs::read_dir(notebooks).unwrap().count(), 0);
    }

    #[test]
    fn delete_quarantines_canonical_and_projection_data_without_purge() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        let notebooks = root.path().join("notebooks");
        let id = create_notebook_staged(&app, &notebooks, "Retain me").unwrap();
        let original = notebooks.join(&id);
        fs::write(original.join("sources/proof.txt"), b"canonical").unwrap();
        let projection = root.path().join("projection");
        fs::create_dir(&projection).unwrap();
        fs::write(projection.join("memory.db"), b"projection").unwrap();
        let receipt = quarantine_notebook(&app, &id, &projection).unwrap();
        let quarantine = Path::new(&receipt.quarantine_directory);
        assert!(app.get_notebook(&id).is_err());
        assert!(!original.exists());
        assert_eq!(
            fs::read(quarantine.join("notebook/sources/proof.txt")).unwrap(),
            b"canonical"
        );
        assert_eq!(
            fs::read(quarantine.join("projection/memory.db")).unwrap(),
            b"projection"
        );
        assert!(quarantine.join("delete_intent.json").exists());
    }

    #[test]
    fn delete_registry_failure_rolls_back_both_directories_and_is_retryable() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        let notebooks = root.path().join("notebooks");
        let id = create_notebook_staged(&app, &notebooks, "Retain me").unwrap();
        let projection = root.path().join("projection");
        fs::create_dir(&projection).unwrap();
        fs::write(projection.join("proof"), b"intact").unwrap();
        app.conn().execute_batch("CREATE TRIGGER fail_delete BEFORE DELETE ON notebooks BEGIN SELECT RAISE(ABORT, 'injected registry delete failure'); END;").unwrap();
        assert!(quarantine_notebook(&app, &id, &projection).is_err());
        assert!(app.get_notebook(&id).is_ok());
        assert!(notebooks.join(&id).join("notebook.db").exists());
        assert_eq!(fs::read(projection.join("proof")).unwrap(), b"intact");
        assert!(!notebooks.join(format!(".deleted-{id}")).exists());
        app.conn()
            .execute_batch("DROP TRIGGER fail_delete")
            .unwrap();
        assert!(quarantine_notebook(&app, &id, &projection).is_ok());
    }

    #[test]
    fn delete_canonical_rename_failure_never_touches_projection_or_registry() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        let original = root.path().join("missing");
        app.create_notebook("missing", "Missing", &original.to_string_lossy())
            .unwrap();
        let projection = root.path().join("projection");
        fs::create_dir(&projection).unwrap();
        fs::write(projection.join("proof"), b"intact").unwrap();
        assert!(quarantine_notebook(&app, "missing", &projection).is_err());
        assert!(app.get_notebook("missing").is_ok());
        assert_eq!(fs::read(projection.join("proof")).unwrap(), b"intact");
    }
}
