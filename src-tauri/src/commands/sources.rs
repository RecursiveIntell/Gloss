use crate::db::notebook_db::Source;
use crate::error::GlossError;
use crate::state::AppState;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use tauri::{Emitter, State};

#[derive(Debug, Serialize)]
pub struct SourceContent {
    pub content_text: Option<String>,
    pub word_count: Option<i32>,
}

/// Classify a file extension into (source_type, optional language).
fn classify_extension(ext: &str) -> (&'static str, Option<&'static str>) {
    match ext {
        "txt" => ("text", None),
        "md" | "markdown" | "rst" => ("markdown", None),

        // Code files
        "py" => ("code", Some("python")),
        "js" => ("code", Some("javascript")),
        "jsx" => ("code", Some("jsx")),
        "ts" => ("code", Some("typescript")),
        "tsx" => ("code", Some("tsx")),
        "rs" => ("code", Some("rust")),
        "go" => ("code", Some("go")),
        "java" => ("code", Some("java")),
        "c" => ("code", Some("c")),
        "cpp" | "cc" | "cxx" => ("code", Some("cpp")),
        "h" | "hpp" => ("code", Some("c_header")),
        "cs" => ("code", Some("csharp")),
        "rb" => ("code", Some("ruby")),
        "php" => ("code", Some("php")),
        "swift" => ("code", Some("swift")),
        "kt" | "kts" => ("code", Some("kotlin")),
        "scala" => ("code", Some("scala")),
        "lua" => ("code", Some("lua")),
        "r" => ("code", Some("r")),
        "sql" => ("code", Some("sql")),
        "sh" | "bash" | "zsh" => ("code", Some("shell")),
        "css" => ("code", Some("css")),
        "scss" | "sass" => ("code", Some("scss")),
        "html" | "htm" => ("code", Some("html")),
        "xml" => ("code", Some("xml")),
        "json" => ("code", Some("json")),
        "yaml" | "yml" => ("code", Some("yaml")),
        "toml" => ("code", Some("toml")),
        "ini" | "cfg" | "conf" => ("code", Some("config")),
        "vue" => ("code", Some("vue")),
        "svelte" => ("code", Some("svelte")),
        "dart" => ("code", Some("dart")),
        "ex" | "exs" => ("code", Some("elixir")),
        "zig" => ("code", Some("zig")),
        "nim" => ("code", Some("nim")),
        "pl" | "pm" => ("code", Some("perl")),
        "proto" => ("code", Some("protobuf")),
        "graphql" | "gql" => ("code", Some("graphql")),
        "tf" | "hcl" => ("code", Some("terraform")),
        "dockerfile" => ("code", Some("dockerfile")),
        "makefile" => ("code", Some("makefile")),

        // Images
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" | "svg" | "tiff" | "tif" => {
            ("image", None)
        }

        // Video
        "mp4" | "webm" | "mov" | "avi" | "mkv" => ("video", None),

        _ => ("text", None),
    }
}

/// Check if a file extension is supported for import.
fn is_supported_extension(ext: &str) -> bool {
    let (source_type, _) = classify_extension(ext);
    // All classified types are supported
    source_type != "text" || ext == "txt"
}

/// Create a file source record: copies file to notebook dir, inserts DB row.
/// Returns (source_id, source_type).
fn create_file_source(
    notebook_id: &str,
    source_path: &Path,
    state: &AppState,
) -> Result<(String, String), GlossError> {
    let filename = source_path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown")
        .to_string();

    let extension = source_path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    let (source_type, language) = classify_extension(&extension);

    // Read file and compute hash
    let content = std::fs::read(source_path).map_err(|e| GlossError::Ingestion {
        source_id: String::new(),
        message: format!("Failed to read file: {}", e),
    })?;
    let hash = format!("{:x}", Sha256::digest(&content));

    // Copy file to notebook sources directory
    let nb_dir = {
        let app_db = state
            .app_db
            .lock()
            .map_err(|e| GlossError::Other(e.to_string()))?;
        let nb = app_db.get_notebook(notebook_id)?;
        PathBuf::from(nb.directory)
    };

    let dest_filename = format!("{}.{}", &hash[..16], extension);
    let dest_path = nb_dir.join("sources").join(&dest_filename);

    // Skip copy if destination already exists (dedup)
    if !dest_path.exists() {
        std::fs::copy(source_path, &dest_path)?;
    }

    // Build metadata JSON with language if present
    let metadata = language.map(|lang| {
        serde_json::json!({ "language": lang }).to_string()
    });

    // Create source record
    let source_id = uuid::Uuid::new_v4().to_string();
    let source = Source {
        id: source_id.clone(),
        source_type: source_type.to_string(),
        title: filename.clone(),
        original_filename: Some(filename),
        file_hash: Some(hash),
        url: None,
        file_path: Some(dest_filename),
        content_text: None,
        word_count: None,
        metadata,
        summary: None,
        summary_model: None,
        status: "pending".to_string(),
        error_message: None,
        selected: true,
        created_at: String::new(),
        updated_at: String::new(),
    };

    state.with_notebook_db(notebook_id, |db| db.insert_source(&source))?;

    Ok((source_id, source_type.to_string()))
}

