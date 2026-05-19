use semantic_memory::db::{bytes_to_embedding, decode_f32_le, embedding_to_bytes};
use semantic_memory::embedder::{format_ollama_http_error, parse_embedding_response};
use semantic_memory::StoragePaths;
use semantic_memory::{MemoryConfig, MemoryStore, MockEmbedder};
use tempfile::TempDir;

// ─── bytes_to_embedding (Fix 1: stable Rust compat) ─────────

#[test]
fn test_bytes_to_embedding_valid() {
    let original = vec![1.0f32, 2.0, 3.0];
    let bytes = embedding_to_bytes(&original);
    let decoded = bytes_to_embedding(&bytes).unwrap();
    assert_eq!(original, decoded);
}

#[test]
fn test_bytes_to_embedding_invalid_length() {
    let bytes = vec![0u8; 5]; // Not divisible by 4
    let result = bytes_to_embedding(&bytes);
    assert!(result.is_err());
}

#[test]
fn test_bytes_to_embedding_empty() {
    let bytes: Vec<u8> = vec![];
    let decoded = bytes_to_embedding(&bytes).unwrap();
    assert!(decoded.is_empty());
}

#[test]
fn test_bytes_to_embedding_roundtrip_large() {
    let original: Vec<f32> = (0..768).map(|i| (i as f32) * 0.001).collect();
    let bytes = embedding_to_bytes(&original);
    let decoded = bytes_to_embedding(&bytes).unwrap();
    assert_eq!(original.len(), decoded.len());
    for (a, b) in original.iter().zip(decoded.iter()) {
        assert!((a - b).abs() < 1e-7, "Values should match: {} vs {}", a, b);
    }
}

#[test]
fn test_decode_f32_le_rejects_wrong_dimension() {
    let bytes = embedding_to_bytes(&[1.0, 2.0]);
    let err = decode_f32_le(&bytes, 3).unwrap_err();
    assert_eq!(err.kind(), "vector_blob_length_mismatch");
}

#[test]
fn test_bytes_to_embedding_rejects_non_finite() {
    let bytes = embedding_to_bytes(&[1.0, f32::INFINITY]);
    let err = bytes_to_embedding(&bytes).unwrap_err();
    assert_eq!(err.kind(), "non_finite_embedding_value");
}

// ─── embeddings_dirty default is false ──────────────────────

#[tokio::test]
async fn test_fresh_db_not_dirty() {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    assert!(!store.embeddings_are_dirty().await.unwrap());
}

#[tokio::test]
async fn migration_creates_derived_vector_artifacts_table() {
    let tmp = TempDir::new().unwrap();
    let config = MemoryConfig {
        base_dir: tmp.path().to_path_buf(),
        ..Default::default()
    };
    let embedder = Box::new(MockEmbedder::new(768));
    let _store = MemoryStore::open_with_embedder(config, embedder).unwrap();
    let paths = StoragePaths::new(tmp.path());
    let conn = rusqlite::Connection::open(paths.sqlite_path).unwrap();
    let table_exists: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master
             WHERE type = 'table' AND name = 'derived_vector_artifacts'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(table_exists, 1);
    let columns = conn
        .prepare("PRAGMA table_info(derived_vector_artifacts)")
        .unwrap()
        .query_map([], |row| row.get::<_, String>(1))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    for required in [
        "item_key",
        "codec_family",
        "codec_profile_digest",
        "source_embedding_digest",
        "encoded_digest",
        "encoding",
        "dim",
        "encoded",
        "status",
    ] {
        assert!(
            columns.iter().any(|column| column == required),
            "missing derived_vector_artifacts column {required}"
        );
    }
}

// ─── parse_embedding_response (Fix 3) ───────────────────────

#[test]
fn test_parse_rejects_non_numeric() {
    let body = serde_json::json!({
        "embeddings": [[1.0, "bad", 3.0]]
    });
    let result = parse_embedding_response(&body, 3);
    assert!(result.is_err());
    assert!(
        result.unwrap_err().to_string().contains("non-numeric"),
        "Error should mention non-numeric value"
    );
}

#[test]
fn test_parse_valid_embedding() {
    let body = serde_json::json!({
        "embeddings": [[1.0, 2.0, 3.0]]
    });
    let result = parse_embedding_response(&body, 3);
    assert!(result.is_ok());
    assert_eq!(result.unwrap()[0], vec![1.0f32, 2.0, 3.0]);
}

#[test]
fn test_parse_wrong_dimensions() {
    let body = serde_json::json!({
        "embeddings": [[1.0, 2.0, 3.0]]
    });
    // Expect 5 dims but got 3
    let result = parse_embedding_response(&body, 5);
    assert!(result.is_err());
}

#[test]
fn test_parse_multiple_embeddings() {
    let body = serde_json::json!({
        "embeddings": [[1.0, 2.0], [3.0, 4.0]]
    });
    let result = parse_embedding_response(&body, 2);
    assert!(result.is_ok());
    let embeddings = result.unwrap();
    assert_eq!(embeddings.len(), 2);
    assert_eq!(embeddings[0], vec![1.0f32, 2.0]);
    assert_eq!(embeddings[1], vec![3.0f32, 4.0]);
}

#[test]
fn test_ollama_http_error_preserves_body_read_failure() {
    let err = format_ollama_http_error(
        reqwest::StatusCode::INTERNAL_SERVER_ERROR,
        Err("failed to read Ollama error body: connection reset".into()),
    );
    let msg = err.to_string();
    assert!(msg.contains("HTTP 500"));
    assert!(msg.contains("failed to read Ollama error body"));
}

// ─── Role trait impls (Fix 7) ───────────────────────────────

#[test]
fn test_role_display() {
    use semantic_memory::Role;
    assert_eq!(format!("{}", Role::User), "user");
    assert_eq!(format!("{}", Role::Assistant), "assistant");
    assert_eq!(format!("{}", Role::System), "system");
    assert_eq!(format!("{}", Role::Tool), "tool");
}

#[test]
fn test_role_from_str() {
    use semantic_memory::Role;
    assert_eq!("user".parse::<Role>().unwrap(), Role::User);
    assert_eq!("assistant".parse::<Role>().unwrap(), Role::Assistant);
    assert_eq!("system".parse::<Role>().unwrap(), Role::System);
    assert_eq!("tool".parse::<Role>().unwrap(), Role::Tool);
    assert!("invalid".parse::<Role>().is_err());
}
