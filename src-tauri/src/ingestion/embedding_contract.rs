//! Configured embedding identity and strict transport response validation.
use crate::db::app_db::AppDb;
use crate::error::GlossError;

pub(crate) const DEFAULT_EMBEDDING_URL: &str = "http://localhost:11434";
pub(crate) const DEFAULT_OLLAMA_EMBEDDING_MODEL: &str = "bge-m3";
pub(crate) const DEFAULT_EMBEDDING_TIMEOUT_SECS: u64 = 60;
pub(crate) const BUILT_IN_EMBEDDING_MODEL: &str = "nomic-ai/nomic-embed-text-v1.5";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EmbeddingProviderKind {
    FastEmbed,
    Native,
    Ollama,
}

impl EmbeddingProviderKind {
    pub fn from_setting(value: Option<&str>) -> Result<Self, GlossError> {
        match value {
            None | Some("ollama") => Ok(Self::Ollama),
            Some("fastembed") => Ok(Self::FastEmbed),
            Some("native") => Ok(Self::Native),
            Some(value) => Err(GlossError::Config(format!(
                "Unsupported embedding provider: {value}"
            ))),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::FastEmbed => "fastembed",
            Self::Native => "native",
            Self::Ollama => "ollama",
        }
    }
}

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
        Self::from_settings(
            settings
                .get("semantic_memory_embedding_provider")
                .map(String::as_str),
            settings
                .get("semantic_memory_embedding_url")
                .map(String::as_str),
            settings
                .get("semantic_memory_embedding_model")
                .map(String::as_str),
            settings
                .get("semantic_memory_embedding_timeout_secs")
                .map(String::as_str),
            settings
                .get("allow_lan_local_providers")
                .map(String::as_str)
                == Some("true"),
            settings
                .get("fastembed_download_consent")
                .map(String::as_str)
                == Some("true"),
        )
    }

    /// Both ingestion and semantic projection use this strict configuration
    /// owner before constructing an embedder or sending source text.
    pub fn from_settings(
        provider: Option<&str>,
        url: Option<&str>,
        model: Option<&str>,
        timeout_secs: Option<&str>,
        allow_lan: bool,
        download_consent: bool,
    ) -> Result<Self, GlossError> {
        let provider = EmbeddingProviderKind::from_setting(provider)?;
        let timeout_secs = match timeout_secs {
            Some(value) => value
                .parse::<u64>()
                .ok()
                .filter(|value| (2..=300).contains(value))
                .ok_or_else(|| {
                    GlossError::Config(
                        "Embedding timeout must be an integer from 2 to 300 seconds".into(),
                    )
                })?,
            None => DEFAULT_EMBEDDING_TIMEOUT_SECS,
        };
        let url = url.unwrap_or(DEFAULT_EMBEDDING_URL);
        if url.trim().is_empty() {
            return Err(GlossError::Config("Embedding URL must not be empty".into()));
        }
        let model = model.unwrap_or(match provider {
            EmbeddingProviderKind::Ollama => DEFAULT_OLLAMA_EMBEDDING_MODEL,
            EmbeddingProviderKind::FastEmbed | EmbeddingProviderKind::Native => {
                BUILT_IN_EMBEDDING_MODEL
            }
        });
        if model.trim().is_empty() {
            return Err(GlossError::Config(
                "Embedding model must not be empty".into(),
            ));
        }
        if provider == EmbeddingProviderKind::Ollama {
            crate::providers::validate_embedding_url(url, allow_lan)?;
        }
        // Built-in providers have one fixed model; retain the explicit provider
        // identity while recording the model that will actually produce vectors.
        let model = match provider {
            EmbeddingProviderKind::Ollama => model,
            EmbeddingProviderKind::FastEmbed | EmbeddingProviderKind::Native => {
                BUILT_IN_EMBEDDING_MODEL
            }
        };
        Ok(Self {
            provider: provider.as_str().to_string(),
            url: url.to_string(),
            model: model.to_string(),
            timeout_secs,
            allow_lan,
            download_consent,
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
    let endpoint = format!("{}/api/embed", url.trim_end_matches('/'));
    let endpoint = endpoint.as_str();
    // The configured client timeout still applies to each request. A hard
    // whole-operation ceiling prevents an arbitrarily long sequence of batches.
    // Direct await keeps cancellation and inference ownership with the caller.
    tokio::time::timeout(
        std::time::Duration::from_secs(300),
        ollama_embed_batches(model, texts, |body| async move {
            let response = client
                .post(endpoint)
                .header(reqwest::header::CONTENT_TYPE, "application/json")
                .body(body)
                .send()
                .await
                .map_err(|e| GlossError::Embedding(format!("Ollama embed request failed: {e}")))?;
            if !response.status().is_success() {
                let status = response.status();
                return Err(GlossError::Embedding(format!(
                    "Ollama embed returned HTTP {status}"
                )));
            }
            bounded_ollama_json(response, MAX_EMBEDDING_RESPONSE_BYTES).await
        }),
    )
    .await
    .map_err(|_| GlossError::Embedding("Ollama embedding operation timed out".into()))?
}

/// Match queue/rebuild cardinality while also bounding escaped JSON bytes.
pub const MAX_EMBEDDING_BATCH_INPUTS: usize = 64;
pub const MAX_EMBEDDING_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(serde::Serialize)]
struct OllamaEmbedBody<'a> {
    model: &'a str,
    input: &'a [&'a str],
    // Ollama must reject inputs exceeding the model context instead of silently
    // embedding a truncated prefix under the original chunk identity.
    truncate: bool,
}

