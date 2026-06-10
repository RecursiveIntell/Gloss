use std::path::{Component, Path, PathBuf};

pub fn redact_path(path: &Path) -> String {
    let label = path
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("path");
    if path.is_absolute() {
        format!("[path]/{label}")
    } else {
        path.to_string_lossy().to_string()
    }
}

/// Resolve `root.join(relative)` and verify the canonicalized result is
/// still inside `root`. Returns the canonical `PathBuf` on success, or
/// `Err(message)` if the path escapes the root or cannot be canonicalized.
///
/// Used to prevent path traversal when reading or deleting user-supplied
/// `file_path` fields stored alongside a notebook source.
pub fn safe_join_under(root: &Path, relative: &str) -> Result<PathBuf, String> {
    // Reject null bytes and any `..` path component up front — these are the
    // classic traversal vectors and we don't want to depend solely on
    // canonicalize for fast rejection.
    if relative.contains('\0') {
        return Err("path contains null byte".into());
    }
    let p = Path::new(relative);
    for comp in p.components() {
        if matches!(comp, Component::ParentDir) {
            return Err(format!("path contains '..' component: {relative}"));
        }
    }

    let joined = root.join(relative);
    let canonical_root = root
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize root {}: {}", redact_path(root), e))?;
    let canonical = joined
        .canonicalize()
        .map_err(|e| format!("cannot canonicalize {}: {}", redact_path(&joined), e))?;
    if !canonical.starts_with(&canonical_root) {
        return Err(format!(
            "path traversal: {} escapes root",
            redact_path(&canonical)
        ));
    }
    Ok(canonical)
}

pub fn redact_text_paths(text: &str) -> String {
    text.split_whitespace()
        .map(|token| {
            if looks_like_path(token) {
                "[path]".to_string()
            } else {
                token.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Redact API-key-like patterns inside JSON string values.
/// Catches keys that start with known prefixes (sk-, key-, gl-, ak-, cpat-, cw-)
/// followed by 20+ alphanumeric characters, Google keys (AIza + 30+ chars),
/// or Bearer tokens, even when embedded inside quoted JSON values.
pub fn redact_json_embedded_secrets(text: &str) -> String {
    static RE: std::sync::OnceLock<regex::Regex> = std::sync::OnceLock::new();
    let re = RE.get_or_init(|| {
        regex::Regex::new(
            r#"(sk-|key-|gl-|ak-|cpat-|cw-)[A-Za-z0-9_-]{20,}|AIza[A-Za-z0-9_-]{30,}|Bearer\s+[A-Za-z0-9_\-\.]{20,}"#,
        ).expect("static redaction regex")
    });
    re.replace_all(text, "[REDACTED]").to_string()
}

fn looks_like_path(token: &str) -> bool {
    let trimmed = token
        .trim_matches(|c: char| matches!(c, '"' | '\'' | '(' | ')' | '[' | ']' | ',' | ';' | ':'));
    trimmed.starts_with('/')
        || trimmed.starts_with("~/")
        || trimmed.starts_with("../")
        || trimmed.starts_with("./")
        || trimmed.contains("\\Users\\")
        || trimmed.contains(":\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_path_keeps_only_leaf_for_absolute_paths() {
        assert_eq!(
            redact_path(Path::new("/home/example/Gloss/notebook.db")),
            "[path]/notebook.db"
        );
        assert_eq!(
            redact_path(Path::new("relative/source.txt")),
            "relative/source.txt"
        );
    }

    #[test]
    fn redact_text_paths_removes_unix_and_windows_paths() {
        let text = redact_text_paths("failed /home/example/a.txt and C:\\Users\\me\\b.txt");
        assert!(!text.contains("/home/example"));
        assert!(!text.contains("C:\\Users"));
        assert!(text.contains("[path]"));
    }

    #[test]
    fn safe_join_under_accepts_leaf_within_root() {
        // Create a temp root with a leaf file.
        let root = std::env::temp_dir().join(format!(
            "gloss_safe_join_accept_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("hello.txt"), "hi").unwrap();

        let p = safe_join_under(&root, "hello.txt").expect("leaf should join");
        assert!(p.ends_with("hello.txt"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn safe_join_under_rejects_parent_traversal() {
        let root = std::env::temp_dir().join(format!(
            "gloss_safe_join_reject_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let err = safe_join_under(&root, "../etc/passwd").expect_err("must reject '..'");
        assert!(err.contains(".."), "error should mention '..'; got: {err}");

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn safe_join_under_rejects_null_bytes() {
        let root = std::env::temp_dir();
        let err = safe_join_under(&root, "good\0bad").expect_err("must reject null bytes");
        assert!(
            err.contains("null"),
            "error should mention null; got: {err}"
        );
    }
}
