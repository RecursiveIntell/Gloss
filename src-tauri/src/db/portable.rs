use crate::db::app_db::AppDb;
use crate::db::notebook_db::NotebookDb;
use crate::db::notebook_lifecycle::{publish_directory, write_json, StagedNotebook};
use crate::error::GlossError;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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
    #[serde(default)]
    pub projection_policy: String,
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
    let parent = archive_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let staging = PackageStaging::new(parent)?;
    let package_dir = staging.path.join("package");
    let result = (|| {
        let receipt = build_notebook_package(
            app_db,
            notebook_id,
            &package_dir,
            "tar_gzip",
            Some(archive_path.to_string_lossy().to_string()),
        )?;
        let staged_archive = staging.path.join("archive.tar.gz");
        create_tar_gz_archive(&package_dir, &staged_archive)?;
        // An exclusive same-filesystem hard link publishes only complete bytes.
        fs::hard_link(staged_archive, archive_path)?;
        Ok(receipt)
    })();
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
    let parent = package_dir
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = PackageStaging::new(parent)?;
    let output = &staging.path;
    fs::create_dir(output.join("sources"))?;
    fs::create_dir(output.join("embeddings"))?;
    fs::create_dir(output.join("receipts"))?;

    // VACUUM INTO takes one SQLite read snapshot, including committed WAL.
    // Opening read-only ensures export cannot migrate/checkpoint the original.
    snapshot_database(&source_dir.join("notebook.db"), &output.join("notebook.db"))?;
    copy_snapshot_sources(&source_dir, output)?;
    // External indexes have independent publication generations. Never copy
    // them as if they were atomically paired with the canonical DB snapshot.

    let mut files = collect_manifest_files(output)?;
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
        projection_policy: "rebuild_required".into(),
    };
    let manifest_path = package_dir.join("manifest.json");
    write_json(&output.join("manifest.json"), &manifest)?;

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
    write_json(&output.join("receipts/export_receipt.json"), &receipt)?;
    validate_notebook_package(output)?;
    if package_dir.exists() {
        return Err(GlossError::Config(
            "Notebook export destination appeared during staging".into(),
        ));
    }
    publish_directory(output, package_dir)?;
    Ok(receipt)
}

