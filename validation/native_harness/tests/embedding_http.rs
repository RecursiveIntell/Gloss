//! Real disposable loopback HTTP through the canonical async embedding owner.
//! These fixtures prove local request/return/drop behavior, not model inference,
//! remote cancellation, or the cooperative fixed 300-second aggregate deadline.
use gloss_native_contract_tests::{embedding_contract, error::GlossError};
use serde_json::{json, Value};
use std::{future::Future, time::Duration};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};

const TEST_BOUND: Duration = Duration::from_secs(8);
const IO_BOUND: Duration = Duration::from_secs(3);
const QUIET_PERIOD: Duration = Duration::from_millis(100);
const REQUEST_LIMIT: usize = embedding_contract::MAX_EMBEDDING_REQUEST_BYTES;

async fn bounded_test(future: impl Future<Output = ()>) {
    tokio::time::timeout(TEST_BOUND, future)
        .await
        .expect("entire loopback test exceeded eight seconds");
}

#[derive(Debug)]
struct CapturedRequest {
    line: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).expect("actual request must be JSON")
    }
}

async fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    let mut bytes = Vec::new();
    loop {
        let mut chunk = [0; 16 * 1024];
        let count = tokio::time::timeout(IO_BOUND, stream.read(&mut chunk))
            .await
            .expect("fixture request read timed out")
            .expect("fixture request read failed");
        assert_ne!(count, 0, "client closed before complete request");
        bytes.extend_from_slice(&chunk[..count]);
        assert!(bytes.len() <= REQUEST_LIMIT + 16 * 1024);
        let Some(header_end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") else {
            continue;
        };
        let header_text = std::str::from_utf8(&bytes[..header_end]).unwrap();
        let mut lines = header_text.split("\r\n");
        let line = lines.next().unwrap().to_string();
        let headers = lines
            .map(|line| {
                let (name, value) = line.split_once(':').expect("HTTP header separator");
                (name.to_string(), value.trim().to_string())
            })
            .collect::<Vec<_>>();
        let lengths = headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .collect::<Vec<_>>();
        assert_eq!(
            lengths.len(),
            1,
            "pre-encoded request needs one body length"
        );
        let length: usize = lengths[0].1.parse().unwrap();
        assert!(length <= REQUEST_LIMIT);
        assert!(!headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("transfer-encoding")));
        let body_start = header_end + 4;
        if bytes.len() < body_start + length {
            continue;
        }
        assert_eq!(bytes.len(), body_start + length);
        return CapturedRequest {
            line,
            headers,
            body: bytes[body_start..].to_vec(),
        };
    }
}

enum Reply {
    Echo,
    Raw(Vec<u8>),
    StallHeaders,
    StallBody,
}

#[derive(Debug, PartialEq, Eq)]
enum Event {
    Stalled(usize),
    PeerClosed(usize),
}

#[derive(Default)]
struct ServerLog {
    accepted_connections: usize,
    requests: Vec<CapturedRequest>,
    peer_closed: Vec<usize>,
}

struct FakeOllama {
    url: String,
    stop: Option<oneshot::Sender<()>>,
    events: mpsc::UnboundedReceiver<Event>,
    worker: JoinHandle<ServerLog>,
}

impl Drop for FakeOllama {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // Also cancel the fixture on assertion/outer-timeout failure. Normal
        // paths join it below, leaving no listener or request task detached.
        self.worker.abort();
    }
}