#[tauri::command]
pub async fn list_sources(
    notebook_id: String,
    state: State<'_, AppState>,
) -> Result<Vec<Source>, GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.list_sources())
}

#[tauri::command]
pub async fn add_source_file(
    notebook_id: String,
    path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<String, GlossError> {
    let source_path = PathBuf::from(&path);
    let (source_id, _source_type) = create_file_source(&notebook_id, &source_path, &state)?;

    // Emit status event
    let _ = app_handle.emit(
        "source:status",
        serde_json::json!({
            "notebook_id": notebook_id,
            "source_id": source_id,
            "status": "pending",
        }),
    );

    // Run ingestion in background
    let nb_id = notebook_id.clone();
    let sid = source_id.clone();
    let handle = app_handle.clone();

    tokio::spawn(async move {
        // Simplified inline ingestion for Phase 1
        // In Phase 2, this would be a tauri-queue job
        tracing::info!(source_id = %sid, "Starting ingestion");

        let _ = handle.emit(
            "source:status",
            serde_json::json!({
                "notebook_id": nb_id,
                "source_id": sid,
                "status": "extracting",
            }),
        );
    });

    Ok(source_id)
}

/// Directories to skip when walking folders.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "__pycache__",
    ".git",
    "dist",
    "build",
    "vendor",
    ".venv",
    "venv",
    ".next",
    ".nuxt",
    ".cache",
];

/// Recursively walk a directory and collect supported file paths.
fn walk_directory(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), GlossError> {
    let entries = std::fs::read_dir(dir).map_err(|e| GlossError::Ingestion {
        source_id: String::new(),
        message: format!("Failed to read directory {}: {}", dir.display(), e),
    })?;

    for entry in entries {
        let entry = entry.map_err(|e| GlossError::Ingestion {
            source_id: String::new(),
            message: format!("Failed to read entry: {}", e),
        })?;

        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip hidden files/dirs
        if name_str.starts_with('.') {
            continue;
        }

        if path.is_dir() {
            // Skip junk directories
            if SKIP_DIRS.contains(&name_str.as_ref()) {
                continue;
            }
            walk_directory(&path, out)?;
        } else if path.is_file() {
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();
            if is_supported_extension(&ext) {
                out.push(path);
            }
        }
    }

    Ok(())
}

#[tauri::command]
pub async fn add_source_folder(
    notebook_id: String,
    path: String,
    state: State<'_, AppState>,
    app_handle: tauri::AppHandle,
) -> Result<Vec<String>, GlossError> {
    let folder = PathBuf::from(&path);
    if !folder.is_dir() {
        return Err(GlossError::Ingestion {
            source_id: String::new(),
            message: format!("Not a directory: {}", path),
        });
    }

    let mut files = Vec::new();
    walk_directory(&folder, &mut files)?;

    let mut source_ids = Vec::new();
    for file_path in &files {
        match create_file_source(&notebook_id, file_path, &state) {
            Ok((source_id, _source_type)) => {
                let _ = app_handle.emit(
                    "source:status",
                    serde_json::json!({
                        "notebook_id": notebook_id,
                        "source_id": source_id,
                        "status": "pending",
                    }),
                );
                source_ids.push(source_id);
            }
            Err(e) => {
                tracing::warn!(
                    file = %file_path.display(),
                    error = %e,
                    "Skipping file during folder import"
                );
            }
        }
    }

    tracing::info!(
        folder = %path,
        count = source_ids.len(),
        "Folder import complete"
    );

    Ok(source_ids)
}

#[tauri::command]
pub async fn add_source_paste(
    notebook_id: String,
    title: String,
    text: String,
    state: State<'_, AppState>,
) -> Result<String, GlossError> {
    let source_id = uuid::Uuid::new_v4().to_string();
    let word_count = text.split_whitespace().count() as i32;

    let source = Source {
        id: source_id.clone(),
        source_type: "paste".to_string(),
        title,
        original_filename: None,
        file_hash: None,
        url: None,
        file_path: None,
        content_text: Some(text),
        word_count: Some(word_count),
        metadata: None,
        summary: None,
        summary_model: None,
        status: "pending".to_string(),
        error_message: None,
        selected: true,
        created_at: String::new(),
        updated_at: String::new(),
    };

    state.with_notebook_db(&notebook_id, |db| db.insert_source(&source))?;

    Ok(source_id)
}

#[tauri::command]
pub async fn delete_source(
    notebook_id: String,
    source_id: String,
    state: State<'_, AppState>,
) -> Result<(), GlossError> {
    state.with_notebook_db(&notebook_id, |db| db.delete_source(&source_id))
}

#[tauri::command]
pub async fn get_source_content(
    notebook_id: String,
    source_id: String,
    state: State<'_, AppState>,
) -> Result<SourceContent, GlossError> {
    state.with_notebook_db(&notebook_id, |db| {
        let source = db.get_source(&source_id)?;
        Ok(SourceContent {
            content_text: source.content_text,
            word_count: source.word_count,
        })
    })
}