pub fn import_notebook_package(
    app_db: &AppDb,
    package_dir: &Path,
    notebooks_dir: &Path,
    name_override: Option<&str>,
) -> Result<NotebookImportReceipt, GlossError> {
    let manifest = validate_notebook_package(package_dir)?;
    let staged = StagedNotebook::new(notebooks_dir)?;
    let result = (|| {
        // Validation and consumption share the exact manifest inventory. Hash the
        // copied bytes again so a package modified during copying cannot publish.
        for entry in &manifest.files {
            let destination = staged.path.join(&entry.path);
            require_safe_descendant(package_dir, &entry.path)?;
            copy_required_file(&package_dir.join(&entry.path), &destination)?;
            let (hash, len) = hash_file(&destination)?;
            if hash != entry.sha256 || len != entry.byte_len {
                return Err(GlossError::Config(format!(
                    "Notebook package changed during import: {}",
                    entry.path
                )));
            }
        }
        validate_database(&staged.path.join("notebook.db"))?;
        let source_count = {
            let db = NotebookDb::open(&staged.path.join("notebook.db"))?;
            db.invalidate_restored_projections()?;
            db.source_count()?
        };
        // Legacy packages may contain verified indexes; none are admitted after a
        // restore under a new notebook identity. Remove only these staged copies.
        fs::remove_dir_all(staged.path.join("embeddings"))?;
        fs::create_dir(staged.path.join("embeddings"))?;
        let notebook_name = name_override
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .unwrap_or(&manifest.notebook_name);
        let receipt = NotebookImportReceipt {
            schema: "NotebookImportReceiptV1".to_string(),
            receipt_id: uuid::Uuid::new_v4().to_string(),
            package_id: manifest.package_id.clone(),
            source_notebook_id: manifest.source_notebook_id.clone(),
            imported_notebook_id: staged.id.clone(),
            imported_notebook_dir: staged.destination.to_string_lossy().into_owned(),
            file_count: manifest.files.len(),
            manifest_digest: manifest.manifest_digest.clone(),
            recorded_utc: chrono::Utc::now().to_rfc3339(),
        };
        write_json(
            &staged.path.join("exports/notebook_import_receipt.json"),
            &receipt,
        )?;
        write_json(
            &staged.path.join("exports/projection_rebuild_required.json"),
            &serde_json::json!({
                "schema": "PortableProjectionInvalidationV1", "import_receipt_id": receipt.receipt_id,
                "reason": "Canonical content restored; external projections were not atomically snapshotted and must be rebuilt",
                "native_hnsw": "stale", "semantic_memory": "stale"
            }),
        )?;
        staged.publish(app_db, notebook_name, source_count)?;
        Ok(receipt)
    })();
    staged.finish(result)
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
    require_regular_directory(package_dir)?;
    let manifest_path = package_dir.join("manifest.json");
    require_regular_file(&manifest_path)?;
    if fs::metadata(&manifest_path)?.len() > 64 * 1024 * 1024 {
        return Err(GlossError::Config(
            "Notebook manifest exceeds byte limit".into(),
        ));
    }
    let manifest: NotebookPortableManifest = serde_json::from_slice(&fs::read(&manifest_path)?)?;
    if manifest.schema != "NotebookPortableManifestV1" {
        return Err(GlossError::Config(format!(
            "Unsupported notebook package manifest schema: {}",
            manifest.schema
        )));
    }
    if manifest.files.len() > MAX_PORTABLE_ARCHIVE_FILES {
        return Err(GlossError::Config(
            "Notebook package contains too many files".into(),
        ));
    }
    if digest_manifest_entries(&manifest.files) != manifest.manifest_digest {
        return Err(GlossError::Config(
            "Notebook package manifest digest mismatch".into(),
        ));
    }
    let mut listed = HashSet::new();
    for entry in &manifest.files {
        validate_relative_package_path(&entry.path)?;
        if !is_payload_path(&entry.path) || !listed.insert(entry.path.clone()) {
            return Err(GlossError::Config(format!(
                "Duplicate or unsupported notebook payload: {}",
                entry.path
            )));
        }
    }
    if !listed.contains("notebook.db") {
        return Err(GlossError::Config(
            "Notebook manifest must contain exactly one notebook.db".into(),
        ));
    }
    let actual = collect_manifest_files(package_dir)?;
    let actual_by_path = actual
        .iter()
        .map(|entry| (entry.path.clone(), entry))
        .collect::<HashMap<_, _>>();
    let actual_paths = actual_by_path.keys().cloned().collect::<HashSet<_>>();
    if listed != actual_paths {
        return Err(GlossError::Config(
            "Notebook package payload inventory does not match manifest".into(),
        ));
    }
    for entry in &manifest.files {
        let found = actual_by_path.get(&entry.path).expect("equal inventories");
        if found.sha256 != entry.sha256 || found.byte_len != entry.byte_len {
            return Err(GlossError::Config(format!(
                "Notebook package file hash mismatch: {}",
                entry.path
            )));
        }
    }
    validate_database(&package_dir.join("notebook.db"))?;
    validate_snapshot_sources(package_dir)?;
    Ok(manifest)
}

