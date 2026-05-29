use crate::db::app_db::AppDb;
use crate::error::GlossError;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};
use tar::{Archive, Builder, EntryType};

const MAX_PORTABLE_ARCHIVE_FILES: usize = 200_000;
const MAX_PORTABLE_ARCHIVE_UNPACKED_BYTES: u64 = 20 * 1024 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PortableFileManifestEntry {
    pub path: String,
    pub sha256: String,
    pub byte_len: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookPortableManifest {
    pub schema: String,
    pub package_id: String,
    pub exported_utc: String,
    pub source_notebook_id: String,
    pub notebook_name: String,
    pub files: Vec<PortableFileManifestEntry>,
    pub manifest_digest: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookExportReceipt {
    pub schema: String,
    pub receipt_id: String,
    pub package_id: String,
    pub notebook_id: String,
    pub package_format: String,
    pub package_dir: String,
    pub archive_path: Option<String>,
    pub manifest_path: String,
    pub file_count: usize,
    pub manifest_digest: String,
    pub recorded_utc: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotebookImportReceipt {
    pub schema: String,
    pub receipt_id: String,
    pub package_id: String,
    pub source_notebook_id: String,
    pub imported_notebook_id: String,
    pub imported_notebook_dir: String,
    pub file_count: usize,
    pub manifest_digest: String,
    pub recorded_utc: String,
}

pub fn export_notebook_package(
    app_db: &AppDb,
    notebook_id: &str,
    package_dir: &Path,
) -> Result<NotebookExportReceipt, GlossError> {
    build_notebook_package(app_db, notebook_id, package_dir, "directory", None)
}

pub fn export_notebook_archive(
    app_db: &AppDb,
    notebook_id: &str,
    archive_path: &Path,
) -> Result<NotebookExportReceipt, GlossError> {
    if archive_path.exists() {
        return Err(GlossError::Config(format!(
            "Notebook export archive already exists: {}",
            archive_path.display()
        )));
    }
    if let Some(parent) = archive_path.parent() {
        fs::create_dir_all(parent)?;
    }
    let temp_root = create_temp_package_dir("gloss-notebook-export")?;
    let package_dir = temp_root.join("package");
    let result = (|| {
        let receipt = build_notebook_package(
            app_db,
            notebook_id,
            &package_dir,
            "tar_gzip",
            Some(archive_path.to_string_lossy().to_string()),
        )?;
        create_tar_gz_archive(&package_dir, archive_path)?;
        Ok(receipt)
    })();
    let _ = fs::remove_dir_all(&temp_root);
    result
}

fn build_notebook_package(
    app_db: &AppDb,
    notebook_id: &str,
    package_dir: &Path,
    package_format: &str,
    archive_path: Option<String>,
) -> Result<NotebookExportReceipt, GlossError> {
    let notebook = app_db.get_notebook(notebook_id)?;
    let source_dir = PathBuf::from(&notebook.directory);
    let package_id = uuid::Uuid::new_v4().to_string();
    let recorded_utc = chrono::Utc::now().to_rfc3339();

    if package_dir.exists() {
        return Err(GlossError::Config(format!(
            "Notebook export package already exists: {}",
            package_dir.display()
        )));
    }
    fs::create_dir_all(package_dir)?;
    fs::create_dir_all(package_dir.join("sources"))?;
    fs::create_dir_all(package_dir.join("embeddings"))?;
    fs::create_dir_all(package_dir.join("receipts"))?;

    copy_required_file(
        &source_dir.join("notebook.db"),
        &package_dir.join("notebook.db"),
    )?;
    copy_dir_contents(&source_dir.join("sources"), &package_dir.join("sources"))?;
    copy_dir_contents(
        &source_dir.join("embeddings"),
        &package_dir.join("embeddings"),
    )?;

    let mut files = collect_manifest_files(package_dir)?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let manifest_digest = digest_manifest_entries(&files);
    let manifest = NotebookPortableManifest {
        schema: "NotebookPortableManifestV1".to_string(),
        package_id: package_id.clone(),
        exported_utc: recorded_utc.clone(),
        source_notebook_id: notebook.id.clone(),
        notebook_name: notebook.name.clone(),
        files,
        manifest_digest: manifest_digest.clone(),
    };
    let manifest_path = package_dir.join("manifest.json");
    fs::write(
        &manifest_path,
        serde_json::to_string_pretty(&manifest).map_err(GlossError::JsonParse)? + "\n",
    )?;

    let receipt = NotebookExportReceipt {
        schema: "NotebookExportReceiptV1".to_string(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        package_id,
        notebook_id: notebook.id,
        package_format: package_format.to_string(),
        package_dir: package_dir.to_string_lossy().to_string(),
        archive_path,
        manifest_path: manifest_path.to_string_lossy().to_string(),
        file_count: manifest.files.len(),
        manifest_digest,
        recorded_utc,
    };
    fs::write(
        package_dir.join("receipts").join("export_receipt.json"),
        serde_json::to_string_pretty(&receipt).map_err(GlossError::JsonParse)? + "\n",
    )?;
    Ok(receipt)
}

pub fn import_notebook_package(
    app_db: &AppDb,
    package_dir: &Path,
    notebooks_dir: &Path,
    name_override: Option<&str>,
) -> Result<NotebookImportReceipt, GlossError> {
    let manifest = validate_notebook_package(package_dir)?;
    let imported_notebook_id = uuid::Uuid::new_v4().to_string();
    let imported_dir = notebooks_dir.join(&imported_notebook_id);
    if imported_dir.exists() {
        return Err(GlossError::Config(format!(
            "Imported notebook directory already exists: {}",
            imported_dir.display()
        )));
    }
    fs::create_dir_all(imported_dir.join("sources"))?;
    fs::create_dir_all(imported_dir.join("embeddings"))?;
    fs::create_dir_all(imported_dir.join("exports"))?;
    fs::create_dir_all(imported_dir.join("audio"))?;

    copy_required_file(
        &package_dir.join("notebook.db"),
        &imported_dir.join("notebook.db"),
    )?;
    copy_dir_contents(&package_dir.join("sources"), &imported_dir.join("sources"))?;
    copy_dir_contents(
        &package_dir.join("embeddings"),
        &imported_dir.join("embeddings"),
    )?;

    let notebook_name = name_override
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(&manifest.notebook_name);
    app_db.create_notebook(
        &imported_notebook_id,
        notebook_name,
        &imported_dir.to_string_lossy(),
    )?;

    let receipt = NotebookImportReceipt {
        schema: "NotebookImportReceiptV1".to_string(),
        receipt_id: uuid::Uuid::new_v4().to_string(),
        package_id: manifest.package_id.clone(),
        source_notebook_id: manifest.source_notebook_id.clone(),
        imported_notebook_id,
        imported_notebook_dir: imported_dir.to_string_lossy().to_string(),
        file_count: manifest.files.len(),
        manifest_digest: manifest.manifest_digest.clone(),
        recorded_utc: chrono::Utc::now().to_rfc3339(),
    };
    fs::write(
        imported_dir
            .join("exports")
            .join("notebook_import_receipt.json"),
        serde_json::to_string_pretty(&receipt).map_err(GlossError::JsonParse)? + "\n",
    )?;
    Ok(receipt)
}

pub fn validate_notebook_archive(
    archive_path: &Path,
) -> Result<NotebookPortableManifest, GlossError> {
    let temp_root = create_temp_package_dir("gloss-notebook-validate")?;
    let package_dir = temp_root.join("package");
    let result = (|| {
        extract_tar_gz_archive(archive_path, &package_dir)?;
        validate_notebook_package(&package_dir)
    })();
    let _ = fs::remove_dir_all(&temp_root);
    result
}

pub fn import_notebook_archive(
    app_db: &AppDb,
    archive_path: &Path,
    notebooks_dir: &Path,
    name_override: Option<&str>,
) -> Result<NotebookImportReceipt, GlossError> {
    let temp_root = create_temp_package_dir("gloss-notebook-import")?;
    let package_dir = temp_root.join("package");
    let result = (|| {
        extract_tar_gz_archive(archive_path, &package_dir)?;
        import_notebook_package(app_db, &package_dir, notebooks_dir, name_override)
    })();
    let _ = fs::remove_dir_all(&temp_root);
    result
}

pub fn validate_notebook_package(
    package_dir: &Path,
) -> Result<NotebookPortableManifest, GlossError> {
    let manifest_path = package_dir.join("manifest.json");
    let manifest: NotebookPortableManifest =
        serde_json::from_slice(&fs::read(&manifest_path)?).map_err(GlossError::JsonParse)?;
    if manifest.schema != "NotebookPortableManifestV1" {
        return Err(GlossError::Config(format!(
            "Unsupported notebook package manifest schema: {}",
            manifest.schema
        )));
    }
    let digest = digest_manifest_entries(&manifest.files);
    if digest != manifest.manifest_digest {
        return Err(GlossError::Config(
            "Notebook package manifest digest mismatch".to_string(),
        ));
    }
    for entry in &manifest.files {
        validate_relative_package_path(&entry.path)?;
        let file_path = package_dir.join(&entry.path);
        let (actual_hash, actual_len) = hash_file(&file_path)?;
        if actual_hash != entry.sha256 || actual_len != entry.byte_len {
            return Err(GlossError::Config(format!(
                "Notebook package file hash mismatch: {}",
                entry.path
            )));
        }
    }
    Ok(manifest)
}

fn copy_required_file(from: &Path, to: &Path) -> Result<(), GlossError> {
    if !from.is_file() {
        return Err(GlossError::NotFound(format!(
            "Required notebook package file missing: {}",
            from.display()
        )));
    }
    if let Some(parent) = to.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::copy(from, to)?;
    Ok(())
}

fn copy_dir_contents(from: &Path, to: &Path) -> Result<(), GlossError> {
    if !from.exists() {
        return Ok(());
    }
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let source_path = entry.path();
        let dest_path = to.join(entry.file_name());
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            copy_dir_contents(&source_path, &dest_path)?;
        } else if file_type.is_file() {
            copy_required_file(&source_path, &dest_path)?;
        }
    }
    Ok(())
}

fn create_temp_package_dir(prefix: &str) -> Result<PathBuf, GlossError> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn create_tar_gz_archive(package_dir: &Path, archive_path: &Path) -> Result<(), GlossError> {
    let file = File::create(archive_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);
    append_package_to_archive(package_dir, package_dir, &mut builder)?;
    builder.finish()?;
    let encoder = builder.into_inner()?;
    encoder.finish()?;
    Ok(())
}

fn append_package_to_archive(
    root: &Path,
    dir: &Path,
    builder: &mut Builder<GzEncoder<File>>,
) -> Result<(), GlossError> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        let path = entry.path();
        let file_type = entry.file_type()?;
        let rel = path
            .strip_prefix(root)
            .map_err(|error| GlossError::Other(error.to_string()))?
            .to_string_lossy()
            .replace('\\', "/");
        validate_relative_package_path(&rel)?;
        if file_type.is_dir() {
            builder.append_dir(&rel, &path)?;
            append_package_to_archive(root, &path, builder)?;
        } else if file_type.is_file() {
            builder.append_path_with_name(&path, &rel)?;
        }
    }
    Ok(())
}

