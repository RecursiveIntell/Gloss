//! Real loopback HTTP through actual Gloss transport and the registry Embedder trait.
use gloss_turbo_quant_runtime_gates::ingestion::embedding_contract::{
    bounded_ollama_json, ollama_client, ollama_probe_dimension,
};
use gloss_turbo_quant_runtime_gates::ollama_embedder::GlossOllamaEmbedder;
use semantic_memory::{Embedder, MemoryConfig, MemoryStore};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_request(stream: &mut TcpStream) -> String {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 4096];
        let count = tokio::time::timeout(Duration::from_secs(3), stream.read(&mut chunk))
            .await
            .unwrap()
            .unwrap();
        assert!(count > 0, "request closed before body completed");
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() <= 64 * 1024);
        if let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            let headers = String::from_utf8_lossy(&bytes[..header_end]);
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap_or(0);
            if bytes.len() >= header_end + 4 + length {
                return String::from_utf8(bytes).unwrap();
            }
        }
    }
}

async fn responses(responses: Vec<String>) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let worker = tokio::spawn(async move {
        let mut requests = Vec::new();
        for response in responses {
            let (mut stream, _) = tokio::time::timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            requests.push(read_request(&mut stream).await);
            stream.write_all(response.as_bytes()).await.unwrap();
        }
        requests
    });
    (url, worker)
}

fn json_response(status: u16, body: &str) -> String {
    format!("HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
}

#[tokio::test]
async fn registry_store_uses_actual_guarded_embedder_and_preserves_request_identity() {
    let (url, server) = responses(vec![json_response(200, r#"{"embeddings":[[1,0,0]]}"#)]).await;
    let embedder = GlossOllamaEmbedder::try_new(&url, "fixture-model", 3, 2, false).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let mut config = MemoryConfig {
        base_dir: directory.path().into(),
        ..Default::default()
    };
    config.embedding.dimensions = 3;
    let store = MemoryStore::open_with_embedder(config, Box::new(embedder)).unwrap();
    assert_eq!(
        store.embed_batch(&["canonical source text"]).await.unwrap(),
        vec![vec![1.0, 0.0, 0.0]]
    );
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("POST /api/embed "));
    let body: serde_json::Value =
        serde_json::from_str(requests[0].split_once("\r\n\r\n").unwrap().1).unwrap();
    assert_eq!(body["model"], "fixture-model");
    // Registry purpose-prefixing remains canonical; the bridge forwards the
    // registry's prepared text unchanged.
    assert_eq!(
        body["input"],
        serde_json::json!(["search_document: canonical source text"])
    );
}

#[tokio::test]
async fn semantic_embedder_rejects_redirects_without_forwarding_source() {
    let target = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let response = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: http://{}/api/embed\r\nContent-Length: 0\r\nConnection: close\r\n\r\n", target.local_addr().unwrap());
    let (url, server) = responses(vec![response.clone()]).await;
    let embedder = GlossOllamaEmbedder::try_new(&url, "fixture", 3, 2, false).unwrap();
    assert!(embedder
        .embed("private text")
        .await
        .unwrap_err()
        .to_string()
        .contains("307"));
    server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), target.accept())
            .await
            .is_err()
    );
    let (url, server) = responses(vec![response]).await;
    assert!(ollama_probe_dimension(&url, "fixture", 2, false)
        .await
        .unwrap_err()
        .to_string()
        .contains("redirect"));
    server.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), target.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn semantic_constructor_and_probe_require_explicit_endpoint_authority() {
    for url in [
        "http://192.168.1.2:11434",
        "https://example.com",
        "http://localhost:11434?token=secret",
    ] {
        assert!(GlossOllamaEmbedder::try_new(url, "fixture", 3, 2, false).is_err());
        let error = ollama_probe_dimension(url, "fixture", 2, false)
            .await
            .unwrap_err();
        assert!(
            matches!(
                error,
                gloss_turbo_quant_runtime_gates::error::GlossError::Config(_)
            ),
            "{error}"
        );
    }
    assert!(
        GlossOllamaEmbedder::try_new("http://192.168.1.2:11434", "fixture", 3, 2, true).is_ok()
    );
}

