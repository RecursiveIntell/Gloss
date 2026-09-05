//! Configured embedding identity and strict transport response validation.
use crate::db::app_db::AppDb;
use crate::error::GlossError;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NativeEmbeddingConfig {
    pub provider: String,
    pub url: String,
    pub model: String,
    pub timeout_secs: u64,
    pub allow_lan: bool,
    pub download_consent: bool,
}

impl NativeEmbeddingConfig {
    pub fn read(db: &AppDb) -> Result<Self, GlossError> {
        // One SELECT gives queued workers a consistent settings snapshot even
        // when the app commits an atomic configuration update concurrently.
        let settings = db.get_settings()?;
        let provider = settings
            .get("semantic_memory_embedding_provider")
            .cloned()
            .unwrap_or_else(|| "ollama".into());
        if !matches!(provider.as_str(), "ollama" | "fastembed" | "native") {
            return Err(GlossError::Config(format!(
                "Unsupported embedding provider: {provider}"
            )));
        }
        let timeout_secs = match settings
            .get("semantic_memory_embedding_timeout_secs")
            .cloned()
        {
            Some(value) => value
                .parse::<u64>()
                .ok()
                .filter(|value| (2..=300).contains(value))
                .ok_or_else(|| {
                    GlossError::Config(
                        "Embedding timeout must be an integer from 2 to 300 seconds".into(),
                    )
                })?,
            None => 60,
        };
        Ok(Self {
            provider,
            url: settings
                .get("semantic_memory_embedding_url")
                .cloned()
                .unwrap_or_else(|| "http://localhost:11434".into()),
            model: settings
                .get("semantic_memory_embedding_model")
                .cloned()
                .unwrap_or_else(|| "bge-m3".into()),
            timeout_secs,
            allow_lan: settings
                .get("allow_lan_local_providers")
                .cloned()
                .as_deref()
                == Some("true"),
            download_consent: settings
                .get("fastembed_download_consent")
                .cloned()
                .as_deref()
                == Some("true"),
        })
    }
}

pub fn parse_embeddings(
    value: &serde_json::Value,
    expected_count: usize,
    expected_dims: Option<usize>,
) -> Result<Vec<Vec<f32>>, GlossError> {
    let fail = |message: &str| GlossError::Embedding(message.into());
    let rows = value
        .get("embeddings")
        .and_then(|value| value.as_array())
        .ok_or_else(|| fail("Embedding response must contain an embeddings array"))?;
    if rows.len() != expected_count {
        return Err(fail("Embedding response count does not match input count"));
    }
    let mut dims = expected_dims;
    let mut vectors = Vec::with_capacity(rows.len());
    for row in rows {
        let values = row
            .as_array()
            .ok_or_else(|| fail("Embedding must be a numeric array"))?;
        if values.is_empty() || dims.is_some_and(|dims| dims != values.len()) {
            return Err(fail(
                "Embedding response has empty or inconsistent dimensions",
            ));
        }
        dims = Some(values.len());
        let vector = values
            .iter()
            .map(|value| {
                let number = value
                    .as_f64()
                    .ok_or_else(|| fail("Embedding contains a nonnumeric value"))?
                    as f32;
                if !number.is_finite() {
                    return Err(fail("Embedding contains a nonfinite value"));
                }
                Ok(number)
            })
            .collect::<Result<Vec<_>, GlossError>>()?;
        vectors.push(vector);
    }
    Ok(vectors)
}

pub fn ollama_client(
    url: &str,
    model: &str,
    timeout_secs: u64,
    allow_lan: bool,
) -> Result<reqwest::Client, GlossError> {
    crate::providers::validate_embedding_url(url, allow_lan)?;
    if model.trim().is_empty() {
        return Err(GlossError::Config(
            "Embedding model must not be empty".into(),
        ));
    }
    let timeout = std::time::Duration::from_secs(timeout_secs.clamp(2, 300));
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| GlossError::Embedding(format!("HTTP client build failed: {e}")))?;
    Ok(client)
}

