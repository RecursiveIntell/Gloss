use std::path::Path;

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
}