#[tokio::test]
async fn shared_reader_bounds_content_length_and_chunked_json() {
    let bodies = vec![
        "HTTP/1.1 200 OK\r\nContent-Length: 1000\r\nConnection: close\r\n\r\n".to_string(),
        "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n10\r\n0123456789abcdef\r\n10\r\n0123456789abcdef\r\n0\r\n\r\n".to_string(),
    ];
    for body in bodies {
        let (url, server) = responses(vec![body]).await;
        let client = ollama_client(&url, "fixture", 2, false).unwrap();
        let response = client.get(&url).send().await.unwrap();
        assert!(bounded_ollama_json(response, 24)
            .await
            .unwrap_err()
            .to_string()
            .contains("exceeds 24 bytes"));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn strict_vectors_and_dimensions_fail_in_actual_registry_trait() {
    for body in [
        r#"{"embeddings":[["1",0,0]]}"#,
        r#"{"embeddings":[[1,0]]}"#,
        r#"{"embeddings":[[1e100,0,0]]}"#,
        r#"{"embeddings":[[1,0,0],[0,1,0]]}"#,
    ] {
        let (url, server) = responses(vec![json_response(200, body)]).await;
        let embedder = GlossOllamaEmbedder::try_new(&url, "fixture", 3, 2, false).unwrap();
        assert!(embedder.embed("fixture text").await.is_err(), "{body}");
        server.await.unwrap();
    }
}

#[tokio::test]
async fn real_probe_preserves_embed_show_and_failure_truth_without_defaults() {
    let (url, server) = responses(vec![json_response(200, r#"{"embeddings":[[1,0,0]]}"#)]).await;
    assert_eq!(
        ollama_probe_dimension(&url, "fixture", 2, false)
            .await
            .unwrap(),
        3
    );
    assert_eq!(server.await.unwrap().len(), 1);
    let (url, server) = responses(vec![
        json_response(500, "{}"),
        json_response(200, r#"{"model_info":{"nomic.embedding_length":768}}"#),
    ])
    .await;
    assert_eq!(
        ollama_probe_dimension(&url, "fixture", 2, false)
            .await
            .unwrap(),
        768
    );
    let requests = server.await.unwrap();
    assert!(requests[0].starts_with("POST /api/embed "));
    assert!(requests[1].starts_with("POST /api/show "));
    for show in [
        json_response(500, "{}"),
        json_response(
            200,
            r#"{"model_info":{"context_length":8192,"parameters":1024}}"#,
        ),
    ] {
        let (url, server) = responses(vec![json_response(500, "{}"), show]).await;
        let error = ollama_probe_dimension(&url, "fixture", 2, false)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            error.contains("/api/embed") && error.contains("/api/show"),
            "{error}"
        );
        server.await.unwrap();
    }
    let (url, server) = responses(vec![json_response(200, r#"{"embeddings":[["768"]]}"#)]).await;
    assert!(ollama_probe_dimension(&url, "fixture", 2, false)
        .await
        .is_err());
    assert_eq!(server.await.unwrap().len(), 1);
}

#[tokio::test]
async fn dropping_guarded_embedding_future_closes_http_without_detached_work() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let (arrived, receive) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request(&mut stream).await;
        arrived.send(()).unwrap();
        let mut buffer = [0];
        // No response: cancellation must close this in-flight request well
        // before the client's 30-second timeout.
        tokio::time::timeout(Duration::from_secs(1), stream.read(&mut buffer))
            .await
            .unwrap()
            .unwrap()
    });
    let embedder = GlossOllamaEmbedder::try_new(&url, "fixture", 3, 30, false).unwrap();
    let gpu = tokio::sync::Semaphore::new(1);
    let llm = tokio::sync::Semaphore::new(1);
    let mut operation = Box::pin(async {
        let _guard = gloss_native_contract_tests::native_gates::acquire(&gpu, &llm)
            .await
            .unwrap();
        embedder.embed("cancelled source text").await
    });
    tokio::select! {
        result = &mut operation => panic!("request unexpectedly completed: {result:?}"),
        arrived = receive => arrived.unwrap(),
    }
    assert!(gpu.try_acquire().is_err());
    drop(operation);
    assert!(gpu.try_acquire().is_ok());
    assert!(llm.try_acquire().is_ok());
    assert_eq!(
        server.await.unwrap(),
        0,
        "cancelled HTTP connection must close"
    );
}