/// Run an Ollama `/api/embed` request on the async reqwest client from both
/// sync and async callers without panicking. The old `reqwest::blocking`
/// client panicked ("Cannot drop a runtime in a context where blocking is not
/// allowed") whenever it was used or dropped inside a tokio async context —
/// which is how `ensure_embedder` and the import path call it. This mirrors
/// the semantic-memory adapter's proven `block_on_probe` pattern:
/// `block_in_place` inside an existing runtime, a throwaway current-thread
/// runtime otherwise.
pub fn ollama_embed_sync(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>, GlossError> {
    block_on_ollama(ollama_embed_request(client, url, model, texts))
}

/// Synchronous owners keep the work joined to their call and inference guard.
/// Async owners must await the request directly so dropping their future cancels I/O.
pub fn block_on_ollama<T: Send>(
    future: impl std::future::Future<Output = Result<T, GlossError>> + Send,
) -> Result<T, GlossError> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) if handle.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread => {
            tokio::task::block_in_place(|| handle.block_on(future))
        }
        Ok(_) => std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let runtime = tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|e| {
                            GlossError::Embedding(format!("Ollama embed runtime build failed: {e}"))
                        })?;
                    runtime.block_on(future)
                })
                .join()
                .map_err(|_| GlossError::Embedding("Ollama embedding worker panicked".into()))?
        }),
        Err(_) => {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|e| {
                    GlossError::Embedding(format!("Ollama embed runtime build failed: {e}"))
                })?;
            runtime.block_on(future)
        }
    }
}

pub async fn ollama_embed_request(
    client: &reqwest::Client,
    url: &str,
    model: &str,
    texts: &[&str],
) -> Result<Vec<Vec<f32>>, GlossError> {
    let body = serde_json::json!({
        "model": model,
        "input": texts,
    });
    let response = client
        .post(format!("{}/api/embed", url.trim_end_matches('/')))
        .json(&body)
        .send()
        .await
        .map_err(|e| GlossError::Embedding(format!("Ollama embed request failed: {e}")))?;

    if !response.status().is_success() {
        let status = response.status();
        return Err(GlossError::Embedding(format!(
            "Ollama embed returned HTTP {status}"
        )));
    }

    let parsed = bounded_ollama_json(response, MAX_EMBEDDING_RESPONSE_BYTES).await?;

    parse_embeddings(&parsed, texts.len(), None)
}

/// Caps raw JSON before parsing, including chunked bodies without Content-Length.
/// Sixteen MiB accommodates normal embedding batches while bounding response memory.
pub const MAX_EMBEDDING_RESPONSE_BYTES: usize = 16 * 1024 * 1024;
const MAX_MODEL_INFO_RESPONSE_BYTES: usize = 1024 * 1024;

pub async fn bounded_ollama_json(
    mut response: reqwest::Response,
    limit: usize,
) -> Result<serde_json::Value, GlossError> {
    let oversized = || GlossError::Embedding(format!("Ollama JSON response exceeds {limit} bytes"));
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(oversized());
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| GlossError::Embedding(format!("Ollama response read failed: {error}")))?
    {
        if chunk.len() > limit.saturating_sub(body.len()) {
            return Err(oversized());
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body)
        .map_err(|error| GlossError::Embedding(format!("Ollama JSON parse failed: {error}")))
}

fn dimension_from_model_info(value: &serde_json::Value) -> Option<usize> {
    match value {
        serde_json::Value::Object(map) => {
            for (key, value) in map {
                let key = key.to_ascii_lowercase();
                if key.ends_with("embedding_length")
                    || key.ends_with("embedding_dimensions")
                    || key == "dimensions"
                {
                    if let Some(dimension) = value.as_u64().and_then(|v| usize::try_from(v).ok()) {
                        if dimension > 0 {
                            return Some(dimension);
                        }
                    }
                }
            }
            // Only named fields establish a dimension; unrelated numbers such
            // as context_length or parameter_count must never become defaults.
            map.values().find_map(dimension_from_model_info)
        }
        serde_json::Value::Array(values) => values.iter().find_map(dimension_from_model_info),
        _ => None,
    }
}