fn extract_tar_gz_archive(archive_path: &Path, package_dir: &Path) -> Result<(), GlossError> {
    if !archive_path.is_file() {
        return Err(GlossError::NotFound(format!(
            "Notebook archive not found: {}",
            archive_path.display()
        )));
    }
    fs::create_dir_all(package_dir)?;
    let file = File::open(archive_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);
    let mut file_count = 0usize;
    let mut unpacked_bytes = 0u64;
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        let rel = entry.path()?.to_string_lossy().replace('\\', "/");
        validate_relative_package_path(&rel)?;
        if entry_type == EntryType::Directory {
            fs::create_dir_all(package_dir.join(&rel))?;
            continue;
        }
        if entry_type != EntryType::Regular {
            return Err(GlossError::Config(format!(
                "Unsupported notebook archive entry type: {rel}"
            )));
        }
        file_count += 1;
        if file_count > MAX_PORTABLE_ARCHIVE_FILES {
            return Err(GlossError::Config(
                "Notebook archive contains too many files".to_string(),
            ));
        }
        unpacked_bytes = unpacked_bytes.saturating_add(entry.header().size()?);
        if unpacked_bytes > MAX_PORTABLE_ARCHIVE_UNPACKED_BYTES {
            return Err(GlossError::Config(
                "Notebook archive exceeds unpacked byte limit".to_string(),
            ));
        }
        let dest = package_dir.join(&rel);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        entry.unpack(dest)?;
    }
    validate_notebook_package(package_dir).map(|_| ())
}