struct PackageStaging {
    path: PathBuf,
}
impl PackageStaging {
    fn new(parent: &Path) -> Result<Self, GlossError> {
        let path = parent.join(format!(".gloss-package-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path)?;
        Ok(Self { path })
    }
}
impl Drop for PackageStaging {
    fn drop(&mut self) {
        if self.path.exists() {
            if let Err(error) = fs::remove_dir_all(&self.path) {
                tracing::warn!(path = %self.path.display(), %error, "Unpublished export staging retained");
            }
        }
    }
}

fn snapshot_database(source: &Path, destination: &Path) -> Result<(), GlossError> {
    require_regular_file(source)?;
    let connection =
        rusqlite::Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(std::time::Duration::from_secs(15))?;
    connection.execute("VACUUM INTO ?1", [destination.to_string_lossy().as_ref()])?;
    validate_database(destination)
}

fn validate_database(path: &Path) -> Result<(), GlossError> {
    let db = NotebookDb::open_read_only(path)?;
    let check: String = db
        .conn()
        .query_row("PRAGMA quick_check", [], |row| row.get(0))?;
    if check != "ok" {
        return Err(GlossError::Config(format!(
            "Notebook database integrity check failed: {check}"
        )));
    }
    let version: String = db.conn().query_row(
        "SELECT value FROM _meta WHERE key = 'schema_version'",
        [],
        |row| row.get(0),
    )?;
    let version = version
        .parse::<i32>()
        .map_err(|_| GlossError::Config("Invalid notebook schema version".into()))?;
    if !(1..=crate::db::migrations::NOTEBOOK_SCHEMA_VERSION).contains(&version) {
        return Err(GlossError::Config(format!(
            "Unsupported notebook database schema version: {version}"
        )));
    }
    for table in ["sources", "chunks", "conversations", "messages", "notes"] {
        db.conn()
            .prepare(&format!("SELECT * FROM {table} LIMIT 0"))?;
    }
    Ok(())
}

fn copy_snapshot_sources(source_dir: &Path, output: &Path) -> Result<(), GlossError> {
    let source_root = source_dir.join("sources");
    if source_root.exists() {
        copy_source_tree(&source_root, &output.join("sources"), 0, &mut 0)?;
    }
    validate_snapshot_sources(output)
}

fn validate_snapshot_sources(output: &Path) -> Result<(), GlossError> {
    let db = NotebookDb::open_read_only(&output.join("notebook.db"))?;
    let mut stmt = db.conn().prepare(
        "SELECT file_path, file_hash FROM sources WHERE file_path IS NOT NULL AND file_path != ''",
    )?;
    let rows = stmt.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?))
    })?;
    for row in rows {
        let (relative, expected_hash) = row?;
        validate_relative_package_path(&relative)?;
        require_safe_descendant(&output.join("sources"), &relative)?;
        let to = output.join("sources").join(&relative);
        require_regular_file(&to)?;
        if let Some(expected) = expected_hash {
            if hash_file(&to)?.0 != expected {
                return Err(GlossError::Config(format!(
                    "Source asset changed since notebook snapshot: {relative}"
                )));
            }
        }
    }
    Ok(())
}