/// Probe with the same client authority and body/vector validation as indexing.
/// Existing /api/show fallback is retained only after a failed embed request;
/// malformed successful vectors and redirects fail closed.
pub async fn ollama_probe_dimension(
    url: &str,
    model: &str,
    timeout_secs: u64,
    allow_lan: bool,
) -> Result<usize, GlossError> {
    let client = ollama_client(url, model, timeout_secs, allow_lan)?;
    let url = url.trim_end_matches('/');
    let embed_error = match client
        .post(format!("{url}/api/embed"))
        .json(&serde_json::json!({"model": model, "input": ["semantic-memory-dimension-probe"]}))
        .send()
        .await
    {
        Ok(response) if response.status().is_success() => {
            let json = bounded_ollama_json(response, MAX_EMBEDDING_RESPONSE_BYTES).await?;
            return Ok(parse_embeddings(&json, 1, None)?[0].len());
        }
        Ok(response) if response.status().is_redirection() => {
            return Err(GlossError::Embedding(format!(
                "Ollama /api/embed dimension probe rejected redirect HTTP {}",
                response.status()
            )));
        }
        Ok(response) => format!("HTTP {}", response.status()),
        Err(error) => error.to_string(),
    };
    let response = client
        .post(format!("{url}/api/show"))
        .json(&serde_json::json!({"model": model}))
        .send()
        .await
        .map_err(|error| {
            GlossError::Embedding(format!(
                "Ollama dimension probe failed: /api/embed {embed_error}; /api/show {error}"
            ))
        })?;
    if !response.status().is_success() {
        return Err(GlossError::Embedding(format!(
            "Ollama dimension probe failed: /api/embed {embed_error}; /api/show HTTP {}",
            response.status()
        )));
    }
    let json = bounded_ollama_json(response, MAX_MODEL_INFO_RESPONSE_BYTES).await?;
    dimension_from_model_info(&json).ok_or_else(|| GlossError::Embedding(format!(
        "Ollama dimension probe failed: /api/embed {embed_error}; /api/show returned no embedding_length metadata"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn malformed_vectors_are_never_coerced() {
        for value in [
            json!({"embeddings":[["1",0]]}),
            json!({"embeddings":[[null,0]]}),
            json!({"embeddings":[[1e100,0]]}),
            json!({"embeddings":[[]]}),
            json!({"embeddings":[[1,0],[1]]}),
            json!({"embeddings":[]}),
        ] {
            assert!(parse_embeddings(&value, 1, Some(2)).is_err(), "{value}");
        }
        assert!(parse_embeddings(&json!({"embeddings":[[1,0],[1]]}), 2, None).is_err());
        assert_eq!(
            parse_embeddings(&json!({"embeddings":[[1,0]]}), 1, None).unwrap(),
            vec![vec![1.0, 0.0]]
        );
    }

    #[test]
    fn configuration_requires_known_provider_and_explicit_lan_authority() {
        let dir = tempfile::tempdir().unwrap();
        let db = AppDb::open(&dir.path().join("app.db")).unwrap();
        assert!(!NativeEmbeddingConfig::read(&db).unwrap().allow_lan);
        db.set_setting("semantic_memory_embedding_provider", "unknown")
            .unwrap();
        assert!(NativeEmbeddingConfig::read(&db).is_err());
        db.set_setting("semantic_memory_embedding_provider", "ollama")
            .unwrap();
        db.set_setting("allow_lan_local_providers", "true").unwrap();
        assert!(NativeEmbeddingConfig::read(&db).unwrap().allow_lan);
        db.set_setting("semantic_memory_embedding_timeout_secs", "oops")
            .unwrap();
        assert!(NativeEmbeddingConfig::read(&db).is_err());
    }

    #[test]
    fn native_embedding_client_rejects_unauthorized_endpoints_before_io() {
        for url in [
            "https://example.com",
            "http://192.168.1.1:11434",
            "http://localhost:11434?secret=1",
        ] {
            assert!(ollama_client(url, "fixture", 2, false).is_err());
        }
        assert!(ollama_client("http://192.168.1.1:11434", "fixture", 2, true).is_ok());
    }

    #[test]
    fn native_embedding_http_refuses_redirect_without_forwarding_source_text() {
        use std::io::{Read, Write};
        let target = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        target.set_nonblocking(true).unwrap();
        let target_addr = target.local_addr().unwrap();
        let source = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", source.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = source.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            stream.read(&mut buffer).unwrap();
            write!(stream,"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{target_addr}/api/embed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").unwrap();
        });
        let client = ollama_client(&url, "fixture", 2, false).unwrap();
        let error = ollama_embed_sync(&client, &url, "fixture", &["private source text"])
            .unwrap_err()
            .to_string();
        assert!(error.contains("307"));
        assert!(!error.contains("private source text"));
        server.join().unwrap();
        assert_eq!(
            target.accept().unwrap_err().kind(),
            std::io::ErrorKind::WouldBlock
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn native_embedding_http_operates_inside_current_thread_runtime() {
        use std::io::{Read, Write};
        let server = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.local_addr().unwrap());
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = server.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                .unwrap();
            let mut buffer = [0; 4096];
            stream.read(&mut buffer).unwrap();
            let body = r#"{"embeddings":[[1,0,0]]}"#;
            write!(stream,"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).unwrap();
        });
        let client = ollama_client(&url, "fixture", 2, false).unwrap();
        assert_eq!(
            ollama_embed_sync(&client, &url, "fixture", &["one"]).unwrap(),
            vec![vec![1.0, 0.0, 0.0]]
        );
        worker.join().unwrap();
    }
}