fn collect_manifest_files(
    package_dir: &Path,
) -> Result<Vec<PortableFileManifestEntry>, GlossError> {
    let mut files = Vec::new();
    collect_manifest_files_inner(package_dir, package_dir, &mut files)?;
    Ok(files)
}

fn collect_manifest_files_inner(
    root: &Path,
    dir: &Path,
    files: &mut Vec<PortableFileManifestEntry>,
) -> Result<(), GlossError> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_manifest_files_inner(root, &path, files)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|error| GlossError::Other(error.to_string()))?
                .to_string_lossy()
                .replace('\\', "/");
            if rel == "manifest.json" || rel.starts_with("receipts/") {
                continue;
            }
            validate_relative_package_path(&rel)?;
            let (sha256, byte_len) = hash_file(&path)?;
            files.push(PortableFileManifestEntry {
                path: rel,
                sha256,
                byte_len,
            });
        }
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<(String, u64), GlossError> {
    let file = File::open(path)?;
    let mut reader = BufReader::new(file);
    let mut hasher = Sha256::new();
    let mut len = 0u64;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        len += read as u64;
        hasher.update(&buf[..read]);
    }
    Ok((format!("{:x}", hasher.finalize()), len))
}

fn digest_manifest_entries(files: &[PortableFileManifestEntry]) -> String {
    let mut hasher = Sha256::new();
    for entry in files {
        hasher.update(entry.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(entry.sha256.as_bytes());
        hasher.update(b"\0");
        hasher.update(entry.byte_len.to_string().as_bytes());
        hasher.update(b"\n");
    }
    format!("{:x}", hasher.finalize())
}

fn validate_relative_package_path(path: &str) -> Result<(), GlossError> {
    let rel = Path::new(path);
    if rel.is_absolute()
        || path.is_empty()
        || path == "."
        || path.contains('\\')
        || rel.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::CurDir
                    | std::path::Component::Prefix(_)
                    | std::path::Component::RootDir
            )
        })
    {
        return Err(GlossError::Config(format!(
            "Unsafe notebook package path: {path}"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        create_tar_gz_archive, export_notebook_archive, export_notebook_package,
        extract_tar_gz_archive, import_notebook_archive, import_notebook_package,
        validate_notebook_archive, validate_notebook_package,
    };
    use crate::db::app_db::AppDb;
    use crate::db::notebook_db::{NotebookDb, Source};
    use tempfile::tempdir;

    fn source(id: &str, file_path: &str) -> Source {
        Source {
            id: id.to_string(),
            source_type: "text".to_string(),
            title: id.to_string(),
            original_filename: Some(file_path.to_string()),
            file_hash: None,
            url: None,
            file_path: Some(file_path.to_string()),
            content_text: None,
            word_count: Some(2),
            metadata: None,
            summary: None,
            summary_model: None,
            status: "ready".to_string(),
            error_message: None,
            selected: true,
            created_at: String::new(),
            updated_at: String::new(),
            processing_state: None,
        }
    }

    #[test]
    fn notebook_export_import_roundtrip_validates_hashes() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        std::fs::create_dir_all(notebook_dir.join("embeddings")).unwrap();
        std::fs::create_dir_all(notebook_dir.join("exports")).unwrap();
        std::fs::write(
            notebook_dir.join("sources").join("source.txt"),
            "portable source",
        )
        .unwrap();
        std::fs::write(
            notebook_dir.join("embeddings").join("chunks.usearch"),
            "index",
        )
        .unwrap();
        app_db
            .create_notebook("nb1", "Portable", &notebook_dir.to_string_lossy())
            .unwrap();
        let notebook_db = NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();
        notebook_db
            .insert_source(&source("s1", "source.txt"))
            .unwrap();
        app_db.update_source_count("nb1", 1).unwrap();

        let package_dir = dir.path().join("portable-package");
        let export_receipt = export_notebook_package(&app_db, "nb1", &package_dir).unwrap();
        assert_eq!(export_receipt.schema, "NotebookExportReceiptV1");
        assert!(package_dir.join("manifest.json").is_file());
        assert!(package_dir.join("notebook.db").is_file());
        assert!(package_dir.join("sources").join("source.txt").is_file());

        let manifest = validate_notebook_package(&package_dir).unwrap();
        assert_eq!(manifest.schema, "NotebookPortableManifestV1");
        assert!(manifest.files.iter().any(|file| file.path == "notebook.db"));
        assert!(manifest
            .files
            .iter()
            .any(|file| file.path == "sources/source.txt"));

        let import_receipt = import_notebook_package(
            &app_db,
            &package_dir,
            &dir.path().join("notebooks"),
            Some("Imported Portable"),
        )
        .unwrap();
        let imported = app_db
            .get_notebook(&import_receipt.imported_notebook_id)
            .unwrap();
        assert_eq!(imported.name, "Imported Portable");
        assert!(std::path::Path::new(&imported.directory)
            .join("sources")
            .join("source.txt")
            .is_file());
    }

    #[test]
    fn notebook_package_validation_rejects_tampering() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        std::fs::write(
            notebook_dir.join("sources").join("source.txt"),
            "portable source",
        )
        .unwrap();
        app_db
            .create_notebook("nb1", "Portable", &notebook_dir.to_string_lossy())
            .unwrap();
        NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();

        let package_dir = dir.path().join("portable-package");
        export_notebook_package(&app_db, "nb1", &package_dir).unwrap();
        std::fs::write(package_dir.join("sources").join("source.txt"), "tampered").unwrap();
        let error = validate_notebook_package(&package_dir).unwrap_err();
        assert!(error.to_string().contains("hash mismatch"));
    }

    #[test]
    fn notebook_archive_export_import_replay_validates_hashes() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        std::fs::create_dir_all(notebook_dir.join("embeddings")).unwrap();
        std::fs::write(
            notebook_dir.join("sources").join("source.txt"),
            "portable source",
        )
        .unwrap();
        std::fs::write(
            notebook_dir.join("embeddings").join("chunks.usearch"),
            "index",
        )
        .unwrap();
        app_db
            .create_notebook("nb1", "Portable", &notebook_dir.to_string_lossy())
            .unwrap();
        NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();

        let archive_path = dir.path().join("portable-package.glosspkg.tar.gz");
        let export_receipt = export_notebook_archive(&app_db, "nb1", &archive_path).unwrap();
        assert_eq!(export_receipt.package_format, "tar_gzip");
        assert_eq!(
            export_receipt.archive_path.as_deref(),
            Some(archive_path.to_string_lossy().as_ref())
        );
        assert!(archive_path.is_file());

        let manifest = validate_notebook_archive(&archive_path).unwrap();
        assert!(manifest
            .files
            .iter()
            .any(|file| file.path == "sources/source.txt"));

        let import_receipt = import_notebook_archive(
            &app_db,
            &archive_path,
            &dir.path().join("notebooks"),
            Some("Imported Archive"),
        )
        .unwrap();
        let imported = app_db
            .get_notebook(&import_receipt.imported_notebook_id)
            .unwrap();
        assert_eq!(imported.name, "Imported Archive");
        assert!(std::path::Path::new(&imported.directory)
            .join("sources")
            .join("source.txt")
            .is_file());
    }

    #[test]
    fn notebook_archive_validation_rejects_tampering() {
        let dir = tempdir().unwrap();
        let app_db = AppDb::open(&dir.path().join("gloss.db")).unwrap();
        let notebook_dir = dir.path().join("notebooks").join("nb1");
        std::fs::create_dir_all(notebook_dir.join("sources")).unwrap();
        std::fs::write(
            notebook_dir.join("sources").join("source.txt"),
            "portable source",
        )
        .unwrap();
        app_db
            .create_notebook("nb1", "Portable", &notebook_dir.to_string_lossy())
            .unwrap();
        NotebookDb::open(&notebook_dir.join("notebook.db")).unwrap();

        let archive_path = dir.path().join("portable-package.glosspkg.tar.gz");
        export_notebook_archive(&app_db, "nb1", &archive_path).unwrap();
        let extracted = dir.path().join("extracted");
        extract_tar_gz_archive(&archive_path, &extracted).unwrap();
        std::fs::write(extracted.join("sources").join("source.txt"), "tampered").unwrap();

        let tampered_archive = dir.path().join("tampered-package.glosspkg.tar.gz");
        create_tar_gz_archive(&extracted, &tampered_archive).unwrap();
        let error = validate_notebook_archive(&tampered_archive).unwrap_err();
        assert!(error.to_string().contains("hash mismatch"));
    }
}