fn copy_source_tree(
    source: &Path,
    destination: &Path,
    depth: usize,
    count: &mut usize,
) -> Result<(), GlossError> {
    if depth > 64 {
        return Err(GlossError::Config(
            "Source directory nesting limit exceeded".into(),
        ));
    }
    require_regular_directory(source)?;
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        *count += 1;
        if *count > MAX_PORTABLE_ARCHIVE_FILES {
            return Err(GlossError::Config(
                "Source asset entry limit exceeded".into(),
            ));
        }
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_source_tree(&entry.path(), &target, depth + 1, count)?;
        } else {
            copy_required_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

fn require_regular_file(path: &Path) -> Result<(), GlossError> {
    if !fs::symlink_metadata(path)?.file_type().is_file() {
        return Err(GlossError::Config(format!(
            "Notebook payload is not a regular file: {}",
            path.display()
        )));
    }
    Ok(())
}
fn require_regular_directory(path: &Path) -> Result<(), GlossError> {
    if !fs::symlink_metadata(path)?.file_type().is_dir() {
        return Err(GlossError::Config(format!(
            "Notebook payload is not a regular directory: {}",
            path.display()
        )));
    }
    Ok(())
}
fn require_safe_descendant(root: &Path, relative: &str) -> Result<(), GlossError> {
    require_regular_directory(root)?;
    let mut path = root.to_path_buf();
    let components = Path::new(relative).components().collect::<Vec<_>>();
    for (i, component) in components.iter().enumerate() {
        path.push(component.as_os_str());
        if i + 1 == components.len() {
            require_regular_file(&path)?;
        } else {
            require_regular_directory(&path)?;
        }
    }
    Ok(())
}
fn is_payload_path(path: &str) -> bool {
    path == "notebook.db" || path.starts_with("sources/") || path.starts_with("embeddings/")
}

fn copy_required_file(from: &Path, to: &Path) -> Result<(), GlossError> {
    require_regular_file(from)?;
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
    File::open(to)?.sync_all()?;
    Ok(())
}

fn create_temp_package_dir(prefix: &str) -> Result<PathBuf, GlossError> {
    let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn create_tar_gz_archive(package_dir: &Path, archive_path: &Path) -> Result<(), GlossError> {
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(archive_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = Builder::new(encoder);
    append_package_to_archive(package_dir, package_dir, &mut builder)?;
    builder.finish()?;
    let encoder = builder.into_inner()?;
    encoder.finish()?.sync_all()?;
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
    let mut seen = HashSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let entry_type = entry.header().entry_type();
        let rel = entry
            .path()?
            .to_str()
            .ok_or_else(|| GlossError::Config("Non-UTF8 archive path".into()))?
            .to_owned();
        validate_relative_package_path(&rel)?;
        if !seen.insert(rel.clone()) || seen.len() > MAX_PORTABLE_ARCHIVE_FILES {
            return Err(GlossError::Config(
                "Duplicate archive path or entry limit exceeded".into(),
            ));
        }
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
    require_regular_directory(package_dir)?;
    let mut files = Vec::new();
    let mut entries = 0usize;
    let mut bytes = 0u64;
    collect_manifest_files_inner(
        package_dir,
        package_dir,
        &mut files,
        &mut entries,
        &mut bytes,
        0,
    )?;
    Ok(files)
}

fn collect_manifest_files_inner(
    root: &Path,
    dir: &Path,
    files: &mut Vec<PortableFileManifestEntry>,
    entries: &mut usize,
    bytes: &mut u64,
    depth: usize,
) -> Result<(), GlossError> {
    if depth > 64 {
        return Err(GlossError::Config(
            "Notebook package nesting limit exceeded".into(),
        ));
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        *entries += 1;
        if *entries > MAX_PORTABLE_ARCHIVE_FILES {
            return Err(GlossError::Config(
                "Notebook package entry limit exceeded".into(),
            ));
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        let rel = path
            .strip_prefix(root)
            .map_err(|error| GlossError::Other(error.to_string()))?
            .to_str()
            .ok_or_else(|| GlossError::Config("Non-UTF8 notebook payload path".into()))?
            .replace(std::path::MAIN_SEPARATOR, "/");
        validate_relative_package_path(&rel)?;
        if file_type.is_dir() {
            if rel != "sources"
                && rel != "embeddings"
                && rel != "receipts"
                && !rel.starts_with("sources/")
                && !rel.starts_with("embeddings/")
            {
                return Err(GlossError::Config(format!(
                    "Unsupported notebook payload directory: {rel}"
                )));
            }
            collect_manifest_files_inner(root, &path, files, entries, bytes, depth + 1)?;
        } else if file_type.is_file() {
            *bytes = bytes
                .checked_add(entry.metadata()?.len())
                .ok_or_else(|| GlossError::Config("Notebook package byte limit exceeded".into()))?;
            if *bytes > MAX_PORTABLE_ARCHIVE_UNPACKED_BYTES {
                return Err(GlossError::Config(
                    "Notebook package byte limit exceeded".into(),
                ));
            }
            // These two metadata files are deliberately not imported.
            if rel == "manifest.json" || rel == "receipts/export_receipt.json" {
                continue;
            }
            if !is_payload_path(&rel) {
                return Err(GlossError::Config(format!(
                    "Unsupported notebook payload: {rel}"
                )));
            }
            let (sha256, byte_len) = hash_file(&path)?;
            files.push(PortableFileManifestEntry {
                path: rel,
                sha256,
                byte_len,
            });
        } else {
            return Err(GlossError::Config(format!(
                "Symlink or special notebook payload rejected: {rel}"
            )));
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
        if len > MAX_PORTABLE_ARCHIVE_UNPACKED_BYTES {
            return Err(GlossError::Config(
                "Notebook payload byte limit exceeded".into(),
            ));
        }
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
        || path.contains(':')
        || path.len() > 4096
        || path.split('/').count() > 64
        || path
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
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

#[cfg(test)]
mod boundary_regressions {
    use super::*;
    use tempfile::tempdir;

    fn fixture(root: &Path) -> (AppDb, NotebookDb, PathBuf) {
        let app = AppDb::open(&root.join("app.db")).unwrap();
        let notebook = root.join("original");
        fs::create_dir_all(notebook.join("sources")).unwrap();
        app.create_notebook("original", "Original", &notebook.to_string_lossy())
            .unwrap();
        let db = NotebookDb::open(&notebook.join("notebook.db")).unwrap();
        (app, db, notebook)
    }
    fn rewrite_manifest(package: &Path, change: impl FnOnce(&mut NotebookPortableManifest)) {
        let path = package.join("manifest.json");
        let mut manifest: NotebookPortableManifest =
            serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        change(&mut manifest);
        manifest.manifest_digest = digest_manifest_entries(&manifest.files);
        fs::write(path, serde_json::to_vec(&manifest).unwrap()).unwrap();
    }
    fn assert_rejected_without_publication(app: &AppDb, package: &Path, destination: &Path) {
        let before = app.list_notebooks().unwrap().len();
        assert!(import_notebook_package(app, package, destination, None).is_err());
        assert_eq!(app.list_notebooks().unwrap().len(), before);
        assert!(!destination.exists() || fs::read_dir(destination).unwrap().count() == 0);
    }

    #[test]
    fn live_wal_roundtrip_preserves_all_canonical_rows_and_invalidates_projections() {
        let root = tempdir().unwrap();
        let (app, db, original) = fixture(root.path());
        db.conn()
            .execute_batch("PRAGMA wal_checkpoint(TRUNCATE); PRAGMA wal_autocheckpoint=0;")
            .unwrap();
        // Every record below is committed after the checkpoint; keep this writer
        // open through directory and archive export/import.
        db.conn().execute_batch(
            "INSERT INTO sources (id, source_type, title, content_text, status, selected) VALUES ('s', 'text', 'Evidence', 'canonical source text', 'ready', 0);
             INSERT INTO chunks (id, source_id, chunk_index, content, embedding_id, embedding_model) VALUES ('c', 's', 0, 'canonical chunk', 9, 'old-model');
             INSERT INTO conversations (id, title, custom_goal) VALUES ('v', 'Conversation', 'Keep this goal');
             INSERT INTO messages (id, conversation_id, role, content, citations) VALUES ('m', 'v', 'assistant', 'Persisted answer', '[{\"source_id\":\"s\",\"chunk_id\":\"c\"}]');
             INSERT INTO notes (id, title, content, note_type, citations, pinned, source_id) VALUES ('n', 'Saved answer', 'Persisted note', 'saved_response', '[{\"source_id\":\"s\",\"chunk_id\":\"c\"}]', 1, 's');
             INSERT INTO semantic_memory_links (chunk_id, notebook_id, source_id, content_digest, backend_version, sync_status, synced_at) VALUES ('c', 'original', 's', 'digest', 'fixture', 'synced', datetime('now'));"
        ).unwrap();
        fs::create_dir_all(original.join("embeddings")).unwrap();
        fs::write(
            original.join("embeddings/chunks.usearch"),
            b"unpaired generation",
        )
        .unwrap();
        let package = root.path().join("package");
        export_notebook_package(&app, "original", &package).unwrap();
        assert!(!package.join("embeddings/chunks.usearch").exists());
        let manifest = validate_notebook_package(&package).unwrap();
        assert_eq!(manifest.projection_policy, "rebuild_required");
        let imported =
            import_notebook_package(&app, &package, &root.path().join("imports"), None).unwrap();
        let archive = root.path().join("package.tar.gz");
        export_notebook_archive(&app, "original", &archive).unwrap();
        let archived =
            import_notebook_archive(&app, &archive, &root.path().join("imports"), None).unwrap();
        for receipt in [imported, archived] {
            let restored = NotebookDb::open_read_only(
                &Path::new(&receipt.imported_notebook_dir).join("notebook.db"),
            )
            .unwrap();
            for table in ["sources", "chunks", "conversations", "messages", "notes"] {
                let count: i64 = restored
                    .conn()
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
                    .unwrap();
                assert_eq!(count, 1, "missing canonical {table}");
            }
            let selected: i64 = restored
                .conn()
                .query_row("SELECT selected FROM sources", [], |r| r.get(0))
                .unwrap();
            assert_eq!(selected, 0);
            let values: (String, String, String) = restored
                .conn()
                .query_row("SELECT content, citations, note_type FROM notes", [], |r| {
                    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
                })
                .unwrap();
            assert_eq!(
                values,
                (
                    "Persisted note".into(),
                    "[{\"source_id\":\"s\",\"chunk_id\":\"c\"}]".into(),
                    "saved_response".into()
                )
            );
            let mapping: Option<i64> = restored
                .conn()
                .query_row("SELECT embedding_id FROM chunks", [], |r| r.get(0))
                .unwrap();
            assert_eq!(mapping, None);
            assert_eq!(
                restored
                    .embedding_index_metadata("native_hnsw")
                    .unwrap()
                    .unwrap()
                    .status,
                "stale"
            );
            assert_eq!(
                restored
                    .conn()
                    .query_row("SELECT sync_status FROM semantic_memory_links", [], |r| {
                        r.get::<_, String>(0)
                    })
                    .unwrap(),
                "stale"
            );
            assert_eq!(
                app.get_notebook(&receipt.imported_notebook_id)
                    .unwrap()
                    .source_count,
                1
            );
        }
        assert_eq!(
            db.conn()
                .query_row("SELECT embedding_id FROM chunks", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            9
        );
        assert_eq!(
            db.conn()
                .query_row("SELECT sync_status FROM semantic_memory_links", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "synced"
        );
    }

    #[test]
    fn package_inventory_rejects_empty_missing_duplicate_and_unlisted_payloads() {
        for case in [
            "empty",
            "missing",
            "duplicate",
            "extra_source",
            "extra_index",
            "extra_root",
            "wrong_bytes",
        ] {
            let root = tempdir().unwrap();
            let (app, _db, _) = fixture(root.path());
            let package = root.path().join("package");
            export_notebook_package(&app, "original", &package).unwrap();
            match case {
                "empty" => rewrite_manifest(&package, |m| m.files.clear()),
                "missing" => {
                    fs::remove_file(package.join("notebook.db")).unwrap();
                }
                "duplicate" => rewrite_manifest(&package, |m| m.files.push(m.files[0].clone())),
                "extra_source" => {
                    fs::write(package.join("sources/unlisted.txt"), b"unlisted").unwrap();
                }
                "extra_index" => {
                    fs::write(package.join("embeddings/unlisted.index"), b"unlisted").unwrap();
                }
                "extra_root" => {
                    fs::write(package.join("unlisted.txt"), b"unlisted").unwrap();
                }
                "wrong_bytes" => {
                    fs::write(package.join("notebook.db"), b"tampered").unwrap();
                }
                _ => unreachable!(),
            }
            assert_rejected_without_publication(&app, &package, &root.path().join("imports"));
        }
    }

    #[test]
    fn hash_valid_non_database_and_future_schema_are_rejected() {
        for future in [false, true] {
            let root = tempdir().unwrap();
            let (app, _db, _) = fixture(root.path());
            let package = root.path().join("package");
            export_notebook_package(&app, "original", &package).unwrap();
            if future {
                let db = rusqlite::Connection::open(package.join("notebook.db")).unwrap();
                db.execute(
                    "UPDATE _meta SET value='9999' WHERE key='schema_version'",
                    [],
                )
                .unwrap();
            } else {
                fs::write(package.join("notebook.db"), b"self-signed arbitrary bytes").unwrap();
            }
            rewrite_manifest(&package, |m| {
                let (hash, len) = hash_file(&package.join("notebook.db")).unwrap();
                let entry = m
                    .files
                    .iter_mut()
                    .find(|e| e.path == "notebook.db")
                    .unwrap();
                entry.sha256 = hash;
                entry.byte_len = len;
            });
            assert_rejected_without_publication(&app, &package, &root.path().join("imports"));
        }
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_database_source_directory_and_package_root_are_rejected() {
        use std::os::unix::fs::symlink;
        for case in ["database", "sources", "root"] {
            let root = tempdir().unwrap();
            let (app, _db, original) = fixture(root.path());
            let package = root.path().join("package");
            export_notebook_package(&app, "original", &package).unwrap();
            let input = if case == "database" {
                fs::remove_file(package.join("notebook.db")).unwrap();
                symlink(original.join("notebook.db"), package.join("notebook.db")).unwrap();
                package
            } else if case == "sources" {
                fs::remove_dir(package.join("sources")).unwrap();
                symlink(original.join("sources"), package.join("sources")).unwrap();
                package
            } else {
                let link = root.path().join("link");
                symlink(&package, &link).unwrap();
                link
            };
            assert_rejected_without_publication(&app, &input, &root.path().join("imports"));
        }
    }

    #[test]
    fn import_registry_failure_removes_staged_payload_and_preserves_input() {
        let root = tempdir().unwrap();
        let (app, _db, _) = fixture(root.path());
        let package = root.path().join("package");
        export_notebook_package(&app, "original", &package).unwrap();
        app.conn().execute_batch("CREATE TRIGGER fail_import BEFORE INSERT ON notebooks BEGIN SELECT RAISE(ABORT, 'injected insert failure'); END;").unwrap();
        assert_rejected_without_publication(&app, &package, &root.path().join("imports"));
        assert!(validate_notebook_package(&package).is_ok());
    }

    #[test]
    fn failed_source_snapshot_export_never_publishes_partial_output() {
        let root = tempdir().unwrap();
        let (app, db, _) = fixture(root.path());
        db.conn().execute("INSERT INTO sources (id, source_type, title, file_path) VALUES ('missing', 'text', 'Missing asset', 'missing.txt')", []).unwrap();
        let package = root.path().join("package");
        assert!(export_notebook_package(&app, "original", &package).is_err());
        assert!(!package.exists());
        let archive = root.path().join("package.tar.gz");
        assert!(export_notebook_archive(&app, "original", &archive).is_err());
        assert!(!archive.exists());
        assert!(!fs::read_dir(root.path()).unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".gloss-package-")));
    }

    #[test]
    fn archive_duplicate_entries_are_rejected_before_publication() {
        let root = tempdir().unwrap();
        let archive = root.path().join("duplicate.tar.gz");
        let encoder = GzEncoder::new(File::create(&archive).unwrap(), Compression::default());
        let mut builder = Builder::new(encoder);
        for _ in 0..2 {
            let mut header = tar::Header::new_gnu();
            header.set_size(2);
            header.set_mode(0o600);
            header.set_cksum();
            builder
                .append_data(&mut header, "notebook.db", &b"db"[..])
                .unwrap();
        }
        builder.into_inner().unwrap().finish().unwrap();
        assert!(validate_notebook_archive(&archive)
            .unwrap_err()
            .to_string()
            .contains("Duplicate archive"));
    }
}

#[cfg(test)]
mod snapshot_concurrency_regressions {
    use super::*;
    use std::sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    };
    use tempfile::tempdir;

    #[test]
    fn concurrent_commits_never_mix_canonical_generations_in_snapshot() {
        let root = tempdir().unwrap();
        let app = AppDb::open(&root.path().join("app.db")).unwrap();
        let original = root.path().join("original");
        fs::create_dir(&original).unwrap();
        app.create_notebook("n", "Concurrent", &original.to_string_lossy())
            .unwrap();
        let path = original.join("notebook.db");
        let db = NotebookDb::open(&path).unwrap();
        db.conn().execute_batch("INSERT INTO sources (id, source_type, title, content_text) VALUES ('s', 'text', 'Source', '0');
            INSERT INTO notes (id, content, note_type) VALUES ('n', '0', 'manual');").unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let stop = stop.clone();
            let path = path.clone();
            std::thread::spawn(move || {
                let mut conn = rusqlite::Connection::open(path).unwrap();
                conn.busy_timeout(std::time::Duration::from_secs(15))
                    .unwrap();
                let mut generation = 0;
                while !stop.load(Ordering::Acquire) {
                    generation += 1;
                    let tx = conn.transaction().unwrap();
                    tx.execute(
                        "UPDATE sources SET content_text=?1",
                        [generation.to_string()],
                    )
                    .unwrap();
                    tx.execute("UPDATE notes SET content=?1", [generation.to_string()])
                        .unwrap();
                    tx.commit().unwrap();
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                generation
            })
        };
        for iteration in 0..4 {
            let package = root.path().join(format!("package-{iteration}"));
            export_notebook_package(&app, "n", &package).unwrap();
            let snapshot = NotebookDb::open_read_only(&package.join("notebook.db")).unwrap();
            let pair: (String, String) = snapshot
                .conn()
                .query_row(
                    "SELECT sources.content_text, notes.content FROM sources, notes",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .unwrap();
            assert_eq!(pair.0, pair.1, "Mixed transaction generations");
        }
        stop.store(true, Ordering::Release);
        assert!(writer.join().unwrap() > 0);
    }
}