impl FakeOllama {
    async fn start(replies: Vec<Reply>) -> Self {
        let listener = tokio::time::timeout(IO_BOUND, TcpListener::bind("127.0.0.1:0"))
            .await
            .unwrap()
            .unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let (stop, mut stopped) = oneshot::channel();
        let (events, receive_events) = mpsc::unbounded_channel();
        let worker = tokio::spawn(async move {
            let mut replies = replies.into_iter();
            let mut log = ServerLog::default();
            let mut offset = 0usize;
            loop {
                let accepted = tokio::select! {
                    biased;
                    _ = &mut stopped => break,
                    accepted = listener.accept() => accepted.unwrap(),
                };
                let (mut stream, _) = accepted;
                log.accepted_connections += 1;
                let request = read_request(&mut stream).await;
                let input_count = request.json()["input"].as_array().unwrap().len();
                log.requests.push(request);
                let request_index = log.requests.len();
                // Unexpected later requests remain observable and get a reply,
                // so a broken wrapper cannot hide a third batch by hanging.
                let reply = replies.next().unwrap_or(Reply::Echo);
                let stall = matches!(reply, Reply::StallHeaders | Reply::StallBody);
                let response = match reply {
                    Reply::Echo => json_response(200, &echo_body(offset, input_count)),
                    Reply::Raw(response) => response,
                    Reply::StallHeaders => Vec::new(),
                    Reply::StallBody => b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 1024\r\nConnection: close\r\n\r\n{".to_vec(),
                };
                offset += input_count;
                tokio::time::timeout(IO_BOUND, stream.write_all(&response))
                    .await
                    .unwrap()
                    .unwrap();
                if stall {
                    events.send(Event::Stalled(request_index)).unwrap();
                    let mut byte = [0];
                    let closed = tokio::select! {
                        biased;
                        _ = &mut stopped => break,
                        result = stream.read(&mut byte) => result,
                    };
                    assert!(
                        matches!(closed, Ok(0))
                            || closed.as_ref().err().is_some_and(|error| {
                                matches!(
                                    error.kind(),
                                    std::io::ErrorKind::ConnectionReset
                                        | std::io::ErrorKind::ConnectionAborted
                                )
                            }),
                        "pending HTTP socket must close locally: {closed:?}"
                    );
                    log.peer_closed.push(request_index);
                    events.send(Event::PeerClosed(request_index)).unwrap();
                }
                // Connection: close makes each captured request independent.
                drop(stream);
            }
            log
        });
        Self {
            url,
            stop: Some(stop),
            events: receive_events,
            worker,
        }
    }

    async fn event(&mut self, expected: Event) {
        assert_eq!(
            tokio::time::timeout(IO_BOUND, self.events.recv())
                .await
                .expect("fixture event timed out")
                .expect("fixture event channel closed"),
            expected
        );
    }

    async fn finish(mut self) -> ServerLog {
        // Keep the listener alive after wrapper completion/drop to observe any
        // erroneous subsequent HTTP work before stopping and joining it.
        tokio::time::sleep(QUIET_PERIOD).await;
        self.stop.take().unwrap().send(()).unwrap();
        tokio::time::timeout(IO_BOUND, &mut self.worker)
            .await
            .expect("fixture worker did not join")
            .expect("fixture worker failed")
    }
}

fn json_response(status: u16, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} Fixture\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

fn vector(index: usize) -> Vec<f32> {
    vec![index as f32, index as f32 + 0.25, -(index as f32)]
}

fn echo_body(offset: usize, count: usize) -> String {
    json!({"embeddings": (offset..offset + count).map(vector).collect::<Vec<_>>()}).to_string()
}

fn inputs(count: usize) -> Vec<String> {
    (0..count)
        .map(|index| format!("row-{index:04} \"quoted\" \\ \\n \u{0001} λ"))
        .collect()
}

fn wire_body(model: &str, texts: &[&str]) -> Vec<u8> {
    // Independent wire oracle uses actual JSON escaping and explicit field
    // order; it does not invoke the private batching helper or fake transport.
    format!(
        "{{\"model\":{},\"input\":{},\"truncate\":false}}",
        serde_json::to_string(model).unwrap(),
        serde_json::to_string(texts).unwrap()
    )
    .into_bytes()
}

fn assert_request(request: &CapturedRequest, model: &str, texts: &[&str]) {
    assert_eq!(request.line, "POST /api/embed HTTP/1.1");
    assert_eq!(request.header("content-type"), Some("application/json"));
    assert_eq!(
        request
            .header("content-length")
            .unwrap()
            .parse::<usize>()
            .unwrap(),
        request.body.len()
    );
    assert!(request.body.len() <= REQUEST_LIMIT);
    assert_eq!(request.body, wire_body(model, texts));
    let body = request.json();
    assert_eq!(body["model"], model);
    assert_eq!(body["input"], json!(texts));
    assert_eq!(body["truncate"], false);
}

fn embedding_error(error: GlossError, expected: &str) {
    match error {
        GlossError::Embedding(message) => assert!(message.contains(expected), "{message}"),
        other => panic!("expected typed embedding failure, got {other:?}"),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn actual_http_count_boundaries_preserve_every_input_and_vector() {
    bounded_test(async {
        let model = "fixture-\"model\"\\\nλ";
        for count in [1, 64, 65, 128, 129] {
            let fixture = FakeOllama::start(Vec::new()).await;
            let client = embedding_contract::ollama_client(&fixture.url, model, 2, false).unwrap();
            let strings = inputs(count);
            let texts = strings.iter().map(String::as_str).collect::<Vec<_>>();
            let result =
                embedding_contract::ollama_embed_request(&client, &fixture.url, model, &texts)
                    .await;
            let log = fixture.finish().await;
            assert_eq!(result.unwrap(), (0..count).map(vector).collect::<Vec<_>>());
            assert_eq!(log.requests.len(), count.div_ceil(64));
            for (request, expected) in log.requests.iter().zip(texts.chunks(64)) {
                assert_request(request, model, expected);
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn exact_encoded_mebibyte_reaches_http_with_escaped_model_overhead() {
    bounded_test(async {
        let model = "fixture-\"model\"\\\n\u{0001}λ";
        let empty_input_size = wire_body(model, &[""]).len();
        let text = "x".repeat(REQUEST_LIMIT - empty_input_size);
        let texts = [text.as_str()];
        assert_eq!(wire_body(model, &texts).len(), REQUEST_LIMIT);
        let fixture = FakeOllama::start(Vec::new()).await;
        let client = embedding_contract::ollama_client(&fixture.url, model, 2, false).unwrap();
        let result =
            embedding_contract::ollama_embed_request(&client, &fixture.url, model, &texts).await;
        let log = fixture.finish().await;
        assert_eq!(result.unwrap(), vec![vector(0)]);
        assert_eq!(log.requests.len(), 1);
        assert_request(&log.requests[0], model, &texts);
        assert_eq!(log.requests[0].body.len(), REQUEST_LIMIT);
    })
    .await;
}

async fn reject_without_http(model: &str, texts: &[&str]) {
    let fixture = FakeOllama::start(Vec::new()).await;
    let client = embedding_contract::ollama_client(&fixture.url, model, 2, false).unwrap();
    let result =
        embedding_contract::ollama_embed_request(&client, &fixture.url, model, texts).await;
    let log = fixture.finish().await;
    embedding_error(result.unwrap_err(), "single input exceeds");
    assert_eq!(log.accepted_connections, 0);
    assert!(log.requests.is_empty(), "preflight must precede all HTTP");
}

#[tokio::test(flavor = "current_thread")]
async fn encoded_one_byte_over_and_expanding_escape_reject_before_http() {
    bounded_test(async {
        let model = "fixture-\"model\"\\\n\u{0001}λ";
        let overhead = wire_body(model, &[""]).len();
        let over = "x".repeat(REQUEST_LIMIT - overhead + 1);
        assert_eq!(wire_body(model, &[&over]).len(), REQUEST_LIMIT + 1);
        reject_without_http(model, &[&over]).await;

        let escaped = "\u{0001}".repeat((REQUEST_LIMIT - overhead) / 6 + 1);
        assert!(escaped.len() < REQUEST_LIMIT);
        assert!(wire_body(model, &[&escaped]).len() > REQUEST_LIMIT);
        reject_without_http(model, &[&escaped]).await;

        let escaped_model = "\u{0001}".repeat(REQUEST_LIMIT / 6 + 1);
        assert!(escaped_model.len() < REQUEST_LIMIT);
        assert!(wire_body(&escaped_model, &[]).len() > REQUEST_LIMIT);
        reject_without_http(&escaped_model, &["small input"]).await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn oversized_input_after_full_first_batch_prevents_all_http() {
    bounded_test(async {
        let model = "fixture";
        let mut strings = inputs(64);
        strings.push("x".repeat(REQUEST_LIMIT - wire_body(model, &[""]).len() + 1));
        let texts = strings.iter().map(String::as_str).collect::<Vec<_>>();
        assert_eq!(wire_body(model, &[texts[64]]).len(), REQUEST_LIMIT + 1);
        reject_without_http(model, &texts).await;
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn byte_boundaries_split_escaped_inputs_without_splitting_a_chunk() {
    bounded_test(async {
        let model = "fixture-\"model\"\\\nλ";
        let first = "\u{0001}".repeat(100_000);
        let second = "\u{0001}".repeat(100_000);
        let third = "final λ";
        let texts = [first.as_str(), second.as_str(), third];
        assert!(wire_body(model, &texts[..2]).len() > REQUEST_LIMIT);
        assert!(wire_body(model, &texts[1..]).len() < REQUEST_LIMIT);
        let fixture = FakeOllama::start(Vec::new()).await;
        let client = embedding_contract::ollama_client(&fixture.url, model, 2, false).unwrap();
        let result =
            embedding_contract::ollama_embed_request(&client, &fixture.url, model, &texts).await;
        let log = fixture.finish().await;
        assert_eq!(result.unwrap(), (0..3).map(vector).collect::<Vec<_>>());
        assert_eq!(log.requests.len(), 2);
        assert_request(&log.requests[0], model, &texts[..1]);
        assert_request(&log.requests[1], model, &texts[1..]);
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn adding_one_encoded_byte_splits_an_exact_limit_two_input_body() {
    bounded_test(async {
        let model = "fixture-\"model\"\\\nλ";
        let suffix = "\u{0001}\\\"λ";
        let overhead = wire_body(model, &["", suffix]).len();
        for added_byte in [0, 1] {
            let first = "x".repeat(REQUEST_LIMIT - overhead + added_byte);
            let texts = [first.as_str(), suffix];
            assert_eq!(wire_body(model, &texts).len(), REQUEST_LIMIT + added_byte);
            let fixture = FakeOllama::start(Vec::new()).await;
            let client = embedding_contract::ollama_client(&fixture.url, model, 2, false).unwrap();
            let result =
                embedding_contract::ollama_embed_request(&client, &fixture.url, model, &texts)
                    .await;
            let log = fixture.finish().await;
            assert_eq!(result.unwrap(), vec![vector(0), vector(1)]);
            assert_eq!(log.requests.len(), 1 + added_byte);
            if added_byte == 0 {
                assert_request(&log.requests[0], model, &texts);
            } else {
                assert_request(&log.requests[0], model, &texts[..1]);
                assert_request(&log.requests[1], model, &texts[1..]);
            }
        }
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn empty_input_returns_empty_without_http() {
    bounded_test(async {
        let fixture = FakeOllama::start(Vec::new()).await;
        let client = embedding_contract::ollama_client(&fixture.url, "fixture", 2, false).unwrap();
        let result =
            embedding_contract::ollama_embed_request(&client, &fixture.url, "fixture", &[]).await;
        let log = fixture.finish().await;
        assert!(result.unwrap().is_empty());
        assert!(log.requests.is_empty());
    })
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn later_real_http_and_response_failures_return_error_without_third_batch() {
    bounded_test(async {
        let changed_dims = json!({"embeddings": vec![vec![1, 2]; 64]}).to_string();
        let numeric = json!({"embeddings": vec![json!(["bad", 1, 2]); 64]}).to_string();
        let nonfinite = format!("{{\"embeddings\":[{}]}}", ["[1e100,1,2]"; 64].join(","));
        let oversized = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            embedding_contract::MAX_EMBEDDING_RESPONSE_BYTES + 1
        )
        .into_bytes();
        let truncated =
            b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\n{}".to_vec();
        let cases = [
            (json_response(503, "{}"), "HTTP 503"),
            (json_response(200, "{"), "JSON parse failed"),
            (oversized, "JSON response exceeds"),
            (truncated, "response read failed"),
            (
                json_response(200, &echo_body(64, 63)),
                "count does not match",
            ),
            (json_response(200, &changed_dims), "inconsistent dimensions"),
            (json_response(200, &numeric), "nonnumeric value"),
            (json_response(200, &nonfinite), "nonfinite value"),
        ];
        let strings = inputs(129);
        let texts = strings.iter().map(String::as_str).collect::<Vec<_>>();
        for (response, expected_error) in cases {
            let fixture = FakeOllama::start(vec![Reply::Echo, Reply::Raw(response)]).await;
            let client =
                embedding_contract::ollama_client(&fixture.url, "fixture", 2, false).unwrap();
            let result =
                embedding_contract::ollama_embed_request(&client, &fixture.url, "fixture", &texts)
                    .await;
            let log = fixture.finish().await;
            // The first 64 valid rows remain private: the public return is Err.
            embedding_error(result.unwrap_err(), expected_error);
            assert_eq!(log.requests.len(), 2, "{expected_error}");
            assert_request(&log.requests[0], "fixture", &texts[..64]);
            assert_request(&log.requests[1], "fixture", &texts[64..128]);
        }
    })
    .await;
}

async fn request_timeout_after_first_batch(stall: Reply, expected_error: &str) {
    let mut fixture = FakeOllama::start(vec![Reply::Echo, stall]).await;
    let client = embedding_contract::ollama_client(&fixture.url, "fixture", 2, false).unwrap();
    let strings = inputs(129);
    let texts = strings.iter().map(String::as_str).collect::<Vec<_>>();
    let started = std::time::Instant::now();
    let result =
        embedding_contract::ollama_embed_request(&client, &fixture.url, "fixture", &texts).await;
    let elapsed = started.elapsed();
    fixture.event(Event::Stalled(2)).await;
    fixture.event(Event::PeerClosed(2)).await;
    let log = fixture.finish().await;
    embedding_error(result.unwrap_err(), expected_error);
    // The separate eight-second watchdog cannot account for this Err. This
    // checks the configured two-second request timer, with scheduling tolerance.
    assert!(elapsed >= Duration::from_millis(1500), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(log.requests.len(), 2);
    assert_eq!(log.peer_closed, vec![2]);
    assert_request(&log.requests[0], "fixture", &texts[..64]);
    assert_request(&log.requests[1], "fixture", &texts[64..128]);
}

#[tokio::test(flavor = "current_thread")]
async fn configured_request_timeout_covers_stalled_second_headers() {
    bounded_test(request_timeout_after_first_batch(
        Reply::StallHeaders,
        "Ollama embed request failed:",
    ))
    .await;
}

#[tokio::test(flavor = "current_thread")]
async fn configured_request_timeout_covers_stalled_second_body() {
    bounded_test(request_timeout_after_first_batch(
        Reply::StallBody,
        "Ollama response read failed:",
    ))
    .await;
}

async fn drop_actual_wrapper_after_first_batch(stall: Reply) {
    let mut fixture = FakeOllama::start(vec![Reply::Echo, stall]).await;
    let url = fixture.url.clone();
    // A deadline longer than the entire test excludes timeout as the cause of
    // the socket closure credited to dropping the operation below.
    let client = embedding_contract::ollama_client(&url, "fixture", 60, false).unwrap();
    let strings = inputs(129);
    let texts = strings.iter().map(String::as_str).collect::<Vec<_>>();
    let mut operation = Box::pin(embedding_contract::ollama_embed_request(
        &client, &url, "fixture", &texts,
    ));
    tokio::select! {
        result = &mut operation => panic!("wrapper completed before pending second request: {result:?}"),
        _ = fixture.event(Event::Stalled(2)) => {},
    }
    // Keep polling the owner while delivered headers/body bytes become ready.
    // The body-stall fixture has already sent headers and a partial body; the
    // request must remain pending throughout this short local settling window.
    tokio::select! {
        result = &mut operation => panic!("wrapper completed during stalled response: {result:?}"),
        _ = tokio::time::sleep(Duration::from_millis(25)) => {},
    }
    let dropped = std::time::Instant::now();
    drop(operation);
    // Client stays alive: closure follows dropping the actual pending wrapper,
    // rather than destroying its client pool or waiting for request timeout.
    fixture.event(Event::PeerClosed(2)).await;
    assert!(dropped.elapsed() < Duration::from_secs(1));
    let log = fixture.finish().await;
    assert_eq!(log.requests.len(), 2);
    assert_eq!(log.peer_closed, vec![2]);
    assert_request(&log.requests[0], "fixture", &texts[..64]);
    assert_request(&log.requests[1], "fixture", &texts[64..128]);
    drop(client);
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_wrapper_with_second_headers_pending_closes_local_http() {
    bounded_test(drop_actual_wrapper_after_first_batch(Reply::StallHeaders)).await;
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_wrapper_with_second_body_pending_closes_local_http() {
    bounded_test(drop_actual_wrapper_after_first_batch(Reply::StallBody)).await;
}