/// One owner for real transport and inert regression transport. Preflight all
/// inputs before I/O: a single encoded input plus envelope must fit one request,
/// otherwise reject the entire operation without truncation or splitting a chunk.
/// Replies remain private until every ordered batch passes strict validation.
async fn ollama_embed_batches<F, Fut>(
    model: &str,
    texts: &[&str],
    mut transport: F,
) -> Result<Vec<Vec<f32>>, GlossError>
where
    F: FnMut(Vec<u8>) -> Fut,
    Fut: std::future::Future<Output = Result<serde_json::Value, GlossError>>,
{
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let oversized = || {
        GlossError::Embedding(format!(
            "Ollama embedding model or single input exceeds {MAX_EMBEDDING_REQUEST_BYTES} request bytes"
        ))
    };
    let encode_error =
        |error| GlossError::Embedding(format!("Ollama embed request JSON encode failed: {error}"));
    // Raw length checks cap temporary serialization allocations even for text
    // whose JSON encoding expands (control characters, quotes, backslashes).
    if model.len() > MAX_EMBEDDING_REQUEST_BYTES {
        return Err(oversized());
    }
    let envelope = serde_json::to_vec(&OllamaEmbedBody {
        model,
        input: &[],
        truncate: false,
    })
    .map_err(encode_error)?
    .len();
    if envelope > MAX_EMBEDDING_REQUEST_BYTES {
        return Err(oversized());
    }
    let mut batches = Vec::new();
    let mut start = 0;
    let mut bytes = envelope;
    for (index, text) in texts.iter().enumerate() {
        if text.len() > MAX_EMBEDDING_REQUEST_BYTES {
            return Err(oversized());
        }
        let encoded = serde_json::to_vec(text).map_err(encode_error)?.len();
        if encoded > MAX_EMBEDDING_REQUEST_BYTES - envelope {
            return Err(oversized());
        }
        let separator = usize::from(index > start);
        if index - start == MAX_EMBEDDING_BATCH_INPUTS
            || bytes + separator + encoded > MAX_EMBEDDING_REQUEST_BYTES
        {
            batches.push(start..index);
            start = index;
            bytes = envelope;
        }
        bytes += usize::from(index > start) + encoded;
    }
    batches.push(start..texts.len());

    let mut vectors = Vec::with_capacity(texts.len());
    let mut dimensions = None;
    for batch in batches {
        let input = &texts[batch];
        let body = serde_json::to_vec(&OllamaEmbedBody {
            model,
            input,
            truncate: false,
        })
        .map_err(encode_error)?;
        let reply = transport(body).await?;
        let rows = parse_embeddings(&reply, input.len(), dimensions)?;
        dimensions = rows.first().map(Vec::len);
        vectors.extend(rows);
    }
    Ok(vectors)
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

    fn mapped_reply(body: &[u8]) -> serde_json::Value {
        let request: serde_json::Value = serde_json::from_slice(body).unwrap();
        let rows: Vec<_> = request["input"]
            .as_array()
            .unwrap()
            .iter()
            .map(|text| {
                let index: usize = text
                    .as_str()
                    .unwrap()
                    .strip_prefix("row:")
                    .unwrap()
                    .parse()
                    .unwrap();
                vec![index as f32, -(index as f32)]
            })
            .collect();
        json!({"embeddings": rows})
    }

    #[tokio::test]
    async fn ollama_batches_bound_1_65_and_1000_inputs_with_exact_vector_mapping() {
        for count in [1, 65, 1000] {
            let owned: Vec<_> = (0..count).map(|i| format!("row:{i}")).collect();
            let texts: Vec<_> = owned.iter().map(String::as_str).collect();
            let mut sent = Vec::new();
            let model = "fixture\\\"模型";
            let vectors = ollama_embed_batches(model, &texts, |body| {
                assert!(body.len() <= MAX_EMBEDDING_REQUEST_BYTES);
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(request["model"], model);
                assert_eq!(request["truncate"], false);
                let input = request["input"].as_array().unwrap();
                assert!(!input.is_empty() && input.len() <= MAX_EMBEDDING_BATCH_INPUTS);
                sent.push(input.len());
                std::future::ready(Ok(mapped_reply(&body)))
            })
            .await
            .unwrap();
            let expected_batches: Vec<_> = texts.chunks(64).map(<[&str]>::len).collect();
            assert_eq!(sent, expected_batches);
            assert_eq!(vectors.len(), count);
            for (index, vector) in vectors.iter().enumerate() {
                assert_eq!(*vector, vec![index as f32, -(index as f32)]);
            }
        }
    }

    #[tokio::test]
    async fn ollama_batches_bound_escaped_request_bytes_and_reject_oversized_input_before_io() {
        // JSON control-character escaping expands each byte sixfold. These
        // inputs fit singly but require separate requests before the count cap.
        let large = "\u{0001}".repeat(MAX_EMBEDDING_REQUEST_BYTES / 12);
        let texts = [large.as_str(); 3];
        let mut calls = 0;
        let vectors = ollama_embed_batches("fixture", &texts, |body| {
            assert!(body.len() <= MAX_EMBEDDING_REQUEST_BYTES);
            let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
            let inputs = request["input"].as_array().unwrap();
            assert_eq!(inputs.len(), 1);
            assert_eq!(inputs[0], large);
            calls += 1;
            std::future::ready(Ok(json!({"embeddings": [[1, 0]]})))
        })
        .await
        .unwrap();
        assert_eq!(calls, 3);
        assert_eq!(vectors.len(), 3);

        for oversized in [
            "x".repeat(MAX_EMBEDDING_REQUEST_BYTES),
            "\u{0001}".repeat(MAX_EMBEDDING_REQUEST_BYTES / 6),
        ] {
            let mut calls = 0;
            let error = ollama_embed_batches("fixture", &["row:0", &oversized], |_| {
                calls += 1;
                std::future::ready(Ok(json!({"embeddings": [[1, 0]]})))
            })
            .await
            .unwrap_err();
            assert_eq!(calls, 0);
            assert!(error.to_string().contains("single input exceeds"));
            assert!(!error.to_string().contains(&oversized));
        }
    }

    #[tokio::test]
    async fn ollama_batches_reject_invalid_late_replies_without_complete_success() {
        let owned: Vec<_> = (0..129).map(|i| format!("row:{i}")).collect();
        let texts: Vec<_> = owned.iter().map(String::as_str).collect();
        for invalid in [
            json!({}),
            json!({"embeddings": []}),
            json!({"embeddings": vec![vec![0.0]; 64]}), // Cross-batch dimension drift.
            json!({"embeddings": vec![json!([]); 64]}),
            json!({"embeddings": vec![json!(["1", 0]); 64]}),
            json!({"embeddings": vec![json!([1e100, 0]); 64]}),
        ] {
            let mut calls = 0;
            let result = ollama_embed_batches("fixture", &texts, |body| {
                calls += 1;
                std::future::ready(Ok(if calls == 1 {
                    mapped_reply(&body)
                } else {
                    invalid.clone()
                }))
            })
            .await;
            assert!(matches!(result, Err(GlossError::Embedding(_))), "{invalid}");
            assert_eq!(calls, 2); // No third request or partial-success return.
        }
        let mut calls = 0;
        let error = ollama_embed_batches("fixture", &texts, |body| {
            calls += 1;
            std::future::ready(if calls == 1 {
                Ok(mapped_reply(&body))
            } else {
                Err(GlossError::Embedding(
                    "Ollama embed returned HTTP 503 Service Unavailable".into(),
                ))
            })
        })
        .await
        .unwrap_err();
        assert_eq!(calls, 2);
        assert!(error.to_string().contains("HTTP 503 Service Unavailable"));
    }

    #[tokio::test]
    async fn ollama_batches_empty_input_has_no_transport_and_cancellation_drops_pending_work() {
        use std::future::Future;
        use std::sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        };
        let empty = ollama_embed_batches("fixture", &[], |_| {
            panic!("Empty input must not call transport");
            #[allow(unreachable_code)]
            std::future::ready(Ok(json!({})))
        })
        .await
        .unwrap();
        assert!(empty.is_empty());

        struct DropMark(Arc<AtomicBool>);
        impl Drop for DropMark {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicBool::new(false));
        let mut calls = 0;
        let texts = ["row:0"; 129];
        let mut operation = Box::pin(ollama_embed_batches("fixture", &texts, |body| {
            calls += 1;
            let call = calls;
            let dropped = Arc::clone(&dropped);
            async move {
                if call == 1 {
                    return Ok(mapped_reply(&body));
                }
                let _guard = DropMark(dropped);
                std::future::pending::<Result<serde_json::Value, GlossError>>().await
            }
        }));
        std::future::poll_fn(|cx| {
            assert!(operation.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert!(!dropped.load(Ordering::SeqCst));
        drop(operation);
        assert!(dropped.load(Ordering::SeqCst));
        assert_eq!(calls, 2);
    }

    #[test]
    fn embedding_settings_defaults_and_supported_aliases_have_one_identity() {
        let config =
            NativeEmbeddingConfig::from_settings(None, None, None, None, false, false).unwrap();
        assert_eq!(config.provider, "ollama");
        assert_eq!(config.model, "bge-m3");
        assert_eq!(config.timeout_secs, 60);
        for provider in ["fastembed", "native"] {
            let config = NativeEmbeddingConfig::from_settings(
                Some(provider),
                None,
                None,
                None,
                false,
                false,
            )
            .unwrap();
            assert_eq!(config.provider, provider);
            assert_eq!(config.model, BUILT_IN_EMBEDDING_MODEL);
        }
    }

    #[test]
    fn embedding_settings_reject_malformed_values_before_an_embedder_exists() {
        for provider in ["", "OLLAMA", " ollama", "unknown"] {
            assert!(NativeEmbeddingConfig::from_settings(
                Some(provider),
                None,
                None,
                None,
                false,
                false
            )
            .is_err());
        }
        for provider in ["ollama", "fastembed", "native"] {
            for timeout in ["", "oops", "1", "301", " 60"] {
                assert!(NativeEmbeddingConfig::from_settings(
                    Some(provider),
                    None,
                    None,
                    Some(timeout),
                    false,
                    false
                )
                .is_err());
            }
            assert!(NativeEmbeddingConfig::from_settings(
                Some(provider),
                None,
                Some(" "),
                None,
                false,
                false
            )
            .is_err());
            assert!(NativeEmbeddingConfig::from_settings(
                Some(provider),
                Some(" "),
                None,
                None,
                false,
                false
            )
            .is_err());
        }
        assert!(NativeEmbeddingConfig::from_settings(
            Some("ollama"),
            Some("http://192.168.1.2:11434"),
            None,
            None,
            false,
            false
        )
        .is_err());
    }

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
            let received = stream.read(&mut buffer).unwrap();
            assert!(received > 0, "mock server received no request bytes");
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
            let received = stream.read(&mut buffer).unwrap();
            assert!(received > 0, "mock server received no request bytes");
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
